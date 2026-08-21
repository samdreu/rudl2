//! P2 nested-tick awaits — a FULL sim ≡ transpiled-SV equivalence test.
//!
//! `if_tick` has a branch-nested tick (then: 1 tick, else: 2); `if_tick_explicit` is
//! the hand-written pc-FSM twin. This asserts BOTH that control extraction is correct
//! (the branch-nested form behaves identically to the explicit FSM in the simulator)
//! AND that the transpiler agrees (`if_tick`'s SV matches the sim under Verilator).
//!
//! The output is `RegOut`, not `Out`: `if_tick` writes `out_o` on both sides of a bare
//! tick after a leading `sel` read, which the multi-write-around-a-tick guardrail
//! rejects for a plain (combinational) `Out` — a plain `Out` there collapses in the
//! sim (see `copper_analysis::multi_write_collapse` and the paper's contribution 5).
//! `RegOut` (registered / non-blocking) both satisfies the guardrail and reconciles
//! sim with the transpiled `always_ff`, so this runs as a full equivalence check
//! rather than the earlier `sim_only`.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, Out, RegOut};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/control_extraction_dut.rs");
const SRC: &str = include_str!("fixtures/control_extraction_dut.rs");

#[test]
fn if_tick_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("if_tick", SRC, Some("if_tick"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (sel_drv, sel_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (dut_out, dut_obs) = registered_wire(&clk, Logic::Zero);
    let (ref_out, ref_obs) = registered_wire(&clk, Logic::Zero);
    let dut_dh = dut_out.dirty_handle();
    let ref_dh = ref_out.dirty_handle();
    // DUT = the branch-nested async form; reference = the explicit pc-FSM twin.
    exec.spawn_wired(if_tick(clk.clone(), sel_in.clone(), dut_out), vec![dut_dh], vec![sel_in.wire_id()]);
    exec.spawn_wired(
        if_tick_explicit(clk.clone(), sel_in.clone(), ref_out),
        vec![ref_dh],
        vec![sel_in.wire_id()],
    );

    let stimulus = [1u8, 0, 0, 1, 1, 0, 0, 0, 1, 0, 1, 1];
    for &s in &stimulus {
        let sel = if s == 1 { Logic::One } else { Logic::Zero };
        sel_drv.write(sel);
        exec.tick_clock(&mut clk);
        eq.record(
            &[("sel", &[sel])],
            &[("out_o", &[dut_obs.read()])],
            &[("out_o", &[ref_obs.read()])],
        );
    }

    eq.finish();
}
