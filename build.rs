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
        "receives its unified instruction/data `Memory` as a parameter, so the sweep cannot \
         supply one (see the Kind::Memory rule below, which would skip it anyway). It also does \
         not transpile — a received memory has no port ABI, and the struct-typed pipeline latches \
         and tuple-returning EX stage are their own blockers (TODO, TRANSPILER COVERAGE)",
    ),
    // ── The claim ledger: tests/fixtures/{bits_ops,signedness,aggregate_locals,
    // match_selector,out_phase,mem_address_width}_dut.rs. Each module below is a
    // KNOWN-WRONG lowering with a working twin in the same file that DOES sweep, so
    // a fix shows up as an entry that can be deleted. The failure mode named in each
    // reason is the one actually observed, not the one predicted.
    (
        "bit_not_bits",
        "WRONG LOWERING, AND THE TRANSPILER'S: `!` on a `Bits<N>` emits SystemVerilog `!` \
         (LOGICAL negation) rather than `~`, so the result collapses to one bit — Verilator \
         reports WIDTHTRUNC on the LOGNOT. LOCALISED by an independent reference (`assign o = \
         ~a`, tests/fixtures/reference_sv/bit_not_bits.sv, wired up in REFERENCE above): it \
         AGREES with the simulator and disagrees with the emitted SV, so the simulator is \
         right and the lowering is wrong. `bit_not_via_xor` is the working spelling and \
         sweeps; `bit_not_bool` pins that `!` on a bool is correct, so a fix must not make \
         `!` bitwise everywhere. Delete this entry when the lowering emits `~`; the anchor \
         is already in place",
    ),
    (
        "lit_width_in_ternary",
        "WRONG WIDTH: a `Bits::<32>::from_lit` with no sibling operand to take its width \
         from emits a 64-bit literal — WIDTHTRUNC into the 32-bit port. \
         `lit_width_via_locals` is the same value through explicitly-typed `let` bindings \
         and sweeps",
    ),
    (
        "signed_lt_via_cast",
        "WRONG ANSWER, AND IT LINTS CLEAN: `ExprType::Cast` is stripped and `signed` is \
         never emitted, so `(a as i32) < (b as i32)` becomes an UNSIGNED compare. The sweep \
         disagrees from cycle 0. This is RISC-V's SLT/BLT/BGE. `signed_lt_via_bias` is the \
         working spelling and sweeps",
    ),
    (
        "sign_extend_via_cast",
        "WRONG ANSWER, AND IT LINTS CLEAN: `as i32 >> 20` is arithmetic in Rust and logical \
         in the emitted SV, so sign extension becomes zero extension (measured: expected \
         4294967170, got 3970). `sign_extend_via_mask` is the working spelling and sweeps",
    ),
    (
        "match_on_usize",
        "WRONG WIDTH: a `match` emits 64-bit case labels (`s == 64'd1`) — WIDTHEXPAND against \
         the 32-bit selector. `ifchain_on_usize` is the same mux and demux as an if-chain and \
         sweeps. See `match_on_literals`: the width comes from the LITERAL, not the scrutinee",
    ),
    (
        "match_on_literals",
        "WRONG WIDTH, and this one was WRITTEN AS A CONTROL EXPECTED TO PASS: a `match` on a \
         `u32` with literal patterns also emits `op == 64'd55`. That is what narrowed the \
         claim from \"a match on a usize\" to \"a match literal\" — a probe that only asked \
         \"does it transpile\" had already recorded this shape as working",
    ),
    (
        "match_on_const_pattern",
        "REFUSED: a named `const` in a match PATTERN reads as an enum-variant pattern and is \
         rejected as `tuple-pattern match lowering is not yet implemented (M2)`, which is not \
         what it is. Cause D-a made file-scope consts work as EXPRESSIONS only. \
         `ifchain_on_const_expr` is the working spelling and sweeps",
    ),
    (
        "out_from_reg_before_commit",
        "ONE-CYCLE DIVERGENCE, and no existing check catches it: a plain `Out` driven from a \
         register and written BEFORE the register commits leads the simulator by a cycle. \
         D1's guard exempts the segment because an `In` read precedes the write. Found by the \
         sweep on rv32i_cpu_transpilable's `program_counter`. \
         `out_from_reg_after_commit` is the shape that agrees and sweeps",
    ),
    (
        "regout_trailing_single_tick",
        "ONE-CYCLE DIVERGENCE, and the half worth knowing: reaching for `RegOut` does NOT fix \
         the entry above. In a SINGLE-TICK loop the trailing statements share the head's \
         phase, so the transpiler folds a trailing `RegOut` write into this edge while the \
         simulator commits it on the next. Same lead, opposite cause. Every other member of \
         the pre-tick alignment family is about a body crossing two or more edges",
    ),
    (
        "wide_index_sole_consumer",
        "UNUSEDSIGNAL: `usize` is 32 bits and a memory address net is narrower, so `10'(i)` \
         reads `i[9:0]` and an index local that feeds NOTHING ELSE has a dead upper half. \
         Not the address cast being wrong — `wide_index_into_narrow_addr` is the same index \
         with a range check (which reads the whole word, as every address in \
         rv32i_cpu_transpilable does) and sweeps. The fix is a real choice — emit the index \
         local at the address width, or accept the lint — and is deliberately not guessed at",
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
/// An **independent hand-written Verilog reference** for a module, by name.
///
/// The sweep's ordinary check is simulator vs the SystemVerilog the transpiler
/// emitted: two implementations of ONE source. That is an oracle for "the two
/// agree" and nothing more — a misconception shared between the executor and the
/// lowering is invisible to it. A reference nobody derived from either closes that
/// gap, and a module listed here is checked against all three.
///
/// **Provenance is part of the claim and belongs in the file's header.** A
/// reference written by whoever wrote the Copper module catches lowering and
/// transcription errors but shares that person's model of the design; only a
/// genuinely third-party source (BaseJump STL and the like) buys independence.
/// Third-party is preferred wherever one exists — see `examples/basejump/sv/` for
/// the header format, including what was adapted and why.
///
/// The referenced file's SystemVerilog module must be NAMED FOR THE COPPER MODULE:
/// it is Verilated as the top exactly as the transpiler's output is. Vendored
/// third-party code is therefore adapted (renamed ports, concrete parameter
/// defaults), which the vendored BaseJump files already do and record.
///
/// A module NOT listed here is unanchored, and `tools/regression.sh` prints how
/// many there are on every run — the gap is tracked rather than rediscovered.
const REFERENCE: &[(&str, &str)] = &[
    ("ram_read_first", "tests/fixtures/reference_sv/ram_read_first.sv"),
    // Anchored while still SKIPped, deliberately: the row is inert until the SKIP
    // below is deleted, and it is what proved the `!` bug is the TRANSPILER's and
    // not the simulator's. Ready the moment the fix lands.
    ("bit_not_bits", "tests/fixtures/reference_sv/bit_not_bits.sv"),
];

const RESET: &[(&str, &str, bool)] = &[
    ("shift_register", "rstn", true),
    // Held low the core is idle and its boot port owns the memory's write bus, so the
    // sweep's random stimulus reaches a defined state instead of X. This is the first
    // CPU-scale design in the sweep; it got here when `vlir_lower` started casting
    // memory addresses to the address-net width (TODO cause Q).
    ("rv32i_cpu_transpilable", "rstn", true),
];

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

    // Validate the REFERENCE table before anything uses it. Every failure mode here
    // is silent otherwise: a mistyped module name anchors NOTHING and the sweep goes
    // on passing, which is this repo's signature bug in a new place. Checked at build
    // time so the mistake is reported where it was made rather than as a confusing
    // Verilator error twenty seconds later.
    for (module, path) in REFERENCE {
        let leaves: Vec<&str> =
            covered.iter().map(|n| n.rsplit("::").next().unwrap_or(n)).collect();
        if !leaves.contains(module) {
            // Near-misses only. Printing all ~130 module names buries the one thing
            // the reader needs, which is "did I typo it".
            let pre = &module[..module.len().min(4)];
            let mut near: Vec<&str> =
                leaves.iter().copied().filter(|l| l.starts_with(pre)).collect();
            near.sort_unstable();
            near.dedup();
            let hint = if near.is_empty() {
                format!("no module starts with `{pre}`; {} are covered", leaves.len())
            } else {
                format!("did you mean: {}", near.join(", "))
            };
            panic!(
                "REFERENCE names `{module}`, which is not a module the sweep covers. \
                 A typo here anchors nothing and every test still passes. {hint}"
            );
        }
        let full = manifest.join(path);
        assert!(
            full.is_file(),
            "REFERENCE for `{module}` points at `{path}`, which does not exist \
             (looked in {})",
            full.display()
        );
        let sv = std::fs::read_to_string(&full)
            .unwrap_or_else(|e| panic!("REFERENCE for `{module}`: cannot read {path}: {e}"));
        assert!(
            sv.contains(&format!("module {module}")),
            "REFERENCE for `{module}` is `{path}`, which does not declare `module {module}`. \
             Verilator is run with `--top-module {module}`, so the reference's module must be \
             NAMED FOR THE COPPER MODULE — vendored third-party code is renamed for this, which \
             the files in examples/basejump/sv/ do and record in their headers"
        );
        assert!(
            !sv.contains("// @generated"),
            "REFERENCE for `{module}` points at generated output. A reference derived from the \
             transpiler cannot anchor the transpiler"
        );
        println!("cargo:rerun-if-changed={path}");
    }

    // The anchoring ledger. A module cross-checked only against the transpiler's own
    // output is verified for CONSISTENCY; one checked against an independent
    // reference is verified for SEMANTICS. Keeping the two counts apart is the point
    // — see REFERENCE above — and G-E in tools/regression.sh prints the remainder so
    // it stays visible instead of being rediscovered by an audit.
    let anchored = covered
        .iter()
        .filter(|n| {
            let leaf = n.rsplit("::").next().unwrap_or(n);
            REFERENCE.iter().any(|(m, _)| *m == leaf)
        })
        .count();
    let _ = writeln!(
        code,
        "/// Modules checked against an INDEPENDENT hand-written Verilog reference.\n\
         pub const ANCHORED: usize = {anchored};\n\
         /// Swept modules with no independent reference — cross-checked against the\n\
         /// transpiler's own output only, so consistent but not externally anchored.\n\
         pub const UNANCHORED: usize = {};",
        covered.len() - anchored
    );

    // Surfaced in the build log too, so the counts are visible without running.
    println!("cargo:warning=corpus sweep: {emitted} generated, {ignored} ignored-with-reason");
    let unanchored_names: Vec<&str> = covered
        .iter()
        .filter(|n| {
            let leaf = n.rsplit("::").next().unwrap_or(n);
            !REFERENCE.iter().any(|(m, _)| *m == leaf)
        })
        .map(|n| n.as_str())
        .collect();
    println!(
        "cargo:warning=anchoring: {anchored} module(s) checked against an independent reference, \
         {} cross-checked against the transpiler only",
        unanchored_names.len()
    );
    // Named once the list is short enough to read. While it is the whole corpus the
    // count is the useful number; the names become the useful thing as the remainder
    // shrinks, which is the direction this is meant to move.
    if !unanchored_names.is_empty() && unanchored_names.len() <= 25 {
        println!("cargo:warning=unanchored: {}", unanchored_names.join(", "));
    }
    std::fs::write(out_dir.join("corpus_generated.rs"), code).expect("write generated tests");
}

