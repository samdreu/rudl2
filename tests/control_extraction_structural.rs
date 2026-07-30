//! Increment-A control-extraction validation, by STRUCTURAL Verilog equivalence.
//!
//! Each async branch-tick module (ticks inside `if`/`else`) is transpiled and
//! compared against the hand-written explicit single-tick `match pc` FSM a human
//! would write for it. Identical SystemVerilog proves the control-extraction pass
//! produces exactly the intended FSM — the det_010_awaits→det_010 adjudication
//! method, applied to straight-line + if/else (increment A per
//! design_docs/CONTROL_EXTRACTION.md).
//!
//! This is deliberately independent of the simulator, so it is unaffected by the
//! still-open output-timing reconciliation (EXECUTION_MODEL_RECONCILIATION.md);
//! the read-timing half is already reconciled (see mac_pipeline_equivalence).

use copper_codegen::{transpile_source, EmitConfig};

const DUT_SRC: &str = include_str!("fixtures/control_extraction_dut.rs");

/// Transpile `module` from the fixture and blank out its `module <name>` header
/// so two modules can be compared on body alone.
fn transpiled_body(module: &str) -> String {
    let sv = transpile_source(DUT_SRC, Some(module), &EmitConfig::default())
        .unwrap_or_else(|e| panic!("transpiling '{module}' failed: {e}"));
    sv.replacen(&format!("module {module} "), "module M ", 1)
}

/// Assert the async branch-tick module extracts to exactly the explicit FSM.
fn assert_extracts_to(async_module: &str, explicit_module: &str) {
    let extracted = transpiled_body(async_module);
    let explicit = transpiled_body(explicit_module);
    assert_eq!(
        extracted, explicit,
        "control extraction of `{async_module}` does not match the hand-written \
         explicit FSM `{explicit_module}`.\n\n--- extracted ---\n{extracted}\n\
         --- explicit ---\n{explicit}"
    );
}

/// Straight-line + `if`/`else` with asymmetric tick counts (then: 1 tick, else: 2).
#[test]
fn if_tick_extracts_to_explicit_fsm() {
    assert_extracts_to("if_tick", "if_tick_explicit");
}

/// `if`/`else` with a continuation after the `if` — exercises the
/// continuation-duplication rule (the tail is inlined into the non-ticking arm
/// and deferred into a new state on the ticking arm).
#[test]
fn branch_merge_extracts_to_explicit_fsm() {
    assert_extracts_to("branch_merge", "branch_merge_explicit");
}

/// Ticks *inside* `match` arms, with a mid-arm tick in one arm — exercises the
/// match-arm generalization of `lower_into` (descend into arms; allocate a fresh
/// `pc` state for the second cycle of the `Double` arm).
#[test]
fn match_tick_extracts_to_explicit_fsm() {
    assert_extracts_to("match_tick", "match_tick_explicit");
}
