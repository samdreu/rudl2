//! Randomized / property-based differential testing for the combinational corpus
//! (P0, `TODO` TESTING plan).
//!
//! The rest of the suite drives hand-picked vectors; this drives **hundreds of
//! seeded-random inputs** per module through the full equivalence harness, so each
//! iteration checks the simulator against an independent Rust reference *and*
//! against the transpiled SystemVerilog under Verilator. A divergence fixed vectors
//! would miss (a carry edge, a rotate wrap, a priority tie) shows up here, and the
//! seed makes any failure reproducible.
//!
//! Each DUT is the exact fixture the per-module equivalence tests transpile,
//! `include!`d inside its own module so the shared `safe_clog2` helpers don't
//! collide.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::{EquivalenceTest, Rng};
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::Logic;
use copper_sim::HardwareExecutor;

// ── DUTs (each in its own module to isolate helper names) ─────────────────────

mod adder {
    use copper_core::port::{In, Out};
    use copper_core::types::Bits;
    use copper_core::Logic;
    use copper_macros::hardware;
    include!("fixtures/ripple_carry_adder_dut.rs");
}
mod cmp {
    use copper_core::port::{In, Out};
    use copper_core::Logic;
    use copper_macros::hardware;
    include!("fixtures/one_bit_comparator_dut.rs");
}
mod gray {
    use copper_core::port::{In, Out};
    use copper_core::types::Bits;
    use copper_core::Logic;
    use copper_macros::hardware;
    include!("fixtures/gray_to_binary_dut.rs");
}
mod rot {
    use copper_core::port::{In, Out};
    use copper_core::types::Bits;
    use copper_core::Logic;
    use copper_macros::hardware;
    include!("fixtures/rotate_right_dut.rs");
}
mod prio {
    use copper_core::port::{In, Out};
    use copper_core::types::Bits;
    use copper_core::Logic;
    use copper_macros::hardware;
    include!("fixtures/priority_encode_dut.rs");
}

const ADDER_SRC: &str = include_str!("fixtures/ripple_carry_adder_dut.rs");
const CMP_SRC: &str = include_str!("fixtures/one_bit_comparator_dut.rs");
const GRAY_SRC: &str = include_str!("fixtures/gray_to_binary_dut.rs");
const ROT_SRC: &str = include_str!("fixtures/rotate_right_dut.rs");
const PRIO_SRC: &str = include_str!("fixtures/priority_encode_dut.rs");

const ITERS: usize = 500;

// ── references ────────────────────────────────────────────────────────────────

fn gray_decode(g: u8) -> u8 {
    let mut b = g;
    b ^= b >> 1;
    b ^= b >> 2;
    b ^= b >> 4;
    b
}

fn rotate_right_ref(data: u8, rot: usize) -> u8 {
    let mut o = 0u8;
    for i in 0..8 {
        if (data >> ((i + rot) % 8)) & 1 == 1 {
            o |= 1 << i;
        }
    }
    o
}

