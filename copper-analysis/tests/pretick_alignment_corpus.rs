//! Corpus regression for the **pre-tick alignment hazard** detector
//! (`copper_analysis::unprotected_pretick_out_write`;
//! `design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md`).
//!
//! This is gate **G2** of that plan, and the point is the second half: the rule must
//! catch every module measured to diverge **without flagging a single one that
//! doesn't**. A prior candidate rule matched all seven synthetic variants and still
//! rejected `mac_fsm` — the project's G2 name-exact register reference — which is why
//! the corpus verdict is pinned here as a test rather than checked by eye.
//!
//! The expectation is an **exact set**, so this fails in both directions: a newly
//! flagged module is a regression (or a real bug in that module), and a
//! no-longer-flagged one means the hazard was fixed and the list needs updating.
//!
//! Scope note: the detector examines only the pre-tick segment, so the multi-tick
//! `accum_2` class is a known false negative (plan Q5) and is deliberately absent.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use syn::{Item, ItemFn};

/// Repo root, from this crate's manifest dir (`<root>/copper-analysis`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target" || n == "obj_dir") {
                continue;
            }
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The mode named by `#[hardware(<mode>, <flags>…)]`, ignoring any trailing flags.
///
/// Parsing with `parse_args::<syn::Ident>()` fails outright once a flag is present
/// (e.g. `allow_pretick_alignment`), which would make such modules vanish from this
/// scan — a check that silently stops running. Take the first token instead.
fn hardware_mode_of(f: &ItemFn) -> Option<String> {
    f.attrs.iter().find_map(|a| {
        if !a.path().segments.last().is_some_and(|s| s.ident == "hardware") {
            return None;
        }
        let text = a.meta.require_list().ok()?.tokens.to_string();
        Some(text.split(',').next()?.trim().to_string())
    })
}

/// Every `fn` carrying `#[hardware(...)]` in a clocked mode, including inside `mod`s.
/// `combinational` has no tick and therefore no pre-tick segment.
fn clocked_hardware_fns(items: &[Item], out: &mut Vec<ItemFn>) {
    for item in items {
        match item {
            Item::Fn(f) => {
                let mode = hardware_mode_of(f);
                if matches!(mode.as_deref(), Some("sequential") | Some("synchronizer")) {
                    out.push(f.clone());
                }
            }
            Item::Mod(m) => {
                if let Some((_, items)) = &m.content {
                    clocked_hardware_fns(items, out);
                }
            }
            _ => {}
        }
    }
}

/// The modules the detector must flag, as `file::module`.
///
/// After the phase-3 migration these are **exactly** the fixtures that exist to
/// *demonstrate* the divergence. Each carries
/// `#[hardware(sequential, allow_pretick_alignment)]`, so the macro permits it while
/// this analysis still reports it — deliberately: the opt-out silences the *error*,
/// not the *detection*, so an opted-out module cannot quietly disappear from the
/// corpus view.
///
/// * `add_then_write` / `fast_counter` in `sequential_forwarding_divergence.rs` —
///   the pinned sim-vs-Verilator proof that the hazard is real.
/// * `probe_fsm` — a pre-existing `#[ignore]`d divergence, measured to be the *same*
///   defect (a leading read on every path fixes it).
/// * `ram_prewrite` — flagged and plausible, but its only test
///   (`probe_mem_latency`) is `#[ignore]`d, so it still has no behavioral verdict.
///
/// The real designs that used to appear here — the three `fast_counter` copies in
/// `examples/cdc/` and `two_domain_hierarchy_cdc.rs` — were migrated to update the
/// sticky flag *after* the tick, the form measured to match the independent
/// hand-written SV reference.
const EXPECTED_FLAGGED: &[&str] = &[
    "tests/fixtures/probe_timing_dut.rs::probe_fsm",
    // `ram_prewrite` also left the set in the phase-D narrowing: its write is not
    // read-preceded, so it falls on the dissolved side of the discriminator. It
    // never had a behavioral verdict (it does not transpile; its only test is
    // `#[ignore]`d), so nothing measured contradicts the narrowing — if it ever
    // transpiles, the corpus sweep covers it the moment it does.
    // REMOVED 2026-08-26 (cycle-dataflow phase D): `add_then_write` and the
    // `fast_counter` witness were DISSOLVED by phase B's forwarded emission —
    // measured agreeing (`pre_tick_update_forwarding_agrees_end_to_end`,
    // `independent_hardware_anchors_the_corrected_spelling`) — and the rule was
    // narrowed to READ-PRECEDED register-reading writes (the path-dependent
    // region boundary), so the no-read opening shapes are legal now.
    //
    // ADDED the same day: `w4_mixed_alignment` — the retained class's own
    // demonstration witness (a read reaches the write on one path only),
    // measured DIVERGENT under forwarded emission (sim holds, SV alternates;
    // pinned in `d_narrowing_battery_verdicts`).
    "tests/sequential_forwarding_divergence.rs::w4_mixed_alignment",
    // Added 2026-08-25 when the CONSTANT-WRITE exemption was narrowed to
    // unconditional writes (guardrail 5.5). All three are measured divergences, not
    // rule regressions: each leads its own emitted SystemVerilog by exactly one
    // cycle, with both traces pinned in sequential_forwarding_divergence.rs. The
    // hold clause is untouched by forwarded emission (they write constants), so
    // the phase-D narrowing keeps all three.
    "tests/fixtures/control_extraction_dut.rs::branch_merge_explicit",
    "tests/sequential_forwarding_divergence.rs::pc_arm_toggle",
    "tests/sequential_forwarding_divergence.rs::pc_arm_write",
];

