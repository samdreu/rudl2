//! Behavioral-equivalence test for a const-generic ripple-carry adder.
//!
//! Promotes the `examples/combinational/ripple_carry_adder` self-check into the
//! `cargo test` suite. A parametric combinational DUT (clockless `()` domain) with
//! **two** outputs — an N-bit sum and a carry-out — transpiled to a parametric SV
//! module and Verilated at N=8 via `.with_params`, at the same monomorphization the
//! simulator runs. Reference: `{cout, sum} == a + b` (9-bit unsigned).
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::Logic;
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

include!("fixtures/ripple_carry_adder_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/ripple_carry_adder_dut.rs");

const N: usize = 8;

#[test]
fn ripple_carry_adder_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("ripple_carry_adder", DUT_SRC).with_params(&[("N", N as i64)]);

    let mut exec = HardwareExecutor::new();
    let (a_drv, a_in) = wire::<Bits<N>, ()>(Bits::zero());
    let (b_drv, b_in) = wire::<Bits<N>, ()>(Bits::zero());
    let (sum_out, sum_obs) = wire::<Bits<N>, ()>(Bits::zero());
    let (cout_out, cout_obs) = wire::<Logic, ()>(Logic::Zero);
    let dhs = vec![sum_out.dirty_handle(), cout_out.dirty_handle()];
    let reads = vec![a_in.wire_id(), b_in.wire_id()];
    exec.spawn_wired(ripple_carry_adder::<N>(a_in, b_in, sum_out, cout_out), dhs, reads);

    // A spread including boundary carries (overflow, max+max, half+half).
    let cases: &[(u8, u8)] = &[
        (0, 0),
        (1, 2),
        (127, 1),
        (255, 0),
        (255, 1),
        (128, 128),
        (200, 100),
        (255, 255),
        (15, 240),
    ];

    for &(a, b) in cases {
        a_drv.write(Bits::<N>::from_u8(a));
        b_drv.write(Bits::<N>::from_u8(b));
        exec.poll_tasks();

        let full = a as u16 + b as u16;
        let expected_sum = Bits::<N>::from_u8((full & 0xff) as u8);
        let expected_cout = if full > 0xff { Logic::One } else { Logic::Zero };

        eq.record(
            &[
                ("a_i", Bits::<N>::from_u8(a).as_array()),
                ("b_i", Bits::<N>::from_u8(b).as_array()),
            ],
            &[("sum_o", sum_obs.read().as_array()), ("cout_o", &[cout_obs.read()])],
            &[("sum_o", expected_sum.as_array()), ("cout_o", &[expected_cout])],
        );
    }

    eq.finish();
}
