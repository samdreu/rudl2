//! `while <cond> { … clk.tick().await; }` — sim ≡ transpiled SystemVerilog.
//!
//! The behavioural half of the `while` desugar. `copper-codegen/tests/while_loops.rs`
//! asserts that the `while` and `loop { if !c { break } … }` spellings emit
//! byte-identical SystemVerilog, which pins the transpiler side completely. What
//! that cannot show is that the **simulator** agrees: the `#[hardware]` macro
//! handles `while` natively and never sees the rewrite, so the two front ends
//! could in principle disagree about the same source. This checks they do not.
//!
//! The stimulus has runs of low `go` of length 2, 1 and 3, so a wait that held
//! for a fixed number of cycles instead of a variable one cannot pass.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/while_wait_dut.rs");
const SRC: &str = include_str!("fixtures/while_wait_dut.rs");

const GO: [bool; 12] = [
    false, false, true, false, true, true, false, false, false, true, true, false,
];

#[test]
fn a_while_wait_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("while_waiter", SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (g_drv, g_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (c_out, c_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = c_out.dirty_handle();
    exec.spawn_wired(while_waiter(clk.clone(), g_in, c_out), vec![dh], vec![]);

    // Reference: the module holds while `go` is low; each cycle with `go` high
    // completes one outer iteration, and the new count is visible the next cycle
    // (the output is a RegOut).
    let mut n = 0u8;
    let mut shown = 0u8;

    for &g in &GO {
        g_drv.write(Logic::from_bool(g));
        exec.tick_clock(&mut clk);

        let expected = shown;
        if g {
            n = n.wrapping_add(1);
        }
        shown = n;

        let gl = Logic::from_bool(g);
        let cb = c_obs.read();
        let eb = Bits::<8>::from_u8(expected);
        eq.record(
            &[("go", std::slice::from_ref(&gl))],
            &[("count", &cb.as_array()[..])],
            &[("count", &eb.as_array()[..])],
        );
    }

    eq.finish();
}
