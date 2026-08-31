//! Behavioral-equivalence test for a two-tick-per-step counter (back-to-back
//! `clk.tick().await`s).
//!
//! The register advances once every *two* clock cycles because two consecutive
//! ticks separate the register write from its update. This pins the "back-to-back
//! tick awaits" corner case (`TODO`, SIMULATOR): consecutive suspension points map
//! to distinct FSM states, and the value must hold across the intermediate tick.
//! Verilating the generated SystemVerilog against the simulator's trace validates
//! the exact cycle timing independently — so the expected schedule is not
//! hand-fitted to the simulator.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, Out};
use copper_core::{Bits, Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/slow_counter_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/slow_counter_dut.rs");

#[test]
fn slow_counter_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("slow_counter", DUT_SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (out_out, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_out.dirty_handle();
    exec.spawn_wired(slow_counter(clk.clone(), out_out), vec![dh], vec![]);

    // Independent reference: with the register write before the first tick and the
    // increment after the second, the value observed after clock cycle `i` is
    // `(i + 1) / 2` — it advances on every odd cycle and holds on every even one.
    // The Verilator cross-check is the ultimate authority on the exact schedule; a
    // trace mismatch with Verilator passing would flag THIS prediction, not the
    // transpiler (see `tests/common/mod.rs`).
    for i in 0..10u8 {
        exec.tick_clock(&mut clk);
        let expected = (i + 1) / 2;
        eq.record(
            &[],
            &[("out", out_obs.read().as_array())],
            &[("out", Bits::<8>::from_u8(expected).as_array())],
        );
    }

    eq.finish();
}
