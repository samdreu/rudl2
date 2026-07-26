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

// KNOWN FAILURE (documented) — the canonical `RegOut` case. mac_fsm writes `out`
// *before* its tick (in the Stage::Out arm), so under the post-edge convention the
// plain-`Out` simulator observes it one cycle *early* relative to the transpiler's
// registered output (`out <= result` at the edge). The fix is to declare this
// output `RegOut` (which defers it one cycle to match) — the macro + sim already
// support `RegOut`, but the transpiler does not yet lower it, so this fixture stays
// on plain `Out` and the test stays ignored until transpiler `RegOut` support
// lands. (Contrast mac_pipeline / det_010 / counter, which now pass under post-edge
// with plain `Out`.) See design_docs/EXECUTOR_CONVENTION_EXPERIMENT.md and
// design_docs/REGISTERED_OUTPUTS.md.
#[test]
#[ignore = "canonical RegOut case: write-before-tick output is 1 cycle early under post-edge with plain Out; needs RegOut lowered by the transpiler (deferred) — see design_docs/EXECUTOR_CONVENTION_EXPERIMENT.md"]
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
