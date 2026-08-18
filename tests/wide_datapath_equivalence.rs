//! Equivalence test for a wide (16-bit) multi-output combinational datapath
//! (P2, `TODO` TESTING plan). Fills three construct-matrix gaps at once: a datapath
//! wider than the 8-bit boundary, the `*` multiply operator, and output fan-in
//! (multiple `Out`s driven from shared inputs). Driven with seeded-random stimulus
//! so carry/overflow/truncation edges are covered, and cross-checked against the
//! transpiled SystemVerilog under Verilator.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::{EquivalenceTest, Rng};
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

include!("fixtures/datapath16_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/datapath16_dut.rs");

#[test]
fn datapath16_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("datapath16", DUT_SRC);

    let mut exec = HardwareExecutor::new();
    let (a_drv, a_in) = wire::<Bits<16>, ()>(Bits::zero());
    let (b_drv, b_in) = wire::<Bits<16>, ()>(Bits::zero());
    let (sum_o, sum_obs) = wire::<Bits<16>, ()>(Bits::zero());
    let (prod_o, prod_obs) = wire::<Bits<16>, ()>(Bits::zero());
    let (diff_o, diff_obs) = wire::<Bits<16>, ()>(Bits::zero());
    let dhs = vec![sum_o.dirty_handle(), prod_o.dirty_handle(), diff_o.dirty_handle()];
    let reads = vec![a_in.wire_id(), b_in.wire_id()];
    exec.spawn_wired(datapath16(a_in, b_in, sum_o, prod_o, diff_o), dhs, reads);

    let mut rng = Rng::new(0xDA7A_9A75);
    for _ in 0..500 {
        let a = rng.next_u64() as u16;
        let b = rng.next_u64() as u16;
        a_drv.write(Bits::<16>::from_u16(a));
        b_drv.write(Bits::<16>::from_u16(b));
        exec.poll_tasks();

        // All three operations are 16-bit wrapping (SystemVerilog truncates to the
        // port width; `*` keeps the low 16 bits of the product).
        let esum = Bits::<16>::from_u16(a.wrapping_add(b));
        let eprod = Bits::<16>::from_u16(a.wrapping_mul(b));
        let ediff = Bits::<16>::from_u16(a.wrapping_sub(b));
        eq.record(
            &[
                ("a", Bits::<16>::from_u16(a).as_array()),
                ("b", Bits::<16>::from_u16(b).as_array()),
            ],
            &[
                ("sum", sum_obs.read().as_array()),
                ("prod", prod_obs.read().as_array()),
                ("diff", diff_obs.read().as_array()),
            ],
            &[
                ("sum", esum.as_array()),
                ("prod", eprod.as_array()),
                ("diff", ediff.as_array()),
            ],
        );
    }
    eq.finish();
}
