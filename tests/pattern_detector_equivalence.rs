//! Behavioral-equivalence test for a "110101" sequence detector.
//!
//! Exercises M2 class-A/B: a **file-scope enum** used as state (encoded to
//! `logic [2:0]`), a **tuple match** `match (state, in)` lowered to a
//! concatenated selector `{state, in_i}`, and a **conditional (Moore) output**
//! driven from `always_comb`.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::{logic, EquivalenceTest};
use copper_core::port::{wire, In, Out};
use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/pattern_detector_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/pattern_detector_dut.rs");

/// Reference model in plain integers — the 110101 transition table.
fn next_state(state: u8, rstn: bool, bit: bool) -> u8 {
    if !rstn {
        return 0;
    }
    match (state, bit) {
        (0, true) => 1,
        (1, true) => 2,
        (2, false) => 3,
        (3, true) => 4,
        (4, false) => 5,
        (5, true) => 6,
        _ => 0,
    }
}

#[test]
fn pattern_detector_sim_matches_transpiled_verilog() {
    // Also G2 structural reg-match vs the independent chipverify.com reference:
    // its flip-flop is `cur_state` (output `out` is combinational), a different
    // name from Copper's `state` — StorageEquivalent (count = 1).
    let mut eq = EquivalenceTest::new("det_110101", DUT_SRC).with_reference_registers(
        "examples/sequential/sv/pattern_detector.sv",
        copper_analysis::RegMatch::StorageEquivalent,
    );

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
    let (in_drv, in_port) = wire::<Logic, MainClk>(Logic::Zero);
    let (out_drv, out_obs) = wire::<Logic, MainClk>(Logic::Zero);
    let dh = out_drv.dirty_handle();
    exec.spawn_wired(det_110101(clk.clone(), rstn_in, in_port, out_drv), vec![dh]);

    // A reset, the target sequence 110101 (asserts on the last bit), then a
    // partial/overlapping run and a mid-stream reset.
    let stimulus: &[(bool, bool)] = &[
        (false, false), // reset
        (true, true),   // 1
        (true, true),   // 11
        (true, false),  // 110
        (true, true),   // 1101
        (true, false),  // 11010
        (true, true),   // 110101 -> detect
        (true, true),   // restart
        (true, false),  // no match
        (false, true),  // reset mid-stream
        (true, true),   // 1
        (true, true),   // 11
    ];

    // Unlike `counter`, the DUT updates `state` and writes `out_o` from the
    // *new* state before `clk.tick().await` — both in the same pre-edge
    // phase. `#[hardware(sequential)]`'s barrier sits after `tick()`, so it
    // only defers when the *next* iteration starts; this cycle's output was
    // already committed before the barrier runs. No shift needed here
    // (confirmed against the simulator's actual trace).
    let mut ref_state: u8 = 0;

    for &(rstn, bit) in stimulus.iter() {
        rstn_drv.write(logic(rstn));
        in_drv.write(logic(bit));
        exec.tick_clock(&mut clk);

        ref_state = next_state(ref_state, rstn, bit);

        eq.record(
            &[("rstn", &[logic(rstn)]), ("in_i", &[logic(bit)])],
            &[("out_o", &[out_obs.read()])],
            &[("out_o", &[logic(ref_state == 6)])],
        );
    }

    eq.finish();
}
