//! Six-state enum FSM (P2, `TODO` TESTING plan).
//!
//! Pushes past the four-state `traffic_light`: six enum states and a wider `match`,
//! stressing state encoding and transition lowering at scale. A ring counter that
//! advances one state per cycle when `en` is high and holds when low, emitting the
//! state index as three `Logic` Moore-output bits. Random `en` stimulus wraps the
//! ring several times.
//!
//! NOTE — runs `sim_only` (no Verilator cross-check). The transpiler currently
//! lowers this FSM's Moore output into `always_ff` (registered, one cycle late)
//! instead of a combinational `always_comb` decode, so sim≡SV FAILS while the sim
//! itself is correct. That is a tracked CODEGEN bug (see `TODO` TRANSPILATION);
//! this test verifies the simulation semantics meanwhile and is ready to become a
//! full equivalence test (`EquivalenceTest::new` + `.finish()`) the day it is fixed.
//!
//! See `tests/common/mod.rs` for the shared harness and how to read a failure.

mod common;

use common::{EquivalenceTest, Rng};
use copper_core::port::{wire, In, Out};
use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/seq6_dut.rs");

#[test]
fn seq6_sim_matches_reference_model() {
    let mut eq = EquivalenceTest::sim_only("seq6");

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (en_drv, en_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (b0_out, b0_obs) = wire::<Logic, MainClk>(Logic::Zero);
    let (b1_out, b1_obs) = wire::<Logic, MainClk>(Logic::Zero);
    let (b2_out, b2_obs) = wire::<Logic, MainClk>(Logic::Zero);
    let dhs = vec![b0_out.dirty_handle(), b1_out.dirty_handle(), b2_out.dirty_handle()];
    let reads = vec![en_in.wire_id()];
    exec.spawn_wired(seq6(clk.clone(), en_in, b0_out, b1_out, b2_out), dhs, reads);

    // Reference: the Moore decode is combinational from the state register, so the
    // bits observed after `tick_clock(i)` reflect the state reached by applying
    // `en[0..=i]` — the same combinational-Moore timing as the traffic light.
    // b2 b1 b0 = binary of the state index.
    let bit = |v: u8, k: u8| common::logic((v >> k) & 1 == 1);
    let mut state: u8 = 0;
    let mut rng = Rng::new(0x5E96_C0DE);
    for _ in 0..48 {
        let en = rng.logic();
        en_drv.write(en);
        exec.tick_clock(&mut clk);

        if en == Logic::One {
            state = (state + 1) % 6;
        }
        eq.record(
            &[("en", &[en])],
            &[
                ("b0", &[b0_obs.read()]),
                ("b1", &[b1_obs.read()]),
                ("b2", &[b2_obs.read()]),
            ],
            &[
                ("b0", &[bit(state, 0)]),
                ("b1", &[bit(state, 1)]),
                ("b2", &[bit(state, 2)]),
            ],
        );
    }

    eq.finish();
}
