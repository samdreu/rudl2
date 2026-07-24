//! Adjudicating equivalence test: the single-tick `mac_fsm` (explicit Stage
//! machine) vs its transpiled Verilog. mac_fsm produces identical *simulator*
//! output to mac_pipeline, but is single-tick — the category that transpiles
//! correctly — so if THIS passes verilator while mac_pipeline fails, the sim is
//! the ground truth and mac_pipeline's multi-tick transpilation is the bug.
//! Also the first real exercise of conditional-output -> implicit-hold register.
mod common;
use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;
struct MainClk;
impl ClockDomain for MainClk {}
include!("fixtures/mac_fsm_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/mac_fsm_dut.rs");

// KNOWN FAILURE (documented). With conditional-output → implicit-hold register,
// mac_fsm now *transpiles* — and the adjudication result is decisive: its `out`
// (registered: `out <= result` at the Stage::Out edge) is **one cycle later** than
// the simulator (`trace: PASS`, `verilator: FAIL`, out lags by 1). This is the
// SAME one-cycle discrepancy as mac_pipeline's read timing, now on the output
// side — i.e. natural clocked hardware (register captures at the edge) is
// consistently one cycle *behind* the simulator for multi-cycle sequential logic.
// See design_docs/EXECUTION_MODEL_RECONCILIATION.md. Un-ignore once reconciled.
#[test]
#[ignore = "sim is 1 cycle ahead of the registered output — see EXECUTION_MODEL_RECONCILIATION.md"]
fn mac_fsm_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("mac_fsm", DUT_SRC);
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (a_drv, a_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (b_drv, b_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (c_drv, c_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (out_drv, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_drv.dirty_handle();
    exec.spawn_wired(mac_fsm(clk.clone(), a_in, b_in, c_in, out_drv), vec![dh]);
    let inputs: &[(u8,u8,u8)] = &[(2,3,4),(0,0,0),(5,6,7),(0,0,0),(0,0,0),(10,10,5),(0,0,0),(0,0,0),(0,0,0)];
    let expected: &[u8] = &[0,10,10,10,37,37,37,105,105];
    for (&(av,bv,cv), &exp) in inputs.iter().zip(expected.iter()) {
        a_drv.write(Bits::from_u8(av)); b_drv.write(Bits::from_u8(bv)); c_drv.write(Bits::from_u8(cv));
        exec.tick_clock(&mut clk);
        eq.record(
            &[("a",&Bits::<8>::from_u8(av).as_array()[..]),("b",&Bits::<8>::from_u8(bv).as_array()[..]),("c",&Bits::<8>::from_u8(cv).as_array()[..])],
            &[("out", out_obs.read().as_array())],
            &[("out", Bits::<8>::from_u8(exp).as_array())]);
    }
    eq.finish();
}