/// Every `.rs` under `dir`, recursively (`examples/` has subdirectories).
///
/// `old/` directories are excluded: `examples/cpu/old/` is untracked scratch
/// holding pre-subset spellings (`Vec` ports the admissible grammar rejects), and
/// sweeping it generates corpus cases that cannot compile. The `TODO`'s KNOWN
/// entry for the admissible grammar verified the tracked corpus is unaffected
/// with `old/` set aside; this makes that the sweep's actual behaviour.
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        let p = entry.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "old") {
                continue;
            }
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
    /// A `Memory<T, R, W, D, RL, WL>` the module RECEIVES rather than declares.
    /// Not a port: no wire is made for it and no stimulus is generated. It exists
    /// as a variant so `classify` can return `Some` for it — see the note there.
    Memory,
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
        if self.ports.iter().any(|p| p.kind == Kind::Memory) {
            return Some(
                "receives a `Memory<…>` parameter: the sweep would have to invent its size and \
                 its contents, and both are design decisions (a ROM's contents ARE the design). \
                 Same disposition as PARAMS — a memory-taking module is swept by a hand-written \
                 test that states them, not by generated stimulus"
                    .to_string(),
            );
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
///
/// **Returning `None` here deletes a module from the sweep.** `Module::from_item`
/// propagates it with `?`, so an unclassifiable parameter does not make the module
/// un-sweepable-with-a-reason — it makes the module invisible, which is the one
/// outcome the SKIP table exists to prevent. That is what happened when `Memory`
/// first became a legal parameter type: the pipelined CPU dropped out of the
/// corpus silently and only `every_corpus_module_has_a_generated_case` (which
/// scans for `#[hardware]` by attribute, NOT through this function) noticed.
/// So a new parameter kind belongs here first, and in `skip_reason` second.
fn classify(ty: &syn::Type) -> Option<(Kind, String, String)> {
    let syn::Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    let kind = match seg.ident.to_string().as_str() {
        "Clock" => Kind::Clock,
        "In" => Kind::In,
        "Out" => Kind::Out,
        "RegOut" => Kind::RegOut,
        "Memory" => Kind::Memory,
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
        // `Memory<T, R, W, D, RL, WL>` — the domain is the 4th argument, not the
        // 2nd, because R and W sit between the element type and it.
        Kind::Memory => Some((kind, text.first()?.clone(), text.get(3)?.clone())),
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

    let with_reference = match REFERENCE.iter().find(|(n, _)| *n == m.name) {
        Some((_, path)) => format!("\n            .with_hand_written_reference(\"{path}\")"),
        None => String::new(),
    };
    let with_params = match params {
        Some(ps) => format!(
            "\n            .with_params(&[{}])",
            ps.iter().map(|(n, v)| format!("(\"{n}\", {v})")).collect::<Vec<_>>().join(", ")
        ),
        None => String::new(),
    };
    let _ = writeln!(
        code,
        "        let mut eq = __Eq::differential_only(\"{name}\", SRC, Some(\"{name}\")){with_params}{with_reference};"
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
            // `skip_reason` returns early for any module with one, so a memory
            // never reaches code generation. Spelled out rather than caught by a
            // wildcard so a future parameter kind fails to compile here instead of
            // being silently dropped on the floor.
            Kind::Memory => unreachable!("a module with a Memory parameter is skipped"),
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
            Kind::Memory => unreachable!("a module with a Memory parameter is skipped"),
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
