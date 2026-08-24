//! Reconciliation: codegen's emitted register set **agrees with** the shared
//! `copper_analysis::infer_registers` (item 2, "route regs into codegen").
//!
//! This makes the shared control/liveness analysis the *authoritative spec* for the
//! synthesizable register set: codegen still computes registers via its own
//! `chir_lower` (pre-loop `let mut`) + `shir_lower::find_promoted_wires` (in-loop
//! wires live across ticks), but that set is now pinned, corpus-wide, to equal the
//! shared inference. Any future divergence — in either the analysis or the codegen
//! heuristic — fails here. (Retiring the codegen heuristic to *consume* the shared
//! set directly is a behavior-neutral follow-up, since this proves they agree.)
//!
//! For each `#[hardware(sequential)]` module in the fixtures, transpile it and
//! extract the flip-flops of the *generated* SV (`<=` targets minus outputs, via
//! the same convention-based extractor G2 uses), `_r`-normalized, then compare to
//! `infer_registers`. The invariant, per module:
//!
//!   * every inferred register appears in codegen's output (`inferred ⊆ codegen`);
//!   * codegen adds only the **synthetic phase/pc counter** it introduces for
//!     multi-tick / control-extracted FSMs (`codegen − inferred ⊆ {phase, pc}`),
//!     which the source-level inference has no name for.
//!
//! **Memory arrays are not in scope** and are filtered out inside
//! `reference_sv_registers` rather than here: `mem[addr] <= data` reduces to
//! `mem <= data` once bit-selects are stripped, so a memory would otherwise read
//! as a flip-flop named `mem`. It is storage, but a different storage class —
//! `infer_registers` names locals live across a tick, and a `Memory<..>` binding
//! is not one. Adding the WriteFirst RAM fixtures is what first surfaced this.
//!
//! **Scope: clocked modules — `sequential` *and* `synchronizer`.** Synchronizers
//! were excluded until 2026-08-21, and that exclusion hid a real bug: inference
//! reported one flip-flop for the 2-FF synchronizer where codegen emits two (`ff2`
//! is defined post-tick and read pre-tick, so its live range crosses the loop back
//! edge but no tick edge). A sweep with the filter lifted showed that was the only
//! divergence in the corpus — 41 modules, three copies of the same synchronizer —
//! and it is fixed by the back-edge clause in `Cfg::registers`. With that fix all
//! 41 clocked modules that transpile agree. The filter is now lifted permanently so
//! the shape stays covered.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use syn::ItemFn;

/// Codegen's phase-FSM / control-extraction counter names, which the source-level
/// inference legitimately does not produce (they are synthesized during lowering).
const SYNTHETIC: &[&str] = &["phase", "pc"];

fn strip_r(name: &str) -> String {
    name.strip_suffix("_r").unwrap_or(name).to_string()
}

fn is_hardware(f: &ItemFn) -> bool {
    f.attrs.iter().any(|a| a.path().segments.last().is_some_and(|s| s.ident == "hardware"))
}

/// Clocked modes — the ones with a top-level ticking loop and therefore registers.
/// Excludes only `combinational`. See the scope note in the header for why
/// `synchronizer` is included.
fn is_clocked(f: &ItemFn) -> bool {
    // Read the FIRST token of `#[hardware(<mode>, <flags>…)]`. `parse_args::<Ident>()`
    // fails outright once a flag is present (e.g. `allow_pretick_alignment`), which
    // would silently drop such modules from this reconciliation.
    f.attrs.iter().any(|a| {
        if !a.path().segments.last().is_some_and(|s| s.ident == "hardware") {
            return false;
        }
        let Ok(list) = a.meta.require_list() else { return false };
        let text = list.tokens.to_string();
        let mode = text.split(',').next().unwrap_or("").trim().to_string();
        mode == "sequential" || mode == "synchronizer"
    })
}

#[test]
fn codegen_registers_match_shared_inference() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files: Vec<_> = fs::read_dir(root.join("tests/fixtures"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect();
    files.sort();

    let mut checked = 0usize;
    let mut violations = Vec::new();
    for path in &files {
        let src = fs::read_to_string(path).unwrap();
        let Ok(file) = syn::parse_file(&src) else { continue };
        let hw_fns: Vec<&ItemFn> = file
            .items
            .iter()
            .filter_map(|i| match i {
                syn::Item::Fn(f) if is_hardware(f) => Some(f),
                _ => None,
            })
            .collect();
        let multi = hw_fns.len() > 1;

        for f in hw_fns {
            if !is_clocked(f) {
                continue;
            }
            let name = f.sig.ident.to_string();
            let module = if multi { Some(name.as_str()) } else { None };
            let sv = match copper_codegen::transpile_source(&src, module, &copper_codegen::EmitConfig::default()) {
                Ok(sv) => sv,
                // Some fixtures exercise not-yet-transpilable shapes; skip — this
                // test is about *agreement where codegen produces output*.
                Err(_) => continue,
            };
            checked += 1;

            let inferred: BTreeSet<String> = copper_analysis::infer_registers(f).into_iter().collect();
            let codegen: BTreeSet<String> = copper_analysis::reference_sv_registers(&sv)
                .iter()
                .map(|n| strip_r(n))
                .collect();

            let missing: Vec<_> = inferred.difference(&codegen).cloned().collect();
            let extra: Vec<_> = codegen
                .difference(&inferred)
                .filter(|r| !SYNTHETIC.contains(&r.as_str()))
                .cloned()
                .collect();

            let where_ = format!("{}::{name}", path.file_name().unwrap().to_string_lossy());
            if !missing.is_empty() {
                violations.push(format!(
                    "{where_}: inferred registers missing from codegen output: {missing:?} \
                     (inferred {inferred:?}, codegen {codegen:?})"
                ));
            }
            if !extra.is_empty() {
                violations.push(format!(
                    "{where_}: codegen has non-synthetic registers the shared inference missed: \
                     {extra:?} (inferred {inferred:?}, codegen {codegen:?})"
                ));
            }
        }
    }

    assert!(checked >= 8, "expected to reconcile the clocked fixtures, only did {checked}");
    assert!(
        violations.is_empty(),
        "register-set reconciliation failed for {} module(s):\n{}",
        violations.len(),
        violations.join("\n")
    );
    eprintln!("register reconciliation: {checked} clocked modules — codegen ≡ shared inference (+ synthetic phase/pc)");
}
