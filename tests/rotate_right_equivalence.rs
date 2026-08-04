//! Behavioral-equivalence test for a const-generic barrel rotate-right.
//!
//! A **combinational** parametric DUT (clockless, `()` domain): transpiled to a
//! parametric SV module and Verilated at N=8, N_LOG=3 via `.with_params`, at the
//! same monomorphization the simulator runs. Exercises const generics, a `for`
//! loop, LHS bit-assign, dynamic bit-select, and `%`.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::Logic;
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

include!("fixtures/rotate_right_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/rotate_right_dut.rs");

const N: usize = 8;
const N_LOG: usize = 3;

/// Reference: output bit `i` is input bit `(i + rot) % N`.
fn rotate_right_ref(data: u8, rot: usize) -> u8 {
    let mut o = 0u8;
    for i in 0..N {
        if (data >> ((i + rot) % N)) & 1 == 1 {
            o |= 1 << i;
        }
    }
    o
}

#[test]
fn rotate_right_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("rotate_right", DUT_SRC)
        .with_params(&[("N", N as i64), ("N_LOG", N_LOG as i64)]);

    let mut exec = HardwareExecutor::new();
    let (data_drv, data_in) = wire::<Bits<N>, ()>(Bits::zero());
    let (rot_drv, rot_in) = wire::<Bits<N_LOG>, ()>(Bits::zero());
    let (o_drv, o_obs) = wire::<Bits<N>, ()>(Bits::zero());
    let dh = o_drv.dirty_handle();
    let reads = vec![data_in.wire_id(), rot_in.wire_id()];
    exec.spawn_wired(rotate_right::<N, N_LOG>(data_in, rot_in, o_drv), vec![dh], reads);

    // (data, rot) — a spread of patterns and rotation amounts.
    let cases: &[(u8, usize)] = &[
        (0b1011_0001, 1),
        (0b1011_0001, 3),
        (0b1011_0001, 4),
        (0b1111_1111, 5),
        (0b0000_0001, 1),
        (0b1000_0000, 7),
        (0b1011_0001, 0),
    ];

    for &(data, rot) in cases {
        data_drv.write(Bits::<N>::from_u8(data));
        rot_drv.write(Bits::<N_LOG>::from_usize(rot));
        exec.poll_tasks();

        let actual = o_obs.read();
        let expected = Bits::<N>::from_u8(rotate_right_ref(data, rot));
        eq.record(
            &[("data_i", &Bits::<N>::from_u8(data).as_array()[..]),
              ("rot_i", &Bits::<N_LOG>::from_usize(rot).as_array()[..])],
            &[("o", actual.as_array())],
            &[("o", expected.as_array())],
        );
    }

    eq.finish();
}
