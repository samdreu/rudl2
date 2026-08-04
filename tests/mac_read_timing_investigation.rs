//! Generalizes the probe timing finding to the original motivating case,
//! `mac_pipeline` (3 ticks/loop, multiple cross-tick reads). Dense stimulus:
//! b=1, c=0, distinct a each cycle, so `out` reveals exactly which `a` the design
//! sampled. Compare against the independent hand-written Verilog MAC (samples
//! 10,13,16 at cycles 0,3,6) and the transpiler (phase-0 edges 0,3,6).
//! Run with: cargo test --test mac_read_timing_investigation -- --nocapture
mod common;
use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;
struct MainClk;
impl ClockDomain for MainClk {}
include!("fixtures/mac_pipeline_dut.rs");

#[test]
fn mac_pipeline_sim_read_cadence() {
    let a_vals: &[u8] = &[10, 11, 12, 13, 14, 15, 16, 17, 18];
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (a_drv, a_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (b_drv, b_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (c_drv, c_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (out_drv, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = out_drv.dirty_handle();
    let reads = vec![a_in.wire_id(), b_in.wire_id(), c_in.wire_id()];
    exec.spawn_wired(mac_pipeline(clk.clone(), a_in, b_in, c_in, out_drv), vec![dh], reads);

    let mut trace = Vec::new();
    for &av in a_vals {
        a_drv.write(Bits::from_u8(av));
        b_drv.write(Bits::from_u8(1));
        c_drv.write(Bits::from_u8(0));
        exec.tick_clock(&mut clk);
        trace.push(out_obs.read().as_u8());
    }
    let samples: Vec<u8> = {
        let mut v = Vec::new();
        for &x in &trace {
            if x != 0 && v.last() != Some(&x) {
                v.push(x);
            }
        }
        v
    };
    eprintln!("a driven               : {a_vals:?}");
    eprintln!("mac_pipeline (sim) out : {trace:?}");
    eprintln!("  sim  samples a       : {samples:?}");
    eprintln!("  hand Verilog samples : [10, 13, 16]  (cycles 0,3,6)");
    eprintln!("  transpiler samples   : [10, 13, 16]  (phase-0 edges 0,3,6)");
}
