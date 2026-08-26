//! Generates one differential equivalence test per `#[hardware]` module in
//! `tests/fixtures/` **and** `examples/` — phases 2 and 3 of
//! `design_docs/CORPUS_DIFFERENTIAL_SWEEP.md`.
//!
//! Each generated case runs the module in the simulator under seeded random
//! stimulus and Verilates the SystemVerilog it transpiles to against that trace,
//! cycle by cycle. No reference model is involved: the simulator and the emitted SV
//! are two independent implementations of one source, so comparing them is already
//! an oracle. That is what makes the case *generatable*, and generatable is what
//! makes the coverage complete instead of "whatever anybody got round to".
//!
//! The output is `$OUT_DIR/corpus_generated.rs`, `include!`d by
//! `tests/corpus_generated.rs`. **A skipped module still gets a test**, marked
//! `#[ignore]` with its reason, because a check that silently does not run is this
//! repo's most-repeated bug class (see the G-A/G-B/G-C guards in
//! `tools/regression.sh`) and a sweep is exactly the wrong place to reintroduce it.
//!
//! # Why a build script and not a proc macro
//!
//! A proc macro reading files off disk has no rebuild tracking: edit a fixture and
//! its generated test silently stays stale. `cargo:rerun-if-changed` below is the
//! whole reason this is a build script.
//!
//! # Two kinds of file
//!
//! A **fixture** declares no imports and no clock domains — deliberately, so the
//! including test picks them — so the wrapper supplies both. An **example** is a
//! standalone program and brings its own, so the wrapper supplies neither and would
//! collide if it did. The predicate is the file's own content (`use` items,
//! `impl ClockDomain for`), not which directory it came from.
//!
//! Everything the generated *body* needs is imported under `__`-prefixed aliases, so
//! it cannot collide with whatever the included file already imports.
//!
//! # Port names
//!
//! The generated testbench addresses the Verilated model by the **emitted** name, and
//! a name colliding with a SystemVerilog or C++ keyword is legalized (`event` →
//! `event_sig`). The generated code calls `copper_codegen::legalized_port_name` at
//! run time rather than reimplementing that rule — two copies of a naming rule that
//! must agree is the drift bug this repo keeps recording.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use quote::ToTokens;

/// Modules the sweep must not run, each with the reason it cannot. Kept here, in
/// version control and in review, rather than inferred: "this one is expected to
/// fail" is a claim that should have an author and a sentence.
const SKIP: &[(&str, &str)] = &[
    (
        "mp_rom",
        "refused by design: a plain `Out` driven from a memory read result across phases has no \
         correct emitted form (measured a full cycle late). multiphase_memory_equivalence.rs pins \
         the diagnostic",
    ),
    (
        "tick_first_waiter",
        "refused by design: a repeating wait must be written test-first; pinned as an unsupported \
         construct in unsupported_constructs.rs",
    ),
    (
        "probe",
        "an `allow_pretick_alignment` witness: the module exists to DEMONSTRATE the pre-tick \
         divergence, so sim ≠ SV is its purpose. probe_timing_investigation.rs measures it",
    ),
    (
        "probe_fsm",
        "a pinned divergence (probe_fsm_sim_matches_verilog is #[ignore]d for it): a phase-gated \
         cross-tick read still disagrees with the transpiler",
    ),
    (
        "rv32i_cpu",
        "does not transpile: cause F, a `Vec<Bits<32>>` port (TODO, TRANSPILER COVERAGE)",
    ),
    (
        "rv32i_cpu_pipelined",
        "does not transpile: cause F, a `Vec<Bits<32>>` port (TODO, TRANSPILER COVERAGE)",
    ),
    (
        "uart_tx",
        "does not transpile: cause H — `spawn_uart` in the same file has a hardware-looking \
         signature with no `#[hardware]`, which `prepare_source` refuses at FILE level before \
         either module is looked at",
    ),
    (
        "uart_rx",
        "does not transpile: cause H, the same file-level `spawn_uart` rejection as `uart_tx`",
    ),
    (
        "ex_combinational_ripple_carry_adder::ripple_carry_adder",
        "does not transpile: cause J-b, a tuple-returning helper (`let (s, c) = full_adder(…)`), \
         reported as a width error. Pinned by transpile_inference_gaps.rs. The fixture copy is \
         written without the helper and does sweep",
    ),
    (
        "trailing_constant",
        "STARTUP TRANSIENT, not a divergence in steady state: `dv` is written only in \
         the trailing segment, so the simulator first drives it at cycle 1 (it holds \
         its initial value until the statement runs) while the emitted `assign dv = \
         1'b1` drives it from time 0. They agree from cycle 1 on. A continuous assign \
         has no notion of \"not yet written\", so this is a property of an `Out` first \
         driven late, not of the trailing-statement semantics — see \
         design_docs/SYNCHRONOUS_SEMANTICS.md",
    ),
    (
        "two_domain_top",
        "a `#[hardware(structural)]` parent: transpile-only by design, with no simulatable body \
         to drive (item 4 — the sim wires the hierarchy by hand)",
    ),
    (
        "branch_merge_explicit",
        "a demonstration of the pre-tick hazard since 2026-08-25: its conditionally-written \
         constant `tail_o` leads the identical emitted SV by one cycle (pinned as \
         sequential_forwarding_divergence.rs::a_write_in_a_state_arm_leads_the_hardware_by_one_cycle, \
         guardrail 5.5). The D1 rule now REJECTS this shape; the module keeps \
         `allow_pretick_alignment` because it exists to demonstrate it, so it still diverges and \
         cannot be swept",
    ),
];