#[test]
fn pretick_alignment_hazard_flags_exactly_the_known_divergent_modules() {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in ["examples", "src", "tests"] {
        rs_files(&root.join(dir), &mut files);
    }
    files.sort();

    let mut scanned = 0usize;
    let mut flagged: BTreeSet<String> = BTreeSet::new();
    for path in &files {
        let Ok(src) = fs::read_to_string(path) else { continue };
        let Ok(file) = syn::parse_file(&src) else { continue };
        let mut fns = Vec::new();
        clocked_hardware_fns(&file.items, &mut fns);
        for f in &fns {
            scanned += 1;
            let ports = copper_analysis::unprotected_pretick_out_write(f);
            if !ports.is_empty() {
                let rel = path.strip_prefix(&root).unwrap_or(path).display();
                flagged.insert(format!("{rel}::{}", f.sig.ident));
            }
        }
    }

    assert!(scanned >= 40, "expected to scan the corpus, only saw {scanned} clocked modules");

    let expected: BTreeSet<String> = EXPECTED_FLAGGED.iter().map(|s| s.to_string()).collect();
    let unexpected: Vec<_> = flagged.difference(&expected).cloned().collect();
    let missing: Vec<_> = expected.difference(&flagged).cloned().collect();

    assert!(
        unexpected.is_empty(),
        "NEW module(s) flagged by the pre-tick alignment rule: {unexpected:#?}\n\
         Either that module has the divergence (check it against its transpiled SV), \
         or the rule has regressed into a false positive. Do NOT silence this by \
         adding to EXPECTED_FLAGGED without a measured verdict — a rule that rejects \
         a correct design is the failure mode this test exists to prevent."
    );
    assert!(
        missing.is_empty(),
        "module(s) no longer flagged: {missing:#?}\n\
         If the underlying divergence was fixed, drop them from EXPECTED_FLAGGED and \
         re-check design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md — several tests are \
         pinned to the current (divergent) behaviour and will need re-blessing."
    );

    eprintln!(
        "pre-tick alignment: {scanned} clocked modules scanned, {} flagged (all measured-divergent)",
        flagged.len()
    );
}


// ── The multi-phase output rule ───────────────────────────────────────────────

/// Plain combinational `Out` ports driven in **more than one clock phase**
/// (`copper_analysis::multi_phase_out_write`).
///
/// The sibling of the rule above, and the one that covers the case it cannot see.
/// `unprotected_pretick_out_write` examines only head → first tick (plan Q5, which
/// recorded the middle-segment gap as theoretical). An instance turned up on
/// 2026-08-25 — a one-cycle output pulse in the UART receiver — and widening the
/// D1 rule to every post-tick segment was measured and REJECTED: it flags 36 of
/// 120 corpus modules, ~30 with passing equivalence tests. Writing a plain `Out`
/// after a tick is the ordinary multi-phase pattern; writing it in *two* phases is
/// not, and that is what this rule keys on.
///
/// Like the rule above, the expectation is an **exact set** so it fails in both
/// directions. Every entry here must be a module that exists to DEMONSTRATE the
/// divergence and carries `#[hardware(sequential, allow_pretick_alignment)]` — the
/// opt-out silences the error, not the detection, so an opted-out module cannot
/// quietly disappear from this view. A real design appearing here is a bug in that
/// design, and the fix is `RegOut`.
const EXPECTED_MULTI_PHASE: &[&str] = &[
    "tests/sequential_forwarding_divergence.rs::pulse_plain",
];

