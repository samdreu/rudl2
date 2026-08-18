//! P2 nested-tick awaits. `if_tick` has a branch-nested tick (then: 1 tick, else:
//! 2 ticks); `if_tick_explicit` is the hand-written pc-FSM twin. This asserts the
//! branch-nested form behaves identically to the explicit FSM **in the simulator**
//! (a control-extraction behavioral differential — the structural test only checks
//! SV shape).
//!
//! P2 nested-tick awaits. `if_tick` has a branch-nested tick (then: 1 tick, else:
//! 2 ticks); `if_tick_explicit` is the hand-written pc-FSM twin. This asserts the
//! branch-nested form behaves identically to the explicit FSM **in the simulator**
//! (a control-extraction behavioral differential — the structural test only checks
//! SV shape).
//!
//! Runs `sim_only`. The registered-Moore-output codegen bug (which blocked `seq6`)
//! is now FIXED, and this FSM's output does lower to `always_comb`. But `if_tick`'s
//! output is *Mealy* — in state `pc=0` it is `sel ? 1 : 0`, depending on the live
//! input — and the combinational SV then disagrees with the simulator at some
//! cycles: a SEPARATE, still-open output-timing (Mealy) reconciliation, distinct
//! from the Moore-decode fix. So the Verilator half stays parked; the sim
//! self-consistency (branch-nested ≡ explicit FSM) is what this asserts.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/control_extraction_dut.rs");

#[test]
fn if_tick_branch_nested_matches_explicit_fsm() {
    let mut eq = EquivalenceTest::sim_only("if_tick");

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (sel_drv, sel_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (dut_out, dut_obs) = wire::<Logic, MainClk>(Logic::Zero);
    let (ref_out, ref_obs) = wire::<Logic, MainClk>(Logic::Zero);
    let dut_dh = dut_out.dirty_handle();
    let ref_dh = ref_out.dirty_handle();
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