/// The monomorphization to sweep a **generic** module at, in signature order.
///
/// A generic module transpiles to a *parametric* SystemVerilog module, so it can be
/// swept — Verilated with `-G` at the same widths the simulator runs, exactly as the
/// hand-written tests do. What cannot be inferred is *which* widths: the parameters
/// are often constrained (`N_LOG == clog2(N)`, asserted in the module itself), so a
/// guess is a compile error at best. The values here are the ones the corresponding
/// hand-written equivalence tests use, so the sweep and the vector test exercise the
/// same shape.
///
/// A generic module missing from this table is ignored with a reason that says so.
const PARAMS: &[(&str, &[(&str, i64)])] = &[
    ("mux", &[("WIDTH_P", 8), ("ELS_P", 4), ("LG_ELS_LP", 2)]),
    ("priority_encode", &[("N", 8), ("N_LOG", 3)]),
    ("ripple_carry_adder", &[("N", 8)]),
    ("rotate_right", &[("N", 8), ("N_LOG", 3)]),
    ("shift_register", &[("N", 8), ("N_1", 7)]),
    ("wide_alu", &[("N", 32)]),
];

/// Modules whose state is **undefined until reset**, and the port that resets them:
/// `(module, port, active_low)`. Cycle 0 drives the port to its asserted value; every
/// cycle after that is random, so the reset itself keeps getting exercised.
///
/// This is not a workaround. `shift_register` initialises its register to `Bits::x()`
/// — deliberately, because an unreset flip-flop is X in hardware — and the two
/// implementations then legitimately DISAGREE about what X reads as (the simulator
/// carries X, Verilator's 2-state model reads 0). Sweeping the pre-reset window
/// compares undefined behaviour against undefined behaviour, which is a test of
/// nothing. A design that needs a reset gets one.
const RESET: &[(&str, &str, bool)] = &[("shift_register", "rstn", true)];

/// Cycles of random stimulus per sequential module.
const SEQ_CYCLES: usize = 200;
/// Random input vectors per combinational module (no clock, so `poll_tasks`).
const COMB_VECTORS: usize = 64;

