//! Equivalence test for det_010 — a single-tick Moore FSM with a POST-tick
//! (trailing-segment) output, and `matches!`. The post-tick output was silently
//! dropped before the trailing port-drive fix; this locks it in and is a second
//! single-tick witness for the sim-vs-hardware timing question.
mod common;
use common::{logic, EquivalenceTest};
use copper_core::port::{wire, In, Out};
use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;
struct MainClk;
impl ClockDomain for MainClk {}
include!("fixtures/det_010_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/det_010_dut.rs");

// Reference: apply the transition, then output = (new state == D). Matches the
// post-tick output timing.
fn next(state: u8, rstn: bool, input: bool) -> u8 {
    if !rstn { return 0; }
    match (state, input) {
        (0, false) => 1, (1, true) => 2, (1, false) => 1,
        (2, false) => 3, (3, false) => 1, _ => 0,
    }
}

// Sim re-timed to atomic-instant semantics (output at its `out.write` instant);
// the sim now differs from the un-aligned transpiler (which drives `out` early).
// Un-ignore + re-baseline once the transpiler is aligned to the atomic output
// timing. See design_docs/ATOMIC_INSTANT_EXECUTOR.md.
#[test]
fn det_010_sim_matches_transpiled_verilog() {
    // Also G2 structural reg-match: pattern_detector_010.sv is a genuinely
    // independent two-process Moore machine whose flip-flop is `cur_state` (vs
    // Copper's `state`), so names can't match — StorageEquivalent (count = 1).
    let mut eq = EquivalenceTest::new("det_010", DUT_SRC).with_reference_registers(
        "examples/sequential/sv/pattern_detector_010.sv",
        copper_analysis::RegMatch::StorageEquivalent,
    );
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
    let (in_drv, in_port) = wire::<Logic, MainClk>(Logic::Zero);
    let (out_drv, out_obs) = wire::<Logic, MainClk>(Logic::Zero);
    let dh = out_drv.dirty_handle();
    exec.spawn_wired(det_010(clk.clone(), rstn_in, in_port, out_drv), vec![dh]);

    // (rstn, input) — from the module's own coverage table.
    let cases: &[(bool, bool)] = &[
        (false,false),(true,true),(true,false),(true,false),(true,true),(true,false),
        (true,true),(true,false),(true,true),(true,false),(true,false),(true,false),
        (true,true),(true,false),(false,true),(true,false),(true,true),(true,true),
    ];
    let mut st = 0u8;
    for &(rstn, input) in cases {
        rstn_drv.write(logic(rstn)); in_drv.write(logic(input));
        exec.tick_clock(&mut clk);
        st = next(st, rstn, input);
        eq.record(
            &[("rstn", &[logic(rstn)]), ("in_i", &[logic(input)])],
            &[("out_o", &[out_obs.read()])],
            &[("out_o", &[logic(st == 3)])]);
    }
    eq.finish();
}
