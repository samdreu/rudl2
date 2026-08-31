//! Behavioral-equivalence test for an enabled accumulator.
//!
//! The register `acc` accumulates `data` only on cycles where `en` is high, and
//! **holds** otherwise — the enabled-register idiom (verified sim≡BaseJump on
//! `bsg_dff_en`). Transpiled to SystemVerilog and Verilated against the
//! simulator's trace; the simulator is also checked against a wrapping-`u8`
//! reference that only accumulates on enabled cycles.
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

include!("fixtures/accumulator_en_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/accumulator_en_dut.rs");

#[test]
fn accumulator_en_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("accumulator_en", DUT_SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (en_drv, en_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (data_drv, data_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (out_out, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_out.dirty_handle();
    let reads = vec![en_in.wire_id(), data_in.wire_id()];
    exec.spawn_wired(accumulator_en(clk.clone(), en_in, data_in, out_out), vec![dh], reads);

    // (enable, data): note the disabled cycles, where `acc` must hold.
    let cases: &[(bool, u8)] = &[
        (true, 5),
        (false, 100), // held: data ignored
        (true, 10),
        (true, 200), // wraps
        (false, 7), // held
        (true, 1),
        (true, 250), // wraps again
    ];
    let mut model: u8 = 0;

    for &(en, data) in cases {
        en_drv.write(if en { Logic::One } else { Logic::Zero });
        data_drv.write(Bits::<8>::from_u8(data));
        exec.tick_clock(&mut clk);

        if en {
            model = model.wrapping_add(data);
        }
        let en_bit = if en { Logic::One } else { Logic::Zero };

        eq.record(
            &[("en", &[en_bit]), ("data", Bits::<8>::from_u8(data).as_array())],
            &[("out", out_obs.read().as_array())],
            &[("out", Bits::<8>::from_u8(model).as_array())],
        );
    }

    eq.finish();
}