fn main() {
    println!("cargo:rerun-if-changed=tests/fixtures");
    println!("cargo:rerun-if-changed=examples");
    println!("cargo:rerun-if-changed=build.rs");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));

    let mut files: Vec<PathBuf> = Vec::new();
    collect_rs(&manifest.join("tests/fixtures"), &mut files);
    collect_rs(&manifest.join("examples"), &mut files);
    files.sort();

    let mut code = String::from(
        "// @generated by build.rs — phase 2 of design_docs/CORPUS_DIFFERENTIAL_SWEEP.md.\n\
         // Edit build.rs, not this file.\n\n",
    );
    let (mut emitted, mut ignored) = (0usize, 0usize);
    let mut covered: Vec<String> = Vec::new();

    for path in &files {
        let src = std::fs::read_to_string(path).expect("fixture is readable");
        let file = match syn::parse_file(&src) {
            Ok(f) => f,
            // A fixture that does not parse standalone is not this script's problem
            // to report — the test that includes it will say so far more clearly.
            Err(_) => continue,
        };
        let modules: Vec<Module> = file.items.iter().filter_map(Module::from_item).collect();
        if modules.is_empty() {
            continue;
        }
        let key = wrapper_name(&manifest, path);
        let (m, i) = emit_file(&mut code, path, &src, &modules, &key);
        emitted += m;
        ignored += i;
        covered.extend(modules.iter().map(|m| format!("{key}::{}", m.name)));
    }

    // The manifest the coverage guard checks against: every module this script
    // emitted a case for, ignored ones included. A generator that quietly stops
    // covering something is the failure mode this whole file exists to prevent, so
    // it is asserted in the test binary rather than trusted.
    let _ = writeln!(
        code,
        "/// Every fixture module `build.rs` emitted a case for (ignored ones too).\n\
         pub const COVERED: &[&str] = &[{}];",
        covered.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ")
    );

    // Surfaced in the build log too, so the counts are visible without running.
    println!("cargo:warning=corpus sweep: {emitted} generated, {ignored} ignored-with-reason");
    std::fs::write(out_dir.join("corpus_generated.rs"), code).expect("write generated tests");
}

