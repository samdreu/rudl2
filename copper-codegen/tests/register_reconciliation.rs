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
//! Two REPRESENTATION mappings are applied before comparing (2026-08-27, the
//! aggregate-surface landing):
//!
//!   * a struct-typed register (`let mut latch = Latch::bubble()`) is inferred
//!     under its own name but FLATTENS to one flop per field
//!     (`latch_valid`, `latch_data`); the inferred name is satisfied by its
//!     complete field set, and those field flops are accounted for in reverse;
//!   * in a SINGLE-TICK module, a plain `let` that crosses the edge (`let xv =
//!     x.read(); tick; latch = f(xv)`) is inferred as a register but is
//!     ABSORBED by codegen: it stays a comb wire and every consumer flop
//!     samples it at the edge — the same machine, with the wire's storage
//!     living inside the consumer's flop. The allowance is narrow: it applies
//!     only when the body has exactly one tick (a multi-tick module must still
//!     PROMOTE such a wire to `{name}_r`, and a promotion regression still
//!     fails here) and only to names the SV actually declares as comb nets
//!     (directly, or renamed `{outer}_{name}` by the block-binding scoping).
//!     Behavioral equivalence of the absorption is the differential sweep's
//!     job — `tests/fixtures/struct_pipeline_dut.rs` pins these shapes.
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

