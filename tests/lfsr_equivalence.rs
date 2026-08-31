//! Behavioral-equivalence test for a 32-bit LFSR.
//!
//! Exercises the M2 features that made this example transpilable: `.as_bool()`,
//! constant bit-indexing (`state[0]`), `Bits::from_u32` constructors, pre-loop
//! combinational constants (`xor_mask`), parenthesised shift/xor expressions,
//! and `if`-expressions with block branches.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/lfsr_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/lfsr_dut.rs");

/// Reference model in plain `u32`, independent of the DUT's bit representation.
fn next_lfsr(state: u32, yumi: bool) -> u32 {
    if !yumi {
        return state;
    }
    let mask: u32 = (1 << 31) | (1 << 29) | (1 << 26) | (1 << 25);
    if state & 1 == 1 {
        (state >> 1) ^ mask
    } else {
        state >> 1
    }
}

#[test]
fn lfsr_sim_matches_transpiled_verilog() {
    // Also G2 structural reg-match vs the independent BaseJump STL reference: its
    // flip-flop is `o_r` (`xor_mask`/`o_n` are combinational `assign`s), vs Copper's
    // `state` — StorageEquivalent (count = 1). Directly exercises constant
    // exclusion: `xor_mask` is a pre-loop constant, correctly NOT a register on
    // either side.
    let mut eq = EquivalenceTest::new("lfsr", DUT_SRC).with_reference_registers(
        "examples/sequential/sv/lfsr.sv",
        copper_analysis::RegMatch::StorageEquivalent,
    );

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (reset_drv, reset_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (yumi_drv, yumi_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (o_out, o_obs) = wire::<Bits<32>, MainClk>(Bits::from_u32(1));
    let dh = o_out.dirty_handle();
    let reads = vec![reset_in.wire_id(), yumi_in.wire_id()];
    exec.spawn_wired(lfsr(clk.clone(), reset_in, yumi_in, o_out), vec![dh], reads);

    // One reset cycle, then advance.
    let stimulus: Vec<(Logic, Logic)> = std::iter::once((Logic::One, Logic::Zero))
        .chain((0..16).map(|_| (Logic::Zero, Logic::One)))
        .collect();

    let mut ref_state: u32 = 1;

    for &(reset, yumi) in stimulus.iter() {
        reset_drv.write(reset);
        yumi_drv.write(yumi);
        exec.tick_clock(&mut clk);

        ref_state = if reset == Logic::One {
            1
        } else {
            next_lfsr(ref_state, yumi == Logic::One)
        };
        let expected = Bits::<32>::from_u32(ref_state);

        eq.record(
            &[("reset_i", &[reset]), ("yumi_i", &[yumi])],
            &[("o", o_obs.read().as_array())],
            &[("o", expected.as_array())],
        );
    }

    eq.finish();
}