#[test]
fn multi_phase_out_write_flags_exactly_the_demonstration_modules() {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in ["examples", "src", "tests"] {
        rs_files(&root.join(dir), &mut files);
    }
    files.sort();

    let mut scanned = 0usize;
    let mut flagged: BTreeSet<String> = BTreeSet::new();
    for path in &files {
        let Ok(src) = fs::read_to_string(path) else { continue };
        let Ok(file) = syn::parse_file(&src) else { continue };
        let mut fns = Vec::new();
        clocked_hardware_fns(&file.items, &mut fns);
        for f in &fns {
            scanned += 1;
            if !copper_analysis::multi_phase_out_write(f).is_empty() {
                let rel = path.strip_prefix(&root).unwrap_or(path).display();
                flagged.insert(format!("{rel}::{}", f.sig.ident));
            }
        }
    }

    assert!(scanned >= 40, "expected to scan the corpus, only saw {scanned} clocked modules");

    let expected: BTreeSet<String> = EXPECTED_MULTI_PHASE.iter().map(|s| s.to_string()).collect();
    let unexpected: Vec<_> = flagged.difference(&expected).cloned().collect();
    let missing: Vec<_> = expected.difference(&flagged).cloned().collect();
    assert!(
        unexpected.is_empty(),
        "newly flagged (a plain `Out` driven in two clock phases — declare it `RegOut`, \
         or if the module exists to demonstrate the hazard add `allow_pretick_alignment` \
         and list it here): {unexpected:?}"
    );
    assert!(
        missing.is_empty(),
        "no longer flagged — the hazard may be fixed, or the module changed: {missing:?}"
    );
}

/// The **trailing-segment** rule's exact set — D1's hazard past the last tick
/// (`unprotected_trailing_out_write`, 2026-08-25).
///
/// A measured divergence carrying `allow_pretick_alignment`. The rule's total corpus
/// cost was one real module: `rv32i_cpu_pipelined`'s `program_counter`, migrated to
/// `RegOut` — the same remedy its scalar sibling had already been given for the
/// multi-phase rule.
///
/// `pulse_plain` was here until the all-paths query was scoped to the region being
/// asked about (2026-08-25). Its trailing `dv.write(Zero)` is an UNCONDITIONAL
/// constant, so it is exempt on the same terms as any other — the rule was reading it
/// as conditional only because it asked whether the HEAD region wrote it. The module
/// is a divergence and stays guarded, by `multi_phase_out_write`: it drives `dv` in
/// two clock phases, which is what is actually wrong with it.
const EXPECTED_TRAILING: &[&str] = &[
    "tests/sequential_forwarding_divergence.rs::trailing_update",
    // Added 2026-08-27 by the phase-C decision probes (the rule's gate is the
    // tick count, which cannot see which lowering route the trailing statements
    // take); both verdicts are now MEASURED:
    //   * `linear_trailing` — the linear multi-tick spelling, MEASURED AGREEING
    //     (the linear path commits trailing updates at the right edge), so the
    //     rule's refusal there is a lowering limitation of the extraction
    //     route, not a semantics rule: a documented false positive held
    //     deliberately.
    //   * `branch_trailing` — extracted with a top-level last tick, MEASURED
    //     DIVERGING (SV is the sim trace delayed one cycle; unblocked by the
    //     2026-08-27 parenthesized-bit-select emission fix). A TRUE positive:
    //     the extraction route commits trailing updates one edge late
    //     regardless of where the last tick sits.
    // Recorded follow-up: narrow the rule to the EXTRACTION route alone
    // (exempting only the linear lowering), or retire it wholesale when
    // phase C lands the corrected trailing lowering.
    "tests/sequential_forwarding_divergence.rs::linear_trailing",
    "tests/sequential_forwarding_divergence.rs::branch_trailing",
];

