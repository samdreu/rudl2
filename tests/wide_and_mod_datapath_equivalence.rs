//! P2 leftovers: wide `Bits<32>`/`Bits<64>` datapaths and the `%` operator
//! (`TODO` TESTING plan). `datapath16` already covered 16-bit + `*` + fan-in; this
//! pushes the width boundary to 32 and 64 bits (full-width multiply included) and
//! adds the supported-but-untested `%` remainder operator. Seeded-random stimulus,
//! cross-checked against the transpiled SystemVerilog under Verilator.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::{EquivalenceTest, Rng};
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

include!("fixtures/wide_alu_dut.rs");
include!("fixtures/mod8_dut.rs");
const WIDE_SRC: &str = include_str!("fixtures/wide_alu_dut.rs");
const MOD_SRC: &str = include_str!("fixtures/mod8_dut.rs");

/// Run the width-generic ALU at one concrete width `N` with `mask` = `2^N - 1`.
/// Inputs are drawn below `2^N` so the u128 reference matches the hardware exactly.
fn run_wide_alu<const N: usize>(seed: u64, mask: u128) {
    let mut eq = EquivalenceTest::new("wide_alu", WIDE_SRC).with_params(&[("N", N as i64)]);

    let mut exec = HardwareExecutor::new();
    let (a_drv, a_in) = wire::<Bits<N>, ()>(Bits::zero());
    let (b_drv, b_in) = wire::<Bits<N>, ()>(Bits::zero());
    let (sum_o, sum_obs) = wire::<Bits<N>, ()>(Bits::zero());
    let (prod_o, prod_obs) = wire::<Bits<N>, ()>(Bits::zero());
    let (diff_o, diff_obs) = wire::<Bits<N>, ()>(Bits::zero());
    let dhs = vec![sum_o.dirty_handle(), prod_o.dirty_handle(), diff_o.dirty_handle()];
    let reads = vec![a_in.wire_id(), b_in.wire_id()];
    exec.spawn_wired(wide_alu::<N>(a_in, b_in, sum_o, prod_o, diff_o), dhs, reads);

    // Values are all < 2^N ≤ 2^64, so they round-trip through `usize`
    // (`from_u128` requires N ≥ 128; `from_usize` covers N ≤ 64).
    let bits = |v: u128| Bits::<N>::from_usize(v as usize);
    let mut rng = Rng::new(seed);
    for _ in 0..400 {
        // 64 random bits masked to N — for N=64 this is the full word.
        let a = (rng.next_u64() as u128) & mask;
        let b = (rng.next_u64() as u128) & mask;
        a_drv.write(bits(a));
        b_drv.write(bits(b));
        exec.poll_tasks();

        let esum = bits(a.wrapping_add(b) & mask);
        let eprod = bits(a.wrapping_mul(b) & mask);
        let ediff = bits(a.wrapping_sub(b) & mask);
        eq.record(
            &[("a", bits(a).as_array()), ("b", bits(b).as_array())],
            &[
                ("sum", sum_obs.read().as_array()),
                ("prod", prod_obs.read().as_array()),
                ("diff", diff_obs.read().as_array()),
            ],
            &[("sum", esum.as_array()), ("prod", eprod.as_array()), ("diff", ediff.as_array())],
        );
    }
    eq.finish();
}

/// Both widths run in ONE test, sequentially: the generic module transpiles to a
/// SystemVerilog module named `wide_alu` regardless of `N`, so the harness's temp
/// `.sv` path and Verilator `obj_dir` are shared between monomorphizations. Two
/// separate `#[test]`s would race under cargo's parallel runner (each clobbering
/// the other's build); running them in sequence keeps each transpile→verilate→check
/// self-contained.
#[test]
fn wide_alu_matches_transpiled_verilog() {
    run_wide_alu::<32>(0x3232_ABCD, (1u128 << 32) - 1);
    run_wide_alu::<64>(0x6464_1234, (1u128 << 64) - 1);
}

#[test]
fn mod8_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("mod8", MOD_SRC);

    let mut exec = HardwareExecutor::new();
    let (a_drv, a_in) = wire::<Bits<8>, ()>(Bits::zero());
    let (b_drv, b_in) = wire::<Bits<8>, ()>(Bits::zero());
    let (o_out, o_obs) = wire::<Bits<8>, ()>(Bits::zero());
    let dh = o_out.dirty_handle();
    let reads = vec![a_in.wire_id(), b_in.wire_id()];
    exec.spawn_wired(mod8(a_in, b_in, o_out), vec![dh], reads);

    let mut rng = Rng::new(0x0D_0D_0D);
    for _ in 0..500 {
        let a = rng.u8();
        let b = (rng.below(255) as u8) + 1; // 1..=255, never zero
        a_drv.write(Bits::<8>::from_u8(a));
        b_drv.write(Bits::<8>::from_u8(b));
        exec.poll_tasks();

        let expected = Bits::<8>::from_u8(a % b);
        eq.record(
            &[("a", Bits::<8>::from_u8(a).as_array()), ("b", Bits::<8>::from_u8(b).as_array())],
            &[("o", o_obs.read().as_array())],
            &[("o", expected.as_array())],
        );
    }
    eq.finish();
}
