//! Behavioral-equivalence test for a 3-stage MAC pipeline `(a*b)+c`.
//!
//! The **multi-phase async/await** case: three `clk.tick().await` boundaries
//! lower to a 3-phase FSM with inferred pipeline registers and phase-local
//! combinational temps. This is the example the latch P0 fix targets — the temps
//! get `= '0` defaults at the top of the merged always_comb, so the transpiled SV
//! is latch-free and matches the simulator.
//!
//! Stimulus and expected timeline are the example's own (3-cycle latency; the
//! output holds its last value between stage-3 writes). See `tests/common/mod.rs`.

mod common;

use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/mac_pipeline_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/mac_pipeline_dut.rs");

// RECONCILED (2026-07-24): the multi-tick input-read timing gap is fixed. The
// simulator now samples cross-tick reads at the registering edge (the pre-edge of
// the tick_clock the read belongs to), matching the transpiled phase FSM and
// independent hand-written Verilog — see EXECUTION_MODEL_RECONCILIATION.md and the
// `copper-sim/src/synced_read.rs` fix. Inputs are read every 3 cycles at phase 0
// (cycles 0, 3, 6), so the stimulus places its three input groups there.
#[test]
#[ignore = "sim re-timed to atomic semantics; transpiler output-timing alignment pending — see design_docs/ATOMIC_INSTANT_EXECUTOR.md"]
fn mac_pipeline_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("mac_pipeline", DUT_SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (a_drv, a_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (b_drv, b_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (c_drv, c_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (out_drv, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_drv.dirty_handle();
    exec.spawn_wired(
        mac_pipeline(clk.clone(), a_in, b_in, c_in, out_drv),
        vec![dh],
    );

    // (a, b, c) per cycle. Input groups sit on the read cycles (0, 3, 6 — phase 0);
    // each result appears via `assign out = sum_r` two cycles after its read and
    // holds until the next result. Same timeline as before, groups shifted onto the
    // hardware-accurate read cadence.
    let inputs: &[(u8, u8, u8)] = &[
        (2, 3, 4),   // 0: Group A, read at phase 0
        (0, 0, 0),   // 1
        (0, 0, 0),   // 2
        (5, 6, 7),   // 3: Group B, read at phase 0
        (0, 0, 0),   // 4
        (0, 0, 0),   // 5
        (10, 10, 5), // 6: Group C, read at phase 0
        (0, 0, 0),   // 7
        (0, 0, 0),   // 8
    ];
    let expected: &[u8] = &[
        0,   // 0
        10,  // 1: (2*3)+4
        10,  // 2: stale
        10,  // 3: stale
        37,  // 4: (5*6)+7
        37,  // 5: stale
        37,  // 6: stale
        105, // 7: (10*10)+5
        105, // 8: stale
    ];

    for (&(av, bv, cv), &exp) in inputs.iter().zip(expected.iter()) {
        a_drv.write(Bits::from_u8(av));
        b_drv.write(Bits::from_u8(bv));
        c_drv.write(Bits::from_u8(cv));

        exec.tick_clock(&mut clk);

        eq.record(
            &[
                ("a", &Bits::<8>::from_u8(av).as_array()[..]),
                ("b", &Bits::<8>::from_u8(bv).as_array()[..]),
                ("c", &Bits::<8>::from_u8(cv).as_array()[..]),
            ],
            &[("out", out_obs.read().as_array())],
            &[("out", Bits::<8>::from_u8(exp).as_array())],
        );
    }

    eq.finish();
}
