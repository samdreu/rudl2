//! Behavioral-equivalence test for a Moore traffic-light FSM.
//!
//! The class-B2 proof — the transition match uses **pattern bindings** (`t`),
//! **arm guards** (`if t < 1`), and **partial wildcards** inside tuple patterns,
//! none of which fit a concatenated-selector `case`. It also covers **tuple
//! destructuring assignment**, whose simultaneity matters here: `timer` must be
//! computed from the *old* `phase`, not the newly assigned one.
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

include!("fixtures/traffic_light_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/traffic_light_dut.rs");

/// Reference model: phase 0=Green, 1=Yellow, 2=Red, 3=RedYellow.
fn next(phase: u8, timer: u8, request: bool) -> (u8, u8) {
    match phase {
        0 if request => (1, 0),
        0 => (0, 0),
        1 if timer < 1 => (1, timer + 1),
        1 => (2, 0),
        2 if timer < 3 => (2, timer + 1),
        2 => (3, 0),
        _ => (0, 0),
    }
}

/// Moore outputs (red, yellow, green) for a phase.
fn outputs(phase: u8) -> (bool, bool, bool) {
    match phase {
        0 => (false, false, true),
        1 => (false, true, false),
        2 => (true, false, false),
        _ => (true, true, false),
    }
}

#[test]
fn traffic_light_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("traffic_light", DUT_SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (req_drv, req_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (red_o, red_obs) = wire::<Logic, MainClk>(Logic::Zero);
    let (yel_o, yel_obs) = wire::<Logic, MainClk>(Logic::Zero);
    let (grn_o, grn_obs) = wire::<Logic, MainClk>(Logic::Zero);
    let dhs = vec![red_o.dirty_handle(), yel_o.dirty_handle(), grn_o.dirty_handle()];
    exec.spawn_wired(traffic_light(clk.clone(), req_in, red_o, yel_o, grn_o), dhs);

    // Hold with no request, then request and ride the full cycle back to Green.
    let requests: Vec<bool> = std::iter::repeat(false)
        .take(2)
        .chain(std::iter::once(true))
        .chain(std::iter::repeat(false).take(12))
        .collect();

    // On each tick the DUT advances state, then drives the outputs from the
    // *new* phase — so the value observed after cycle i reflects the phase
    // after i updates.
    let (mut ref_phase, mut ref_timer) = (0u8, 0u8);

    for &req in requests.iter() {
        req_drv.write(logic(req));
        exec.tick_clock(&mut clk);

        let (np, nt) = next(ref_phase, ref_timer, req);
        ref_phase = np;
        ref_timer = nt;
        let (r, y, g) = outputs(ref_phase);

        eq.record(
            &[("request", &[logic(req)])],
            &[
                ("red", &[red_obs.read()]),
                ("yellow", &[yel_obs.read()]),
                ("green_out", &[grn_obs.read()]),
            ],
            &[
                ("red", &[logic(r)]),
                ("yellow", &[logic(y)]),
                ("green_out", &[logic(g)]),
            ],
        );
    }

    eq.finish();
}