/// `pc_<digits>` — the extraction counter's collision-fallback spelling (a
/// module that already names `pc`, like the pipelined CPU, gets `pc_1`).
fn is_pc_fallback(name: &str) -> bool {
    name.strip_prefix("pc_")
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// A register synthesized by the lowering rather than named in the source.
///
/// Two kinds: the phase/pc counter above, and a memory read port's pipeline stage
/// (`<mem>_rd<N>_q<K>` / `_v<K>`) or write pipeline stage (`<mem>_wr<N>_s<K>_…`),
/// which carry a port's latency. Neither has a source-level name —
/// `infer_registers` names locals that live across a tick, and a memory's internal
/// pipeline is not one; it belongs to the memory the way a submodule's registers
/// belong to the submodule.
///
/// The shape match is safe because `vlir_lower` RESERVES these names in the
/// legalizer from the memory declaration: a user signal that collided would be the
/// one renamed, so a name of this shape in the emitted SV really is a memory
/// pipeline stage.
fn is_synthetic(name: &str) -> bool {
    if SYNTHETIC.contains(&name) || is_pc_fallback(name) {
        return true;
    }
    // The counter control extraction synthesizes for `for _ in <range>` — the same
    // kind of thing as `pc`, and for the same reason the shared inference cannot
    // name it: `_` binds nothing, so there is no source-level name to infer. A
    // NAMED `for` binding (`for i in …`) is not synthetic and IS inferred; see
    // `DefinedInLoop::visit_expr_for_loop`.
    if name.starts_with("__copper_ctr") {
        return true;
    }
    // Read pipeline stage: `<mem>_rd<N>_q<K>` / `_v<K>`.
    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if let Some(rest) = trimmed.strip_suffix("_q").or_else(|| trimmed.strip_suffix("_v")) {
        if trimmed.len() < name.len() && has_port_index(rest, "_rd") {
            return true;
        }
    }
    // Write pipeline stage: `<mem>_wr<N>_s<K>_v` / `_addr` / `_data`.
    for suffix in ["_v", "_addr", "_data"] {
        let Some(rest) = name.strip_suffix(suffix) else { continue };
        let rest = rest.trim_end_matches(|c: char| c.is_ascii_digit());
        if let Some(head) = rest.strip_suffix("_s") {
            if has_port_index(head, "_wr") {
                return true;
            }
        }
    }
    false
}

/// `name` ends in `<marker><digits>` with something before it — i.e. it carries a
/// memory port index, which is what makes a synthesized net name recognizable.
fn has_port_index(name: &str, marker: &str) -> bool {
    match name.rfind(marker) {
        Some(i) if i > 0 => {
            let digits = &name[i + marker.len()..];
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
        }
        _ => false,
    }
}

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

/// Modules that exist to DEMONSTRATE a reconciliation failure, by name.
///
/// Not a category filter — the header above records that excluding a *category*
/// (synchronizers) once hid a real bug, and that filter is lifted permanently. A
/// named witness is the opposite thing: the divergence is the module's whole
/// purpose, it is stated here with its mechanism, and the entry disappears when
/// the mechanism is fixed. Same disposition as `probe` in `build.rs`'s SKIP table.
const WITNESSES: &[(&str, &str)] = &[];

/// If the module declares `let mut <name> = X::…(…)` or `let mut <name> = X { … }`
/// at fn top level and the file defines `struct X`, return X's field names — the
/// flattening a struct-typed register gets.
fn struct_register_fields(f: &ItemFn, file: &syn::File, name: &str) -> Option<Vec<String>> {
    let struct_name = f.block.stmts.iter().find_map(|stmt| {
        let syn::Stmt::Local(l) = stmt else { return None };
        let syn::Pat::Ident(pi) = &l.pat else { return None };
        if pi.ident != name || pi.mutability.is_none() {
            return None;
        }
        match &*l.init.as_ref()?.expr {
            // `X::ctor(…)` — the type is the path prefix.
            syn::Expr::Call(c) => {
                let syn::Expr::Path(p) = &*c.func else { return None };
                let segs: Vec<_> = p.path.segments.iter().collect();
                (segs.len() >= 2).then(|| segs[segs.len() - 2].ident.to_string())
            }
            syn::Expr::Struct(st) => {
                st.path.segments.last().map(|seg| seg.ident.to_string())
            }
            _ => None,
        }
    })?;
    file.items.iter().find_map(|i| {
        let syn::Item::Struct(st) = i else { return None };
        if st.ident != struct_name {
            return None;
        }
        Some(
            st.fields
                .iter()
                .filter_map(|fl| fl.ident.as_ref().map(|id| id.to_string()))
                .collect(),
        )
    })
}

/// Names the emitted SV declares as UNPACKED ARRAYS (`logic [..] <name> [0:..];`)
/// — an ARRAY REGISTER lowered through the memory machinery. It IS registered
/// storage (one flop per word, committed in `always_ff`), but the flop
/// extractor deliberately filters memory-array writes, so the inferred name is
/// satisfied here instead.
fn sv_array_nets(sv: &str) -> BTreeSet<String> {
    sv.lines()
        .filter_map(|l| {
            let t = l.trim().strip_suffix(';')?;
            let mut parts = t.split_whitespace();
            if parts.next()? != "logic" {
                return None;
            }
            let rest: Vec<&str> = parts.collect();
            // `[w:0] name [0:d]` — the trailing token is the unpacked dimension.
            if rest.len() >= 2 && rest.last()?.starts_with('[') {
                let name = rest[rest.len() - 2];
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    .then(|| name.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Every net the emitted SV declares (`logic … <name>;`), flop or not. Minus the
/// flop set, this is the comb-wire set the absorption allowance checks against.
fn sv_declared_nets(sv: &str) -> BTreeSet<String> {
    sv.lines()
        .filter_map(|l| {
            let t = l.trim();
            let t = t.strip_prefix("logic")?;
            let t = t.strip_suffix(';')?;
            // skip the width part, keep the last identifier
            let name = t.split_whitespace().last()?;
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
                .then(|| name.to_string())
        })
        .collect()
}

/// A SINGLE-PHASE emission — one always_comb settle per cycle — is where the
/// absorption allowance applies: a single-tick body, or a control-EXTRACTED one
/// (branch-nested ticks collapse into a one-phase `match pc` FSM; the extracted
/// regfile fixtures are the instances). A multi-tick LINEAR module emits a
/// `phase_r` counter, and there a tick-crossing wire must be PROMOTED — that
/// regression still fails here.
fn is_single_phase(sv: &str) -> bool {
    !sv.contains("phase_r")
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

            // Struct-typed registers: an inferred name is satisfied by its
            // complete flattened field set, and those flops are accounted for.
            let mut struct_fields_of: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for n in &inferred {
                if let Some(fields) = struct_register_fields(f, &file, n) {
                    struct_fields_of.insert(n.clone(), fields);
                }
            }
            let accounted: BTreeSet<String> = struct_fields_of
                .iter()
                .flat_map(|(n, fields)| fields.iter().map(move |fl| format!("{n}_{fl}")))
                .collect();
            // Single-tick absorption — see the header.
            let comb: BTreeSet<String> = if is_single_phase(&sv) {
                sv_declared_nets(&sv).difference(&codegen).cloned().collect()
            } else {
                BTreeSet::new()
            };

            let arrays = sv_array_nets(&sv);
            let missing: Vec<_> = inferred
                .difference(&codegen)
                .filter(|n| {
                    if arrays.contains(*n) {
                        return false;
                    }
                    if let Some(fields) = struct_fields_of.get(*n) {
                        if fields.iter().all(|fl| codegen.contains(&format!("{n}_{fl}"))) {
                            return false;
                        }
                    }
                    if comb.contains(*n)
                        || comb.iter().any(|c| c.ends_with(&format!("_{n}")))
                    {
                        return false;
                    }
                    true
                })
                .cloned()
                .collect();
            let extra: Vec<_> = codegen
                .difference(&inferred)
                .filter(|r| !is_synthetic(r) && !accounted.contains(*r))
                .cloned()
                .collect();

            let where_ = format!("{}::{name}", path.file_name().unwrap().to_string_lossy());
            if let Some((_, why)) = WITNESSES.iter().find(|(n, _)| *n == name) {
                // Assert the witness still diverges. A witness that quietly started
                // agreeing would sit here forever claiming a bug that was fixed —
                // the stale-`#[ignore]` failure this repo keeps recording.
                assert!(
                    !missing.is_empty() || !extra.is_empty(),
                    "{where_} is listed as a reconciliation witness but now AGREES. \
                     If that is the fix, delete its WITNESSES entry. Reason on file: {why}"
                );
                continue;
            }
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
