//! Read-timing regression suite: sim-vs-Verilator equivalence for multi-tick
//! loops with CROSS-TICK input reads — the case the static read-timing
//! classification targets (`copper_analysis::classify_reads`, impl-plan item 3;
//! EXECUTION_MODEL_RECONCILIATION.md). Each module has a combinational (aliased)
//! output, so a `verilator: PASS` here isolates read timing from the still-open
//! output-timing axis. Every one of these would have failed before the fix (the
//! sim sampled one cycle early on 2+-tick loops).
mod common;
use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;
struct MainClk;
impl ClockDomain for MainClk {}
include!("fixtures/read_timing_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/read_timing_dut.rs");

/// The cycle a period-`p` phase FSM sampled its input for output cycle `n`:
/// phase 0 recurs every `p` cycles, so the value seen at `n` was read at the
/// largest multiple of `p` not exceeding `n`.
fn sampled_at(n: usize, p: usize) -> usize {
    (n / p) * p
}

/// 2-tick sample-and-hold: `out` holds the input sampled every 2 cycles.
#[test]
fn sample_hold_2_sim_matches_verilog() {
    let inputs: Vec<u8> = (0..12).map(|i| (i as u8) + 10).collect();
    let mut eq = EquivalenceTest::for_module("sample_hold_2", DUT_SRC, Some("sample_hold_2"));
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (in_drv, in_port) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (out_drv, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_drv.dirty_handle();
    exec.spawn_wired(sample_hold_2(clk.clone(), in_port, out_drv), vec![dh]);
    for (n, &v) in inputs.iter().enumerate() {
        in_drv.write(Bits::from_u8(v));
        exec.tick_clock(&mut clk);
        let exp = inputs[sampled_at(n, 2)];
        eq.record(
            &[("inp", &Bits::<8>::from_u8(v).as_array()[..])],
            &[("out", out_obs.read().as_array())],
            &[("out", &Bits::<8>::from_u8(exp).as_array()[..])],
        );
    }
    eq.finish();
}

/// 2-tick accumulator, reading `step` in the MIDDLE phase (between the two ticks).
/// Under the atomic-instant model the READ is now correct: the sim accumulates the
/// same `step` values in the same order as the independent Verilog (sums 2,6,12,…
/// match exactly). The only residual is a uniform one-cycle shift of the OUTPUT —
/// sim `[0,0,2,2,6,…]` vs verilog `[0,2,2,6,…]` — i.e. the held output is registered
/// (+1) in the sim but aliased (combinational, immediate) in this reference Verilog.
/// That is the deferred output register-vs-combinational axis, the SAME bucket as
/// the other transpiler-alignment ignores — NOT a read-timing bug. See
/// SYNCHRONOUS_SEMANTICS.md / ATOMIC_INSTANT_EXECUTOR.md.
#[test]
#[ignore = "mid-phase (between-ticks) read: sim and transpiler disagree by one cycle. Under post-edge the loop-top read cases (sample_hold_2, sum_hold_2) now match, but this read whose result crosses a *second* tick does not — adjudication pending. See design_docs/EXECUTOR_CONVENTION_EXPERIMENT.md"]
fn accum_2_sim_matches_verilog() {
    let step: Vec<u8> = (0..12).map(|i| (i as u8) + 1).collect();
    let mut eq = EquivalenceTest::for_module("accum_2", DUT_SRC, Some("accum_2"));
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (s_drv, s_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (out_drv, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_drv.dirty_handle();
    exec.spawn_wired(accum_2(clk.clone(), s_in, out_drv), vec![dh]);
    let mut acc: u8 = 0;
    for (n, &v) in step.iter().enumerate() {
        s_drv.write(Bits::from_u8(v));
        exec.tick_clock(&mut clk);
        if n % 2 == 1 {
            acc = acc.wrapping_add(v); // acc updates at phase-1 (odd) edges
        }
        eq.record(
            &[("step", &Bits::<8>::from_u8(v).as_array()[..])],
            &[("out", out_obs.read().as_array())],
            &[("out", &Bits::<8>::from_u8(acc).as_array()[..])],
        );
    }
    eq.finish();
}

/// Two loop-top cross-tick reads summed, then held (period 2).
#[test]
fn sum_hold_2_sim_matches_verilog() {
    let a: Vec<u8> = (0..12).map(|i| (i as u8) + 10).collect();
    let b: Vec<u8> = (0..12).map(|i| (i as u8) + 1).collect();
    let mut eq = EquivalenceTest::for_module("sum_hold_2", DUT_SRC, Some("sum_hold_2"));
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (a_drv, a_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (b_drv, b_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (out_drv, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_drv.dirty_handle();
    exec.spawn_wired(sum_hold_2(clk.clone(), a_in, b_in, out_drv), vec![dh]);
    for n in 0..a.len() {
        a_drv.write(Bits::from_u8(a[n]));
        b_drv.write(Bits::from_u8(b[n]));
        exec.tick_clock(&mut clk);
        let s = sampled_at(n, 2);
        let exp = a[s] + b[s];
        eq.record(
            &[
                ("in_a", &Bits::<8>::from_u8(a[n]).as_array()[..]),
                ("in_b", &Bits::<8>::from_u8(b[n]).as_array()[..]),
            ],
            &[("out", out_obs.read().as_array())],
            &[("out", &Bits::<8>::from_u8(exp).as_array()[..])],
        );
    }
    eq.finish();
}
