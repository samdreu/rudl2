//! Behavioral-equivalence test for an up/down counter (branching register update).
//!
//! A single-register sequential DUT whose flip-flop is incremented or decremented
//! per cycle depending on a direction input. Transpiled to SystemVerilog and
//! Verilated against the simulator's trace; the simulator is also checked against
//! an independent wrapping-`u8` reference. Exercises a per-tick `if/else` and
//! wrapping `-` — beyond the unconditional `counter` fixture.
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

include!("fixtures/up_down_counter_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/up_down_counter_dut.rs");

#[test]
fn up_down_counter_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("up_down_counter", DUT_SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (dir_drv, dir_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (out_out, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_out.dirty_handle();
    let reads = vec![dir_in.wire_id()];
    exec.spawn_wired(up_down_counter(clk.clone(), dir_in, out_out), vec![dh], reads);

    // Directions per cycle: climb, wrap down past zero, climb again.
    let dirs = [true, true, true, false, false, false, false, true, true];
    let mut model: u8 = 0; // wrapping reference

    for &up in &dirs {
        dir_drv.write(if up { Logic::One } else { Logic::Zero });
        exec.tick_clock(&mut clk);

        model = if up { model.wrapping_add(1) } else { model.wrapping_sub(1) };
        let dir_bit = if up { Logic::One } else { Logic::Zero };

        eq.record(
            &[("dir", &[dir_bit])],
            &[("out", out_obs.read().as_array())],
            &[("out", Bits::<8>::from_u8(model).as_array())],
        );
    }

    eq.finish();
}
