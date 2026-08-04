//! Behavioral-equivalence test for a const-generic bidirectional shift register.
//!
//! The first **parametric** equivalence test: the DUT is transpiled to a
//! parametric SystemVerilog module (`parameter int N`, `[N-1:0]` ports) and
//! Verilated at concrete widths via `.with_params(&[("N", 8), ("N_1", 7)])`, at
//! the same monomorphization (`shift_register::<8, 7>`) the simulator runs.
//!
//! Exercises const generics, for-loops, LHS bit-assign, dynamic bit-select, and
//! the auto-hoist of a block-local combinational temp — all at once.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/shift_register_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/shift_register_dut.rs");

const N: usize = 8;
const N_1: usize = 7;

/// Reference model in plain `u8` shifts — the behavioral spec, independent of how
/// the DUT indexes bits internally.
fn next_state(prev: u8, rstn: Logic, en: Logic, dir: Logic, d: Logic) -> u8 {
    if rstn == Logic::Zero {
        0
    } else if en != Logic::One {
        prev // hold
    } else if dir == Logic::Zero {
        (prev << 1) | (d == Logic::One) as u8 // shift left, d enters LSB
    } else {
        (prev >> 1) | (((d == Logic::One) as u8) << (N - 1)) // shift right, d enters MSB
    }
}

#[test]
fn shift_register_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("shift_register", DUT_SRC)
        .with_params(&[("N", N as i64), ("N_1", N_1 as i64)]);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (d_drv, d_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (en_drv, en_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (dir_drv, dir_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
    let (out_drv, out_obs) = wire::<Bits<N>, MainClk>(Bits::zero());
    let dh = out_drv.dirty_handle();
    let reads = vec![d_in.wire_id(), en_in.wire_id(), dir_in.wire_id(), rstn_in.wire_id()];
    exec.spawn_wired(
        shift_register::<N, N_1>(d_in, clk.clone(), en_in, dir_in, rstn_in, out_drv),
        vec![dh],
        reads,
    );

    // (rstn, en, dir, d) — reset, left-shift a few bits, hold, right-shift, reset.
    let stimulus: &[(Logic, Logic, Logic, Logic)] = &[
        (Logic::Zero, Logic::Zero, Logic::Zero, Logic::Zero), // reset
        (Logic::One, Logic::One, Logic::Zero, Logic::One),    // left, d=1
        (Logic::One, Logic::One, Logic::Zero, Logic::One),    // left, d=1
        (Logic::One, Logic::One, Logic::Zero, Logic::Zero),   // left, d=0
        (Logic::One, Logic::Zero, Logic::Zero, Logic::Zero),  // hold (en=0)
        (Logic::One, Logic::One, Logic::One, Logic::One),     // right, d=1
        (Logic::One, Logic::One, Logic::One, Logic::Zero),    // right, d=0
        (Logic::Zero, Logic::One, Logic::One, Logic::One),    // reset overrides
    ];

    let mut ref_state = 0u8;
    for &(rstn, en, dir, d) in stimulus {
        rstn_drv.write(rstn);
        en_drv.write(en);
        dir_drv.write(dir);
        d_drv.write(d);

        exec.tick_clock(&mut clk);
        ref_state = next_state(ref_state, rstn, en, dir, d);

        let actual = out_obs.read();
        let expected = Bits::<N>::from_u8(ref_state);
        eq.record(
            &[
                ("rstn", &[rstn]),
                ("en", &[en]),
                ("dir", &[dir]),
                ("d", &[d]),
            ],
            &[("out", actual.as_array())],
            &[("out", expected.as_array())],
        );
    }

    eq.finish();
}
