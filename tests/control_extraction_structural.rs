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

// ── State-count budget ────────────────────────────────────────────────────────

/// The FSM must be the size of the DESIGN, not of the lowering's bookkeeping.
///
/// Leaving a loop is not a clock boundary, so a `break` INLINES the enclosing
/// loop's continuation — and that continuation used to be lowered afresh at every
/// `break` site, allocating a fresh set of states for every tick in it. The
/// desugared counted `for` has two `break`s, each lowered again by the rotation's
/// entry copy, so each nesting level roughly doubled. `examples/uart/rx.rs` came
/// out at 788 states and 16k lines of SystemVerilog — enough to overflow the `pc`
/// register, which is how it was noticed at all.
///
/// The count did not depend on `CLKS_PER_BIT` (434 and 8 both gave 788), which is
/// what identified it as duplication rather than unrolling. `LoopCtx::lowered_break`
/// caches the lowered continuation, so it costs one set of states however many
/// `break`s reach it.
///
/// A budget rather than an exact number: this pins the ORDER OF MAGNITUDE so an
/// exponential blow-up fails loudly, without freezing a state count that a
/// legitimate lowering change may legitimately shift by one or two.
#[test]
fn a_deeply_nested_design_does_not_blow_up_the_state_count() {
    let src = include_str!("fixtures/uart_rx_dut.rs");
    let sv = transpile_source(src, Some("uart_rx_dut"), &EmitConfig::default())
        .expect("the UART receiver must transpile");

    // `<width>'d<n>:` at the head of a line — a `case` arm label, and nothing else
    // (an expression mentioning `32'd1` must not be counted as a state).
    fn case_label(line: &str) -> Option<&str> {
        let t = line.trim();
        let (head, _) = t.split_once(':')?;
        let (w, v) = head.split_once("'d")?;
        let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
        (digits(w) && digits(v)).then_some(head)
    }
    let mut states: Vec<&str> = sv.lines().filter_map(case_label).collect();
    states.sort_unstable();
    states.dedup();

    assert!(
        states.len() <= 24,
        "the receiver flattened to {} states — it has about ten segments, so this \
         is duplication, not design. Was the break continuation re-lowered per \
         `break` again?\n{sv}",
        states.len()
    );
    // …and it must still be a real FSM, not one collapsed by an unrelated bug.
    assert!(
        states.len() >= 6,
        "only {} states: the receiver's segments cannot all be there",
        states.len()
    );
}
