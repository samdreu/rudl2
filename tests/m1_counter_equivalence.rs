//! Milestone 1 acceptance test: behavioral equivalence between the Copper
//! simulation and the transpiled SystemVerilog for a counter.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::{Bits, Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/counter_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/counter_dut.rs");

#[test]
#[ignore = "sim re-timed to atomic semantics; transpiler output-timing alignment pending — see design_docs/ATOMIC_INSTANT_EXECUTOR.md"]
fn counter_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("counter", DUT_SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (step_drv, step_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (out_out, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_out.dirty_handle();
    exec.spawn_wired(counter(clk.clone(), step_in, out_out), vec![dh]);

    // Reference: `out.write(count)` happens after `count += step` on each resumed
    // tick, so the value observed after cycle i is the running sum inclusive of
    // step[i], wrapping at 8 bits.
    let steps: [u8; 8] = [1, 1, 3, 0, 5, 200, 100, 2];
    let mut acc: u16 = 0;

    for &s in steps.iter() {
        step_drv.write(Bits::from_u8(s));
        exec.tick_clock(&mut clk);

        acc = (acc + s as u16) & 0xff;
        let step_bits = Bits::<8>::from_u8(s);
        let expected_bits = Bits::<8>::from_u8(acc as u8);

        eq.record(
            &[("step", step_bits.as_array())],
            &[("out", out_obs.read().as_array())],
            &[("out", expected_bits.as_array())],
        );
    }

    eq.finish();
}
