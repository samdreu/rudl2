//! Behavioral-equivalence test for a one-bit comparator (combinational XNOR).
//!
//! Promotes the `examples/combinational/one_bit_comparator` self-check into the
//! `cargo test` suite: the DUT is transpiled to SystemVerilog and Verilated
//! against the simulator's own trace, and the simulator is checked against an
//! independent reference (`eq == (i0 == i1)`) over the full 2×2 input space.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::Logic;
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

include!("fixtures/one_bit_comparator_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/one_bit_comparator_dut.rs");

#[test]
fn one_bit_comparator_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("one_bit_comparator", DUT_SRC);

    let mut exec = HardwareExecutor::new();
    let (i0_drv, i0_in) = wire::<Logic, ()>(Logic::Zero);
    let (i1_drv, i1_in) = wire::<Logic, ()>(Logic::Zero);
    let (eq_out, eq_obs) = wire::<Logic, ()>(Logic::Zero);
    let dh = eq_out.dirty_handle();
    let reads = vec![i0_in.wire_id(), i1_in.wire_id()];
    exec.spawn_wired(one_bit_comparator(i0_in, i1_in, eq_out), vec![dh], reads);

    // The entire input space: eq is One iff the two inputs are equal.
    for &(i0, i1) in &[
        (Logic::Zero, Logic::Zero),
        (Logic::Zero, Logic::One),
        (Logic::One, Logic::Zero),
        (Logic::One, Logic::One),
    ] {
        i0_drv.write(i0);
        i1_drv.write(i1);
        exec.poll_tasks();

        let expected = if i0 == i1 { Logic::One } else { Logic::Zero };
        eq.record(
            &[("i0", &[i0]), ("i1", &[i1])],
            &[("eq", &[eq_obs.read()])],
            &[("eq", &[expected])],
        );
    }

    eq.finish();
}
