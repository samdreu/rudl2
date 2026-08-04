//! Behavioral-equivalence test for a const-generic MSB-priority encoder.
//!
//! A **combinational** parametric DUT with two outputs (`result`, `valid`):
//! transpiled to a parametric SV module and Verilated at N=8, N_LOG=3 via
//! `.with_params`, at the same monomorphization the simulator runs.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::Logic;
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

include!("fixtures/priority_encode_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/priority_encode_dut.rs");

const N: usize = 8;
const N_LOG: usize = 3;

/// Reference: index of the highest set bit, and whether any bit is set.
fn priority_encode_ref(inputs: u8) -> (u8, bool) {
    let mut res = 0u8;
    let mut valid = false;
    for i in 0..N {
        if (inputs >> i) & 1 == 1 {
            res = i as u8;
            valid = true;
        }
    }
    (res, valid)
}

#[test]
fn priority_encode_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("priority_encode", DUT_SRC)
        .with_params(&[("N", N as i64), ("N_LOG", N_LOG as i64)]);

    let mut exec = HardwareExecutor::new();
    let (in_drv, in_port) = wire::<Bits<N>, ()>(Bits::zero());
    let (res_drv, res_obs) = wire::<Bits<N_LOG>, ()>(Bits::zero());
    let (valid_drv, valid_obs) = wire::<Logic, ()>(Logic::Zero);
    let dh_r = res_drv.dirty_handle();
    let dh_v = valid_drv.dirty_handle();
    let reads = vec![in_port.wire_id()];
    exec.spawn_wired(
        priority_encode::<N, N_LOG>(in_port, res_drv, valid_drv),
        vec![dh_r, dh_v],
        reads,
    );

    let cases: &[u8] = &[
        0b0000_0000,
        0b0000_0001,
        0b0000_0010,
        0b1000_0000,
        0b0000_0011, // bits 0,1 → 1 wins
        0b0011_0000, // bits 4,5 → 5 wins
        0b1000_0001, // bits 0,7 → 7 wins
        0b1111_1111, // all → 7 wins
    ];

    for &inputs in cases {
        in_drv.write(Bits::<N>::from_u8(inputs));
        exec.poll_tasks();

        let (exp_res, exp_valid) = priority_encode_ref(inputs);
        eq.record(
            &[("inputs", &Bits::<N>::from_u8(inputs).as_array()[..])],
            &[
                ("result", res_obs.read().as_array()),
                ("valid", &[valid_obs.read()]),
            ],
            &[
                ("result", Bits::<N_LOG>::from_usize(exp_res as usize).as_array()),
                ("valid", &[if exp_valid { Logic::One } else { Logic::Zero }]),
            ],
        );
    }

    eq.finish();
}
