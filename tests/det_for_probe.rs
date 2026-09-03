//! The `for`-loop pattern detector (tests/fixtures/det_for_dut.rs) on a fixed
//! pattern (010) and a hand-chosen input stream, three ways:
//!   * the simulator against a hand model of the source's own control flow;
//!   * the transpiled SV under Verilator against the simulator, cycle for cycle.
//!
//! Written 2026-09-02 as a divergence pin: the RTL missed the detections at cycles
//! 9, 13 and 19 because a `break` out of the counted `for` was charged the hoisted
//! last-iteration tick (see `expand_counted_for` in control_extract.rs). Fixed the
//! same day; this test now asserts agreement, and the sweep covers the module too.
//! Kept because the stream is chosen (mismatch at every bit position, a retry
//! straight after a detection) where the sweep's is random.
mod common;
use common::{logic, verilator_available, verilator_command};
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/det_for_dut.rs");
const DUT_SRC: &str = include_str!("fixtures/det_for_dut.rs");

const N: usize = 3;

/// The source's control flow, walked by hand. `Match(i)`: this cycle compares the
/// input with pattern bit `i`. `Done`: the `for` completed; this cycle writes 1 and
/// takes the trailing tick. The observed value after the edge is the value written
/// this cycle, or the previous value when nothing was written (RegOut holds).
#[derive(Clone, Copy, Debug, PartialEq)]
enum St { Match(usize), Done }

fn model(st: St, rstn: bool, x: bool, p: &[bool; N], prev: bool) -> (St, bool) {
    match st {
        St::Match(0) => {
            if !rstn { return (St::Match(0), false); }
            if x == p[0] { (if N == 1 { St::Done } else { St::Match(1) }, false) } else { (St::Match(0), false) }
        }
        St::Match(i) => {
            if x == p[i] { (if i + 1 == N { St::Done } else { St::Match(i + 1) }, prev) } else { (St::Match(0), prev) }
        }
        St::Done => (St::Match(0), true),
    }
}

fn stream() -> Vec<(bool, bool)> {
    let mut v = vec![(false, false)];
    for &x in &[0,1,0, 0,1,0, 1, 0,0, 0,1,0, 0,1,1, 0,1,0, 1,1,0,1,0, 0,1,0, 0,0,0] {
        v.push((true, x == 1));
    }
    v
}

fn sim_trace() -> Vec<bool> {
    let p: [bool; N] = [false, true, false];
    let pat_bits = Bits::<N>::from_lit::<0b010>();
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
    let (pat_drv, pat_in) = wire::<Bits<N>, MainClk>(pat_bits);
    let (in_drv, in_port) = wire::<Logic, MainClk>(Logic::Zero);
    let (out_drv, out_obs) = registered_wire::<Logic, MainClk>(&clk, Logic::Zero);
    let dh = out_drv.dirty_handle();
    let reads = vec![rstn_in.wire_id(), pat_in.wire_id(), in_port.wire_id()];
    exec.spawn_wired(det_for::<N>(clk.clone(), rstn_in, pat_in, in_port, out_drv), vec![dh], reads);

    let mut st = St::Match(0);
    let mut prev = false;
    let mut out = Vec::new();
    for (c, &(rstn, x)) in stream().iter().enumerate() {
        rstn_drv.write(logic(rstn));
        in_drv.write(logic(x));
        pat_drv.write(pat_bits);
        exec.tick_clock(&mut clk);
        let (nst, obs) = model(st, rstn, x, &p, prev);
        st = nst; prev = obs;
        let sim = out_obs.read() == Logic::One;
        assert_eq!(sim, obs, "cycle {c}: simulator {sim} vs hand model of the source {obs}");
        out.push(sim);
    }
    out
}

/// Transpile the fixture and run it under Verilator on the same stream, `N = 3`.
fn sv_trace() -> Vec<bool> {
    let sv = copper_codegen::transpile_source(DUT_SRC, Some("det_for"), &copper_codegen::EmitConfig::default())
        .expect("transpile det_for");
    let work = std::env::temp_dir().join(format!("copper_det_for_probe_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("det_for.sv"), sv).unwrap();
    let s = stream();
    let rstn: Vec<String> = s.iter().map(|(r, _)| (*r as u8).to_string()).collect();
    let x: Vec<String> = s.iter().map(|(_, x)| (*x as u8).to_string()).collect();
    let tb = format!(
        "#include \"Vdet_for.h\"\n#include \"verilated.h\"\n#include <iostream>\n\
         int main(int c, char** v) {{ Verilated::commandArgs(c, v); Vdet_for* t = new Vdet_for();\n\
         int rstn[] = {{{}}}; int x[] = {{{}}};\n\
         t->clk = 0; t->pattern = 0b010; t->eval();\n\
         for (int i = 0; i < {}; i++) {{ t->rstn = rstn[i]; t->in_i = x[i];\n\
           t->clk = 0; t->eval(); t->clk = 1; t->eval(); std::cout << (int)t->out_o << std::endl; }}\n\
         return 0; }}\n",
        rstn.join(","), x.join(","), s.len()
    );
    std::fs::write(work.join("tb.cpp"), tb).unwrap();
    let out = verilator_command()
        .current_dir(&work)
        .args(["--cc", "--exe", "--build", "--top-module", "det_for", "-GN=3",
               "-Wno-DECLFILENAME", "-Wno-WIDTHEXPAND", "-CFLAGS", "-std=c++14", "det_for.sv", "tb.cpp"])
        .output()
        .unwrap();
    assert!(out.status.success(), "verilator build failed:\n{}", String::from_utf8_lossy(&out.stderr));
    let run = std::process::Command::new(work.join("obj_dir/Vdet_for")).output().unwrap();
    let trace: Vec<bool> = String::from_utf8_lossy(&run.stdout).lines().map(|l| l.trim() == "1").collect();
    let _ = std::fs::remove_dir_all(&work);
    trace
}

#[test]
fn det_for_simulator_matches_hand_model_of_the_source() {
    let sim = sim_trace();
    // Five detections on this stream, at the cycles the hand model predicts.
    let fired: Vec<usize> = sim.iter().enumerate().filter(|&(_, &v)| v).map(|(c, _)| c).collect();
    assert_eq!(fired, vec![4, 9, 13, 19, 24]);
}

#[test]
fn det_for_rtl_matches_the_simulator_cycle_for_cycle() {
    if !verilator_available() {
        return;
    }
    let sim = sim_trace();
    let sv = sv_trace();
    assert_eq!(sim.len(), sv.len());
    let diverging: Vec<usize> = (0..sim.len()).filter(|&c| sim[c] != sv[c]).collect();
    // Before the 2026-09-02 fix this list was [9, 13, 19]: the three detections
    // that follow a mismatch-and-retry, each lost to the extra cycle the retry cost.
    assert_eq!(
        diverging,
        Vec::<usize>::new(),
        "sim {:?}\nsv  {:?}",
        sim.iter().map(|&b| b as u8).collect::<Vec<_>>(),
        sv.iter().map(|&b| b as u8).collect::<Vec<_>>()
    );
}
