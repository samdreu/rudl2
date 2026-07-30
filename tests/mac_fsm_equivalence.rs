//! Adjudicating equivalence test: the single-tick `mac_fsm` (explicit Stage
//! machine) vs its transpiled Verilog. mac_fsm produces identical *simulator*
//! output to mac_pipeline, but is single-tick — the category that transpiles
//! correctly — so if THIS passes verilator while mac_pipeline fails, the sim is
//! the ground truth and mac_pipeline's multi-tick transpilation is the bug.
//! Also the first real exercise of conditional-output -> implicit-hold register.
mod common;
use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;
struct MainClk;
impl ClockDomain for MainClk {}
include!("fixtures/mac_fsm_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/mac_fsm_dut.rs");

// The canonical `RegOut` case, now aligned. mac_fsm writes `out` in the Stage::Out
// arm and holds it between writes, so it is a registered output: declared `RegOut`,
// the simulator (deferred-commit `registered_wire`) and the transpiler (`out <=
// result` in `always_ff`) both give the +1 registered timing and agree under
// Verilator. See design_docs/REGISTERED_OUTPUTS.md.
#[test]
fn mac_fsm_sim_matches_transpiled_verilog() {
    // Also G2 structural reg-match: mac_fsm.sv is a faithful translation mirroring
    // the design's names, so NameExact — {stage, product, c_latch, result}.
    let mut eq = EquivalenceTest::new("mac_fsm", DUT_SRC).with_reference_registers(
        "tests/fixtures/timing_probe_sv/mac_fsm.sv",
        copper_analysis::RegMatch::NameExact,
    );
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (a_drv, a_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (b_drv, b_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (c_drv, c_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (out_drv, out_obs): (RegOut<Bits<8>, MainClk>, _) = registered_wire(&clk, Bits::zero());
    let dh = out_drv.dirty_handle();
    exec.spawn_wired(mac_fsm(clk.clone(), a_in, b_in, c_in, out_drv), vec![dh]);
    // mac_fsm samples inputs in its Load state, which recurs every 3 cycles (0,3,6);
    // hold each input group across its 3-cycle window so each MAC is well-defined.
    // out = a*b+c, driven in the Out state (cycles 2,5,8) and held (RegOut): the
    // three groups give 2*3+4=10, 5*6+7=37, 10*10+5=105.
    let inputs: &[(u8,u8,u8)] = &[(2,3,4),(2,3,4),(2,3,4),(5,6,7),(5,6,7),(5,6,7),(10,10,5),(10,10,5),(10,10,5)];
    let expected: &[u8] = &[0,0,10,10,10,37,37,37,105];
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
