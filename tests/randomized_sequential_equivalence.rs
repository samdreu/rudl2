//! Randomized / property-based differential testing for the sequential corpus
//! (P0, `TODO` TESTING plan).
//!
//! Drives **hundreds of seeded-random per-cycle inputs** through clocked DUTs,
//! checking the simulator against an independent Rust reference *and* against the
//! transpiled SystemVerilog under Verilator every cycle. The reference models here
//! are the ones already validated cycle-exact by the hand-vector equivalence tests
//! (`up_down_counter`, `accumulator_en`, `shift_register`), so the only new
//! variable is the stimulus — random input streams a fixed vector set never walks.
//!
//! These fixtures share no helper names, so they are `include!`d at top level.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::{EquivalenceTest, Rng};
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/up_down_counter_dut.rs");
include!("fixtures/accumulator_en_dut.rs");
include!("fixtures/shift_register_dut.rs");

const UP_DOWN_SRC: &str = include_str!("fixtures/up_down_counter_dut.rs");
const ACCUM_SRC: &str = include_str!("fixtures/accumulator_en_dut.rs");
const SHIFT_SRC: &str = include_str!("fixtures/shift_register_dut.rs");

const ITERS: usize = 400;

#[test]
fn up_down_counter_random_directions() {
    let mut eq = EquivalenceTest::new("up_down_counter", UP_DOWN_SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (dir_drv, dir_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (out_out, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_out.dirty_handle();
    exec.spawn_wired(up_down_counter(clk.clone(), dir_in, out_out), vec![dh], vec![]);

    let mut rng = Rng::new(0x0D0_C0117);
    let mut model: u8 = 0;
    for _ in 0..ITERS {
        let up = rng.next_u64() & 1 == 1;
        dir_drv.write(common::logic(up));
        exec.tick_clock(&mut clk);
        model = if up { model.wrapping_add(1) } else { model.wrapping_sub(1) };
        eq.record(
            &[("dir", &[common::logic(up)])],
            &[("out", out_obs.read().as_array())],
            &[("out", Bits::<8>::from_u8(model).as_array())],
        );
    }
    eq.finish();
}

#[test]
fn accumulator_en_random_stream() {
    let mut eq = EquivalenceTest::new("accumulator_en", ACCUM_SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (en_drv, en_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (data_drv, data_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (out_out, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_out.dirty_handle();
    exec.spawn_wired(accumulator_en(clk.clone(), en_in, data_in, out_out), vec![dh], vec![]);

    let mut rng = Rng::new(0xACC0_9111);
    let mut model: u8 = 0;
    for _ in 0..ITERS {
        let en = rng.next_u64() & 1 == 1;
        let data = rng.u8();
        en_drv.write(common::logic(en));
        data_drv.write(Bits::<8>::from_u8(data));
        exec.tick_clock(&mut clk);
        if en {
            model = model.wrapping_add(data);
        }
        eq.record(
            &[("en", &[common::logic(en)]), ("data", Bits::<8>::from_u8(data).as_array())],
            &[("out", out_obs.read().as_array())],
            &[("out", Bits::<8>::from_u8(model).as_array())],
        );
    }
    eq.finish();
}

/// Reference model for the bidirectional shift register (the cycle-exact spec from
/// `tests/shift_register_equivalence.rs`, in plain `u8` shifts).
fn shift_next(prev: u8, rstn: Logic, en: Logic, dir: Logic, d: Logic) -> u8 {
    if rstn == Logic::Zero {
        0
    } else if en != Logic::One {
        prev
    } else if dir == Logic::Zero {
        (prev << 1) | (d == Logic::One) as u8
    } else {
        (prev >> 1) | (((d == Logic::One) as u8) << 7)
    }
}

#[test]
fn shift_register_random_stream() {
    const N: usize = 8;
    const N_1: usize = 7;
    let mut eq = EquivalenceTest::new("shift_register", SHIFT_SRC)
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

    let mut rng = Rng::new(0x5417_5EED);
    let mut model: u8 = 0;
    for i in 0..ITERS {
        // Cycle 0 forces a reset so both DUT (initial `Bits::x()`) and the model
        // start from a defined 0; afterwards reset recurs ~1/8 of the time.
        let rstn = if i == 0 { Logic::Zero } else { common::logic(rng.below(8) != 0) };
        let en = rng.logic();
        let dir = rng.logic();
        let d = rng.logic();
        rstn_drv.write(rstn);
        en_drv.write(en);
        dir_drv.write(dir);
        d_drv.write(d);

        exec.tick_clock(&mut clk);
        model = shift_next(model, rstn, en, dir, d);

        eq.record(
            &[("rstn", &[rstn]), ("en", &[en]), ("dir", &[dir]), ("d", &[d])],
            &[("out", out_obs.read().as_array())],
            &[("out", Bits::<N>::from_u8(model).as_array())],
        );
    }
    eq.finish();
}
