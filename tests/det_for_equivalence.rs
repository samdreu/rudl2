//! Equivalence test for `det_for` — the `for`-loop pattern detector — on the same
//! coverage discipline as det_010: every mismatch position, a retry straight after
//! a detection, and a reset both between attempts and in the middle of one. Three
//! legs, as `EquivalenceTest` runs them: the simulator against a hand model of the
//! source, and the transpiled SystemVerilog against the simulator under Verilator,
//! at `N = 3`.
//!
//! The runnable twin with the same model is `examples/sequential/pattern_detector_for.rs`;
//! `tests/det_for_probe.rs` is the (retired) divergence pin that found the
//! extraction bug this module exposed on 2026-09-02.
mod common;
use common::{logic, EquivalenceTest};
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/det_for_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/det_for_dut.rs");

const N: usize = 3;

#[derive(Clone, Copy, PartialEq)]
enum St { Match(usize), Done }

/// The source's control flow by hand; reset is read only between attempts.
fn step(st: St, rstn: bool, x: bool, p: &[bool; N], prev: bool) -> (St, bool) {
    match st {
        St::Match(0) => {
            if !rstn { return (St::Match(0), false); }
            if x == p[0] { (St::Match(1), false) } else { (St::Match(0), false) }
        }
        St::Match(i) => {
            if x == p[i] { (if i + 1 == N { St::Done } else { St::Match(i + 1) }, prev) } else { (St::Match(0), prev) }
        }
        St::Done => (St::Match(0), true),
    }
}

fn coverage_stream() -> Vec<(bool, bool)> {
    let mut v = vec![(false, false)];
    let bits: &[(u8, u8)] = &[
        (1, 0), (1, 1), (1, 0), // cycles 1-3: match            -> fires at 4
        (1, 1),                 // cycle 4: consumed by the detection cycle
        (1, 1),                 // cycle 5: mismatch at bit 0
        (1, 0), (1, 0),         // cycles 6-7: mismatch at bit 1
        (1, 0), (1, 1), (1, 1), // cycles 8-10: mismatch at bit 2
        (1, 0), (1, 1), (1, 0), // cycles 11-13: match          -> fires at 14
        (1, 0),                 // cycle 14: consumed by the detection cycle
        (1, 0), (0, 1), (1, 0), // cycles 15-17: match with reset asserted MID-attempt
                                //   (ignored: rstn is read only between attempts) -> fires at 18
        (0, 0),                 // cycle 18: consumed by the detection cycle (reset ignored there too)
        (0, 0),                 // cycle 19: reset between attempts
        (1, 0), (1, 1), (1, 0), // cycles 20-22: match          -> fires at 23
        (1, 1),                 // cycle 23: consumed by the detection cycle
        (1, 1),                 // cycle 24: idle
    ];
    v.extend(bits.iter().map(|&(r, x)| (r == 1, x == 1)));
    v
}

#[test]
fn det_for_sim_matches_model_and_transpiled_verilog() {
    let p: [bool; N] = [false, true, false];
    let pat = Bits::<N>::from_lit::<0b010>();

    let mut eq = EquivalenceTest::new("det_for", DUT_SRC).with_params(&[("N", N as i64)]);
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
    let (pat_drv, pat_in) = wire::<Bits<N>, MainClk>(pat);
    let (in_drv, in_port) = wire::<Logic, MainClk>(Logic::Zero);
    let (out_drv, out_obs) = registered_wire::<Logic, MainClk>(&clk, Logic::Zero);
    let dh = out_drv.dirty_handle();
    let reads = vec![rstn_in.wire_id(), pat_in.wire_id(), in_port.wire_id()];
    exec.spawn_wired(det_for::<N>(clk.clone(), rstn_in, pat_in, in_port, out_drv), vec![dh], reads);

    let mut st = St::Match(0);
    let mut prev = false;
    for &(rstn, x) in &coverage_stream() {
        rstn_drv.write(logic(rstn));
        in_drv.write(logic(x));
        pat_drv.write(pat);
        exec.tick_clock(&mut clk);
        let (nst, expected) = step(st, rstn, x, &p, prev);
        st = nst;
        prev = expected;
        eq.record(
            &[("rstn", &[logic(rstn)]), ("pattern", pat.as_array().as_slice()), ("in_i", &[logic(x)])],
            &[("out_o", &[out_obs.read()])],
            &[("out_o", &[logic(expected)])],
        );
    }
    eq.finish();
}