/// Every `.rs` under `dir`, recursively (`examples/` has subdirectories).
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        let p = entry.path();
        if p.is_dir() {
            collect_rs(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// A unique Rust identifier for one source file's wrapper module, from its path
/// relative to the manifest — `examples/cdc/flag_crossing.rs` → `ex_cdc_flag_crossing`.
/// Path-derived rather than stem-derived because two files legitimately share a stem,
/// and because two different modules can share a NAME (`fast_counter` exists in both
/// `two_domain_counter.rs` and `two_domain_hierarchy.rs`).
fn wrapper_name(manifest: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(manifest).unwrap_or(path).with_extension("");
    let mut s: String = rel
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    s = s
        .trim_start_matches("tests_fixtures_")
        .trim_start_matches("examples_")
        .to_string();
    let prefix = if path.components().any(|c| c.as_os_str() == "examples") { "ex_" } else { "fx_" };
    format!("{prefix}{s}")
}

/// One `#[hardware]` module, as much of it as generation needs.
struct Module {
    name: String,
    /// Const-generic parameter names in signature order, empty for a concrete module.
    /// A generic module emits a PARAMETRIC SystemVerilog module, so it can be swept —
    /// at the widths `PARAMS` records for it.
    generics: Vec<String>,
    ports: Vec<Port>,
}

struct Port {
    name: String,
    /// The payload type as written (`Bits<8>`, `Logic`), or the domain for a clock.
    ty: String,
    domain: String,
    kind: Kind,
}

#[derive(PartialEq)]
enum Kind {
    Clock,
    In,
    Out,
    RegOut,
}

impl Module {
    fn from_item(item: &syn::Item) -> Option<Module> {
        let syn::Item::Fn(f) = item else { return None };
        if !f.attrs.iter().any(|a| {
            a.path().segments.last().is_some_and(|s| s.ident == "hardware")
        }) {
            return None;
        }
        let mut ports = Vec::new();
        for arg in &f.sig.inputs {
            let syn::FnArg::Typed(pt) = arg else { return None };
            let name = pt.pat.to_token_stream().to_string();
            let (kind, ty, domain) = classify(&pt.ty)?;
            ports.push(Port { name, ty, domain, kind });
        }
        let generics = f
            .sig
            .generics
            .params
            .iter()
            .filter_map(|p| match p {
                syn::GenericParam::Const(c) => Some(c.ident.to_string()),
                _ => None,
            })
            .collect();
        Some(Module { name: f.sig.ident.to_string(), generics, ports })
    }

    fn clock(&self) -> Option<&Port> {
        self.ports.iter().find(|p| p.kind == Kind::Clock)
    }

    /// The recorded monomorphization, if this module needs one and has one. Every
    /// declared parameter must be covered — a partial entry would monomorphize to
    /// something the author did not choose.
    fn params(&self) -> Option<&'static [(&'static str, i64)]> {
        if self.generics.is_empty() {
            return None;
        }
        let entry = PARAMS.iter().find(|(n, _)| *n == self.name)?.1;
        self.generics
            .iter()
            .all(|g| entry.iter().any(|(n, _)| n == g))
            .then_some(entry)
    }

    /// The reason this module cannot be swept, if any.
    fn skip_reason(&self, qualified: &str) -> Option<String> {
        // Matched on the bare name, or on `<wrapper>::<name>` when only one COPY of a
        // module is affected — `ripple_carry_adder` exists twice and only the example
        // copy uses the construct that refuses.
        if let Some((_, why)) = SKIP
            .iter()
            .find(|(n, _)| *n == self.name || n.rsplit("::").next() == Some(self.name.as_str()) && *n == qualified)
        {
            return Some((*why).to_string());
        }
        if !self.generics.is_empty() && self.params().is_none() {
            return Some(format!(
                "generic module with no monomorphization recorded: add `(\"{}\", &[{}])` to \
                 build.rs's PARAMS table. The parameters are usually constrained (a module can \
                 `const assert!` a relation between them), so the widths are a decision, not \
                 something to infer",
                self.name,
                self.generics
                    .iter()
                    .map(|g| format!("(\"{g}\", ?)"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if self.ports.iter().filter(|p| p.kind == Kind::Clock).count() > 1 {
            return Some(
                "more than one clock port: the generated harness drives a single clock, and the \
                 ratio between two domains is a design decision the sweep must not invent — \
                 phase 4"
                    .to_string(),
            );
        }
        if self.ports.iter().any(|p| p.kind != Kind::Clock && !is_stimulable(&p.ty)) {
            return Some(format!(
                "unsupported port payload: the generator can only randomise `Logic` and \
                 `Bits<N>` ({})",
                self.ports
                    .iter()
                    .filter(|p| p.kind != Kind::Clock && !is_stimulable(&p.ty))
                    .map(|p| format!("{}: {}", p.name, p.ty))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        None
    }
}

/// `In<Bits<8>, MainClk>` → `(Kind::In, "Bits<8>", "MainClk")`; `Clock<D>` → the domain.
fn classify(ty: &syn::Type) -> Option<(Kind, String, String)> {
    let syn::Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    let kind = match seg.ident.to_string().as_str() {
        "Clock" => Kind::Clock,
        "In" => Kind::In,
        "Out" => Kind::Out,
        "RegOut" => Kind::RegOut,
        _ => return None,
    };
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else { return None };
    let text: Vec<String> = args
        .args
        .iter()
        .map(|a| a.to_token_stream().to_string().replace(' ', ""))
        .collect();
    match kind {
        Kind::Clock => Some((kind, text.first()?.clone(), text.first()?.clone())),
        _ => Some((kind, text.first()?.clone(), text.get(1)?.clone())),
    }
}

/// Can seeded random stimulus be generated for this payload? `RandStim` covers
/// `Logic`, `Bits<N>` for any `N` — including a width written as a file-scope `const`
/// or a const-generic parameter, both of which are in scope in the generated body —
/// and arrays of either.
fn is_stimulable(ty: &str) -> bool {
    if ty == "Logic" {
        return true;
    }
    if let Some(inner) = ty.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        // `[Bits<W>;N]` — the element must be stimulable; the length is a width-like
        // identifier or a literal, and is checked by the compiler, not here.
        return inner.rsplit_once(';').is_some_and(|(elem, _)| is_stimulable(elem.trim()));
    }
    ty.strip_prefix("Bits<")
        .and_then(|r| r.strip_suffix('>'))
        .is_some_and(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// A zero of the payload type, for the wire's initial value.
fn zero_of(ty: &str) -> String {
    if ty == "Logic" {
        "__Logic::Zero".to_string()
    } else if ty.starts_with('[') {
        // `[T; N]`: `from_fn` rather than `[zero; N]`, which would need `Copy` in a
        // const context the element type does not necessarily satisfy.
        let elem = ty[1..ty.len() - 1].rsplit_once(';').map(|(e, _)| e.trim()).unwrap_or("Logic");
        format!("std::array::from_fn(|_| {})", zero_of(elem))
    } else {
        "__Bits::zero()".to_string()
    }
}

/// A stable per-module seed, so two modules do not walk the same bit pattern and a
/// failure is reproducible from the name alone (FNV-1a).
fn seed_of(name: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h | 1
}

/// Emit the wrapper module for one source file plus a test per hardware module.
/// Returns `(generated, ignored)`.
fn emit_file(
    code: &mut String,
    path: &Path,
    src: &str,
    modules: &[Module],
    wrapper: &str,
) -> (usize, usize) {
    let file_path = path.to_string_lossy();

    // An `include!` cannot produce inner doc comments (`//!` may not follow an item,
    // and a macro expansion cannot emit one at all), so such a file will not compile
    // included. Every fixture and every example uses `//` headers today; if one
    // regresses, say so in the ignore reason rather than emitting a case that breaks
    // the build.
    let inner_doc = src.trim_start().starts_with("//!");

    // A file that brings its own imports and clock domains is an EXAMPLE — a
    // standalone program. Supplying them again would collide. A fixture brings
    // neither, deliberately, so the wrapper supplies both. The predicate is content,
    // not directory.
    let has_own_imports = src.lines().any(|l| l.trim_start().starts_with("use "));
    // Spelled three ways in the corpus — `impl ClockDomain for X {}`, the same with
    // no space before the braces, and `impl copper_core::ClockDomain for X {}` — so
    // match on the phrase and take the identifier that follows, rather than on a
    // prefix.
    let declared_domains: BTreeSet<String> = src
        .lines()
        .filter_map(|l| l.split_once("ClockDomain for "))
        .map(|(_, rest)| {
            rest.trim()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<String>()
        })
        .filter(|d| !d.is_empty())
        .collect();

    let needed_domains: BTreeSet<&str> = modules
        .iter()
        .flat_map(|m| m.ports.iter())
        .map(|p| p.domain.as_str())
        .filter(|d| *d != "()" && !declared_domains.contains(*d))
        .collect();

    let _ = writeln!(code, "#[allow(unused_imports, dead_code)]");
    let _ = writeln!(code, "mod {wrapper} {{");
    // Aliased, so nothing here can clash with what the included file imports.
    let _ = writeln!(
        code,
        "    use copper_core::port::{{registered_wire as __rw, wire as __w}};\n    \
         use copper_core::types::Bits as __Bits;\n    \
         use copper_core::{{Clock as __Clock, Logic as __Logic}};\n    \
         use copper_sim::HardwareExecutor as __Exec;\n    \
         use crate::common::{{EquivalenceTest as __Eq, RandStim as __RandStim, Rng as __Rng}};"
    );
    if !has_own_imports {
        // A fixture: it needs the hardware vocabulary to compile at all.
        let _ = writeln!(
            code,
            "    use copper_core::port::{{registered_wire, wire, In, Out, RegOut}};\n    \
             use copper_core::types::Bits;\n    \
             use copper_core::{{Clock, ClockDomain, Logic, Memory}};\n    \
             use copper_macros::hardware;\n    \
             use copper_sim::HardwareExecutor;"
        );
    }
    for d in &needed_domains {
        // Fully qualified: a file that needs a domain declared is one that did not
        // import `ClockDomain` either.
        let _ = writeln!(
            code,
            "    struct {d};\n    impl copper_core::ClockDomain for {d} {{}}"
        );
    }
    if !inner_doc {
        let _ = writeln!(code, "    include!(\"{file_path}\");");
    }
    let _ = writeln!(code, "    const SRC: &str = include_str!(\"{file_path}\");\n");

    let (mut emitted, mut ignored) = (0, 0);
    for m in modules {
        let reason = if inner_doc {
            Some(
                "the file opens with `//!` module docs, which `include!` cannot carry — an inner \
                 doc comment may not follow an item. Give it a `//` header, as every other \
                 fixture and example has"
                    .to_string(),
            )
        } else {
            m.skip_reason(&format!("{wrapper}::{}", m.name))
        };
        match reason {
            Some(reason) => {
                let _ = writeln!(code, "    #[ignore = \"{}\"]", escape(&reason));
                emit_test(code, m, /*body=*/ false);
                ignored += 1;
            }
            None => {
                emit_test(code, m, true);
                emitted += 1;
            }
        }
    }
    let _ = writeln!(code, "}}\n");
    (emitted, ignored)
}

/// The test itself. With `body == false` the case is `#[ignore]`d and the body is a
/// single `panic!` — it must still *exist* (so the skip is listed and counted), and
/// it must not be silently green if someone removes the `#[ignore]`.
fn emit_test(code: &mut String, m: &Module, body: bool) {
    let name = &m.name;
    let _ = writeln!(code, "    #[test]");
    let _ = writeln!(code, "    fn {name}_differential() {{");
    if !body {
        let _ = writeln!(
            code,
            "        panic!(\"skipped by build.rs's SKIP table — see the #[ignore] reason\");"
        );
        let _ = writeln!(code, "    }}\n");
        return;
    }

    let clk = m.clock();
    let params = m.params();

    // The monomorphization, as local bindings the port types and the call both use.
    for (n, v) in params.unwrap_or(&[]) {
        let _ = writeln!(code, "        const P_{n}: usize = {v};");
    }

    let with_params = match params {
        Some(ps) => format!(
            "\n            .with_params(&[{}])",
            ps.iter().map(|(n, v)| format!("(\"{n}\", {v})")).collect::<Vec<_>>().join(", ")
        ),
        None => String::new(),
    };
    let _ = writeln!(
        code,
        "        let mut eq = __Eq::differential_only(\"{name}\", SRC, Some(\"{name}\")){with_params};"
    );
    let _ = writeln!(code, "        let mut rng = __Rng::new({});", seed_of(name));
    if let Some(c) = clk {
        let _ = writeln!(code, "        let mut clk = __Clock::<{}>::new();", c.domain);
    }
    let _ = writeln!(code, "        let mut exec = __Exec::new();");

    for p in &m.ports {
        let (ty, z, d) = (alias_ty(&p.ty, &m.generics), zero_of(&p.ty), &p.domain);
        match p.kind {
            Kind::Clock => {}
            Kind::In => {
                let _ = writeln!(
                    code,
                    "        let ({n}_drv, {n}_in) = __w::<{ty}, {d}>({z});",
                    n = p.name
                );
            }
            Kind::Out => {
                let _ = writeln!(
                    code,
                    "        let ({n}_out, {n}_obs) = __w::<{ty}, {d}>({z});",
                    n = p.name
                );
            }
            Kind::RegOut => {
                let _ = writeln!(
                    code,
                    "        let ({n}_out, {n}_obs) = __rw::<{ty}, {d}>(&clk, {z});",
                    n = p.name
                );
            }
        }
    }

    // The names the testbench will address the Verilated model by — the EMITTED
    // ones. Resolved by the transpiler's own rule at run time, not guessed here.
    for p in m.ports.iter().filter(|p| p.kind != Kind::Clock) {
        let _ = writeln!(
            code,
            "        let {n}_sv = copper_codegen::legalized_port_name(\"{n}\");",
            n = p.name
        );
    }

    let handles: Vec<String> = m
        .ports
        .iter()
        .filter(|p| matches!(p.kind, Kind::Out | Kind::RegOut))
        .map(|p| format!("{}_out.dirty_handle()", p.name))
        .collect();
    let reads: Vec<String> = m
        .ports
        .iter()
        .filter(|p| p.kind == Kind::In)
        .map(|p| format!("{}_in.wire_id()", p.name))
        .collect();
    let _ = writeln!(code, "        let handles = vec![{}];", handles.join(", "));
    let _ = writeln!(code, "        let reads = vec![{}];", reads.join(", "));

    // Arguments in SIGNATURE order — not inputs-then-outputs, which is merely the
    // usual convention and not a rule any module is obliged to follow.
    let args: Vec<String> = m
        .ports
        .iter()
        .map(|p| match p.kind {
            Kind::Clock => "clk.clone()".to_string(),
            Kind::In => format!("{}_in", p.name),
            Kind::Out | Kind::RegOut => format!("{}_out", p.name),
        })
        .collect();
    let turbofish = match params {
        Some(ps) if !m.generics.is_empty() => format!(
            "::<{}>",
            m.generics
                .iter()
                .map(|g| {
                    let _ = ps;
                    format!("P_{g}")
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => String::new(),
    };
    let _ = writeln!(
        code,
        "        exec.spawn_wired({name}{turbofish}({}), handles, reads);\n",
        args.join(", ")
    );

    let cycles = if clk.is_some() { SEQ_CYCLES } else { COMB_VECTORS };
    let reset = RESET.iter().find(|(n, _, _)| *n == m.name);
    let _ = writeln!(code, "        for __cycle in 0..{cycles} {{");
    for p in m.ports.iter().filter(|p| p.kind == Kind::In) {
        let ty = alias_ty(&p.ty, &m.generics);
        match reset {
            // Cycle 0 asserts the reset; after that it is random like everything else,
            // so the reset path keeps being exercised rather than being visited once.
            Some((_, port, active_low)) if *port == p.name => {
                let asserted = if *active_low { "__Logic::Zero" } else { "__Logic::One" };
                let _ = writeln!(
                    code,
                    "            let {n}_v = if __cycle == 0 {{ {asserted} }} else {{ \
                     <{ty} as __RandStim>::rand(&mut rng) }};\n            {n}_drv.write({n}_v);",
                    n = p.name
                );
            }
            _ => {
                let _ = writeln!(
                    code,
                    "            let {n}_v = <{ty} as __RandStim>::rand(&mut rng);\n            \
                     {n}_drv.write({n}_v);",
                    n = p.name
                );
            }
        }
    }
    let _ = writeln!(
        code,
        "            {}",
        if clk.is_some() { "exec.tick_clock(&mut clk);" } else { "exec.poll_tasks();" }
    );
    for p in m.ports.iter().filter(|p| matches!(p.kind, Kind::Out | Kind::RegOut)) {
        let _ = writeln!(code, "            let {n}_v = {n}_obs.read();", n = p.name);
    }
    let ins: Vec<String> = m
        .ports
        .iter()
        .filter(|p| p.kind == Kind::In)
        .map(|p| format!("(&{n}_sv[..], &{n}_v.as_bits()[..])", n = p.name))
        .collect();
    let outs: Vec<String> = m
        .ports
        .iter()
        .filter(|p| matches!(p.kind, Kind::Out | Kind::RegOut))
        .map(|p| format!("(&{n}_sv[..], &{n}_v.as_bits()[..])", n = p.name))
        .collect();
    let _ = writeln!(
        code,
        "            eq.record_differential(&[{}], &[{}]);",
        ins.join(", "),
        outs.join(", ")
    );
    let _ = writeln!(code, "        }}");
    let _ = writeln!(code, "        eq.finish();");
    let _ = writeln!(code, "    }}\n");
}

/// The payload type spelled for the generated body: `Bits` and `Logic` under the
/// wrapper's aliases (so nothing depends on what the included file imported), and any
/// const-generic parameter under its local `P_` binding.
///
/// Token-wise, not by string replacement: `Bits<N_LOG>` must not be rewritten by a
/// rule for `N`, and a substring replace does exactly that.
fn alias_ty(ty: &str, generics: &[String]) -> String {
    let mut out = String::new();
    let mut ident = String::new();
    let flush = |ident: &mut String, out: &mut String| {
        if ident.is_empty() {
            return;
        }
        let mapped = match ident.as_str() {
            "Bits" => "__Bits".to_string(),
            "Logic" => "__Logic".to_string(),
            other if generics.iter().any(|g| g == other) => format!("P_{other}"),
            other => other.to_string(),
        };
        out.push_str(&mapped);
        ident.clear();
    };
    for c in ty.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            ident.push(c);
        } else {
            flush(&mut ident, &mut out);
            out.push(c);
        }
    }
    flush(&mut ident, &mut out);
    out
}

/// Escape a reason string for a `#[ignore = "…"]` attribute.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