#[test]
fn trailing_out_write_flags_exactly_the_demonstration_modules() {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in ["examples", "src", "tests"] {
        rs_files(&root.join(dir), &mut files);
    }
    files.sort();

    let mut scanned = 0usize;
    let mut flagged: BTreeSet<String> = BTreeSet::new();
    for path in &files {
        let Ok(src) = fs::read_to_string(path) else { continue };
        let Ok(file) = syn::parse_file(&src) else { continue };
        let mut fns = Vec::new();
        clocked_hardware_fns(&file.items, &mut fns);
        for f in &fns {
            scanned += 1;
            if !copper_analysis::unprotected_trailing_out_write(f).is_empty() {
                let rel = path.strip_prefix(&root).unwrap_or(path).display();
                flagged.insert(format!("{rel}::{}", f.sig.ident));
            }
        }
    }

    assert!(scanned >= 40, "expected to scan the corpus, only saw {scanned} clocked modules");

    let expected: BTreeSet<String> = EXPECTED_TRAILING.iter().map(|s| s.to_string()).collect();
    let unexpected: Vec<_> = flagged.difference(&expected).cloned().collect();
    let missing: Vec<_> = expected.difference(&flagged).cloned().collect();
    assert!(
        unexpected.is_empty(),
        "newly flagged by the trailing-segment rule. Either the module HAS the \
         divergence — check it against its transpiled SV, and the remedy is `RegOut` — \
         or the rule has regressed into a false positive, which is what cost the two \
         widenings in guardrail 5.4 their place: {unexpected:?}"
    );
    assert!(
        missing.is_empty(),
        "no longer flagged — the hazard may be fixed, or the module changed: {missing:?}"
    );
}

// ── The write-between-read-and-update rule (V8, m1) ──────────────────────────

/// Plain `Out` ports written between a leading `In` read and the update of a
/// register the write reads (`copper_analysis::pretick_out_write_before_update`;
/// `design_docs/DERIVATION_TABLE.md` F2).
///
/// The fifth member of the family, and the first DERIVED before it was measured:
/// the cycle-dataflow model predicted the divergence, the exact traces, and both
/// flipping controls (the V8 battery in `sequential_forwarding_divergence.rs`)
/// before the controlled measurement existed. The in-vivo instance was
/// `rv32i_cpu_transpilable`'s `program_counter` (TODO cause Q, divergence #1),
/// since rewritten to the post-commit form — which is why no real design appears
/// here. Same exact-set discipline as the rules above: the opt-out silences the
/// error, not the detection.
const EXPECTED_WRITE_BEFORE_UPDATE: &[&str] = &[
    "tests/sequential_forwarding_divergence.rs::v8a_read_write_update",
    // V8a with the update routed through a same-cycle temp — the shape the rule
    // caught in ui/pass/single_loop_local_ok.rs THE DAY IT LANDED (a compile-only
    // fixture nobody had measured; it diverges exactly like V8a and is pinned as
    // this witness; the fixture itself was reordered to the legal V8c form).
    "tests/sequential_forwarding_divergence.rs::v8d_temp_renamed_update",
];

#[test]
fn write_before_update_flags_exactly_the_demonstration_modules() {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in ["examples", "src", "tests"] {
        rs_files(&root.join(dir), &mut files);
    }
    files.sort();

    let mut scanned = 0usize;
    let mut flagged: BTreeSet<String> = BTreeSet::new();
    for path in &files {
        let Ok(src) = fs::read_to_string(path) else { continue };
        let Ok(file) = syn::parse_file(&src) else { continue };
        let mut fns = Vec::new();
        clocked_hardware_fns(&file.items, &mut fns);
        for f in &fns {
            scanned += 1;
            if !copper_analysis::pretick_out_write_before_update(f).is_empty() {
                let rel = path.strip_prefix(&root).unwrap_or(path).display();
                flagged.insert(format!("{rel}::{}", f.sig.ident));
            }
        }
    }

    assert!(scanned >= 40, "expected to scan the corpus, only saw {scanned} clocked modules");

    let expected: BTreeSet<String> =
        EXPECTED_WRITE_BEFORE_UPDATE.iter().map(|s| s.to_string()).collect();
    let unexpected: Vec<_> = flagged.difference(&expected).cloned().collect();
    let missing: Vec<_> = expected.difference(&flagged).cloned().collect();

    assert!(
        unexpected.is_empty(),
        "NEW module(s) flagged by the write-before-update rule: {unexpected:#?}\n\
         Either that module has the divergence (check it against its transpiled SV — \
         the V8a trace pair is the signature: SV leads the sim by exactly one), or \
         the rule has regressed into a false positive. Do NOT add to \
         EXPECTED_WRITE_BEFORE_UPDATE without a measured verdict."
    );
    assert!(
        missing.is_empty(),
        "module(s) no longer flagged: {missing:#?}\n\
         If the divergence was fixed (the paired cycle-dataflow migration), drop \
         them here and re-bless the V8 tests in sequential_forwarding_divergence.rs, \
         which pin today's divergent traces."
    );

    eprintln!(
        "write-before-update: {scanned} clocked modules scanned, {} flagged",
        flagged.len()
    );
}
