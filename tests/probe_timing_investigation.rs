//! Timing-reconciliation investigation (EXECUTION_MODEL_RECONCILIATION.md):
//! which execution model is accurate to hardware for values that cross a tick?
//!
//! Method (the `mac_fsm` strategy): compare four traces of the SAME intent —
//! multi-tick `probe` vs explicit single-tick `probe_fsm`, each in the simulator
//! and as transpiled Verilog. Single-tick sim==verilog is the agreed category, so
//! `probe_fsm` is the reference-quality hardware witness. Whichever sampling
//! schedule it produces is the hardware-accurate answer.
//!
//! Run with:  cargo test --test probe_timing_investigation -- --nocapture
mod common;
use common::EquivalenceTest;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;
struct MainClk;
impl ClockDomain for MainClk {}
include!("fixtures/probe_timing_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/probe_timing_dut.rs");

const INPUTS: &[u8] = &[10, 11, 12, 13, 14, 15, 16];

/// Run one module in the simulator and collect its `out` trace over `INPUTS`.
macro_rules! sim_trace {
    ($module:ident) => {{
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();
        let (inp_drv, inp_in) = wire::<Bits<8>, MainClk>(Bits::zero());
        let (out_drv, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
        let dh = out_drv.dirty_handle();
        exec.spawn_wired($module(clk.clone(), inp_in, out_drv), vec![dh]);
        let mut trace = Vec::new();
        for &v in INPUTS {
            inp_drv.write(Bits::from_u8(v));
            exec.tick_clock(&mut clk);
            trace.push(out_obs.read().as_u8());
        }
        trace
    }};
}

/// Distinct successive nonzero values in a held-output trace = sampling schedule.
fn samples(trace: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for &v in trace {
        if v != 0 && out.last() != Some(&v) {
            out.push(v);
        }
    }
    out
}

#[test]
fn probe_timing_four_way() {
    let probe_sim = sim_trace!(probe);
    let probe_fsm_sim = sim_trace!(probe_fsm);

    eprintln!("inputs driven        : {INPUTS:?}");
    eprintln!("probe      (sim)     : {probe_sim:?}");
    eprintln!("probe_fsm  (sim)     : {probe_fsm_sim:?}");
    eprintln!("probe      verilog   : [10, 10, 12, 12, 14, 14, 16]   (hand-traced)");
    eprintln!("probe_fsm  verilog   : [0, 10, 10, 12, 12, 14, 14]    (hand-traced)");
    eprintln!("\nSampled-input schedule (the disputed axis):");
    eprintln!("  probe     (sim)    samples: {:?}", samples(&probe_sim));
    eprintln!("  probe_fsm (sim)    samples: {:?}", samples(&probe_fsm_sim));
    eprintln!("  transpiler (both)  samples: [10, 12, 14, 16]");

    // Regression guard: the simulator must sample on the hardware schedule
    // (10,12,14,… — not the old one-cycle-early 10,11,13,…). Under atomic-instant
    // output timing the trace is registered (+1), so the last sample can fall off
    // the fixed-length trace; check the schedule prefix.
    assert_eq!(&samples(&probe_sim)[..3], &[10, 12, 14], "probe read timing regressed");
    assert_eq!(&samples(&probe_fsm_sim)[..3], &[10, 12, 14], "probe_fsm read timing regressed");
}

/// Cross-check demonstrating the discrepancy: even the SINGLE-tick `probe_fsm`
/// (phase-gated cross-tick read) has sim != transpiled Verilog — so the "1 tick →
/// agree" boundary is really "no phase-gated cross-tick read". Intentionally
/// failing (verilator: FAIL); kept as an executable record of the finding.
#[test]
#[ignore = "demonstrates the sim-reads-one-cycle-early gap (verilator FAIL) — see EXECUTION_MODEL_RECONCILIATION.md"]
fn probe_fsm_sim_matches_verilog() {
    let mut eq = EquivalenceTest::for_module("probe_fsm", DUT_SRC, Some("probe_fsm"));
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (inp_drv, inp_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (out_drv, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_drv.dirty_handle();
    exec.spawn_wired(probe_fsm(clk.clone(), inp_in, out_drv), vec![dh]);
    // Reference = the module's own simulator output (this test asks only whether
    // the transpiled Verilog agrees with the sim for the single-tick coding).
    let mut sim_out = Vec::new();
    for &v in INPUTS {
        inp_drv.write(Bits::from_u8(v));
        exec.tick_clock(&mut clk);
        sim_out.push(out_obs.read());
    }
    // Re-run through the harness (fresh executor) recording sim==expected.
    let mut clk2 = Clock::<MainClk>::new();
    let mut exec2 = HardwareExecutor::new();
    let (inp_drv2, inp_in2) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (out_drv2, out_obs2) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh2 = out_drv2.dirty_handle();
    exec2.spawn_wired(probe_fsm(clk2.clone(), inp_in2, out_drv2), vec![dh2]);
    for (i, &v) in INPUTS.iter().enumerate() {
        inp_drv2.write(Bits::from_u8(v));
        exec2.tick_clock(&mut clk2);
        let a = out_obs2.read();
        eq.record(
            &[("inp", &Bits::<8>::from_u8(v).as_array()[..])],
            &[("out", a.as_array())],
            &[("out", sim_out[i].as_array())],
        );
    }
    eq.finish();
}