/// MSB-priority encode of an 8-bit input: (index of highest set bit, any-set).
fn priority_ref(inputs: u8) -> (usize, bool) {
    if inputs == 0 {
        (0, false)
    } else {
        (7 - inputs.leading_zeros() as usize, true)
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[test]
fn ripple_carry_adder_random_inputs() {
    let mut eq = EquivalenceTest::new("ripple_carry_adder", ADDER_SRC).with_params(&[("N", 8)]);

    let mut exec = HardwareExecutor::new();
    let (a_drv, a_in) = wire::<Bits<8>, ()>(Bits::zero());
    let (b_drv, b_in) = wire::<Bits<8>, ()>(Bits::zero());
    let (sum_out, sum_obs) = wire::<Bits<8>, ()>(Bits::zero());
    let (cout_out, cout_obs) = wire::<Logic, ()>(Logic::Zero);
    let dhs = vec![sum_out.dirty_handle(), cout_out.dirty_handle()];
    let reads = vec![a_in.wire_id(), b_in.wire_id()];
    exec.spawn_wired(adder::ripple_carry_adder::<8>(a_in, b_in, sum_out, cout_out), dhs, reads);

    let mut rng = Rng::new(0xADDE_5EED);
    for _ in 0..ITERS {
        let (a, b) = (rng.u8(), rng.u8());
        a_drv.write(Bits::<8>::from_u8(a));
        b_drv.write(Bits::<8>::from_u8(b));
        exec.poll_tasks();

        let full = a as u16 + b as u16;
        let esum = Bits::<8>::from_u8((full & 0xff) as u8);
        let ecout = if full > 0xff { Logic::One } else { Logic::Zero };
        eq.record(
            &[("a_i", Bits::<8>::from_u8(a).as_array()), ("b_i", Bits::<8>::from_u8(b).as_array())],
            &[("sum_o", sum_obs.read().as_array()), ("cout_o", &[cout_obs.read()])],
            &[("sum_o", esum.as_array()), ("cout_o", &[ecout])],
        );
    }
    eq.finish();
}

#[test]
fn one_bit_comparator_random_inputs() {
    let mut eq = EquivalenceTest::new("one_bit_comparator", CMP_SRC);

    let mut exec = HardwareExecutor::new();
    let (i0_drv, i0_in) = wire::<Logic, ()>(Logic::Zero);
    let (i1_drv, i1_in) = wire::<Logic, ()>(Logic::Zero);
    let (eq_out, eq_obs) = wire::<Logic, ()>(Logic::Zero);
    let dh = eq_out.dirty_handle();
    let reads = vec![i0_in.wire_id(), i1_in.wire_id()];
    exec.spawn_wired(cmp::one_bit_comparator(i0_in, i1_in, eq_out), vec![dh], reads);

    let mut rng = Rng::new(0xC0FFEE);
    for _ in 0..ITERS {
        let (i0, i1) = (rng.logic(), rng.logic());
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

#[test]
fn gray_to_binary_random_inputs() {
    let mut eq = EquivalenceTest::new("gray_to_binary", GRAY_SRC);

    let mut exec = HardwareExecutor::new();
    let (g_drv, g_in) = wire::<Bits<8>, ()>(Bits::zero());
    let (b_out, b_obs) = wire::<Bits<8>, ()>(Bits::zero());
    let dh = b_out.dirty_handle();
    let reads = vec![g_in.wire_id()];
    exec.spawn_wired(gray::gray_to_binary(g_in, b_out), vec![dh], reads);

    let mut rng = Rng::new(0x62A4_D3C0);
    for _ in 0..ITERS {
        let g = rng.u8();
        g_drv.write(Bits::<8>::from_u8(g));
        exec.poll_tasks();

        let expected = Bits::<8>::from_u8(gray_decode(g));
        eq.record(
            &[("gray_i", Bits::<8>::from_u8(g).as_array())],
            &[("binary_o", b_obs.read().as_array())],
            &[("binary_o", expected.as_array())],
        );
    }
    eq.finish();
}

#[test]
fn rotate_right_random_inputs() {
    let mut eq = EquivalenceTest::new("rotate_right", ROT_SRC).with_params(&[("N", 8), ("N_LOG", 3)]);

    let mut exec = HardwareExecutor::new();
    let (data_drv, data_in) = wire::<Bits<8>, ()>(Bits::zero());
    let (rot_drv, rot_in) = wire::<Bits<3>, ()>(Bits::zero());
    let (o_out, o_obs) = wire::<Bits<8>, ()>(Bits::zero());
    let dh = o_out.dirty_handle();
    let reads = vec![data_in.wire_id(), rot_in.wire_id()];
    exec.spawn_wired(rot::rotate_right::<8, 3>(data_in, rot_in, o_out), vec![dh], reads);

    let mut rng = Rng::new(0x2013A7E);
    for _ in 0..ITERS {
        let data = rng.u8();
        let rot = rng.below(8) as usize;
        data_drv.write(Bits::<8>::from_u8(data));
        rot_drv.write(Bits::<3>::from_usize(rot));
        exec.poll_tasks();

        let expected = Bits::<8>::from_u8(rotate_right_ref(data, rot));
        eq.record(
            &[
                ("data_i", Bits::<8>::from_u8(data).as_array()),
                ("rot_i", Bits::<3>::from_usize(rot).as_array()),
            ],
            &[("o", o_obs.read().as_array())],
            &[("o", expected.as_array())],
        );
    }
    eq.finish();
}

#[test]
fn priority_encode_random_inputs() {
    let mut eq = EquivalenceTest::new("priority_encode", PRIO_SRC).with_params(&[("N", 8), ("N_LOG", 3)]);

    let mut exec = HardwareExecutor::new();
    let (in_drv, in_in) = wire::<Bits<8>, ()>(Bits::zero());
    let (res_out, res_obs) = wire::<Bits<3>, ()>(Bits::zero());
    let (v_out, v_obs) = wire::<Logic, ()>(Logic::Zero);
    let dhs = vec![res_out.dirty_handle(), v_out.dirty_handle()];
    let reads = vec![in_in.wire_id()];
    exec.spawn_wired(prio::priority_encode::<8, 3>(in_in, res_out, v_out), dhs, reads);

    let mut rng = Rng::new(0x9165_17ED);
    for _ in 0..ITERS {
        let inputs = rng.u8();
        in_drv.write(Bits::<8>::from_u8(inputs));
        exec.poll_tasks();

        let (idx, valid) = priority_ref(inputs);
        eq.record(
            &[("inputs", Bits::<8>::from_u8(inputs).as_array())],
            &[("result", res_obs.read().as_array()), ("valid", &[v_obs.read()])],
            &[("result", Bits::<3>::from_usize(idx).as_array()), ("valid", &[common::logic(valid)])],
        );
    }
    eq.finish();
}
