//! `continue` in the top-level hardware loop — sim ≡ transpiled SystemVerilog.
//!
//! `TODO` cause O. A `continue` re-enters the loop head **in the same cycle**, and
//! the extracted FSM cannot say that: its shape is
//! `loop { match pc { … }; clk.tick().await; }`, so any `pc` transition costs a
//! tick. Control extraction emits a zero-time goto instead and
//! `splice_zero_time_gotos` inlines the target state's body once every body
//! exists — the same "inline the continuation rather than transition to it" rule
//! `break` already follows, applied to the back edge.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/continue_skip_dut.rs");
const SRC: &str = include_str!("fixtures/continue_skip_dut.rs");

#[test]
fn a_continue_costs_no_cycle_and_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("skip_on_halt", SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (a_drv, a_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (n_drv, n_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (o_out, o_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = o_out.dirty_handle();
    exec.spawn_wired(skip_on_halt(clk.clone(), a_in, n_in, o_out), vec![dh], vec![]);

    // `halt` is deliberately irregular, so the halted and non-halted periods
    // interleave and a one-cycle error cannot stay in phase by luck.
    let stim: [(bool, u8); 18] = [
        (false, 0x11), (false, 0x22), (false, 0x33),
        (true,  0x44), (false, 0x55), (false, 0x66),
        (true,  0x77), (true,  0x88), (false, 0x99),
        (false, 0xAA), (false, 0xBB), (true,  0xCC),
        (false, 0xDD), (false, 0xEE), (false, 0xFF),
        (true,  0x12), (false, 0x34), (false, 0x56),
    ];

    // Reference model, written from the SOURCE rather than from the FSM.
    // `phase` counts the `for` ticks already taken when the cycle begins:
    //   0 → this cycle is the first `for` tick,
    //   1 → the second,
    //   2 → the decision cycle: `halt` is read here, and either the back edge is
    //       taken (making this cycle the next `for`'s FIRST tick) or `o` is
    //       written and this is the trailing tick.
    let mut expected: Vec<u8> = Vec::new();
    let mut held = 0u8;
    let mut phase = 0u8;
    for (ab, v) in stim {
        match phase {
            0 => phase = 1,
            1 => phase = 2,
            _ => {
                if ab {
                    phase = 1; // the back edge costs nothing: this IS a `for` tick
                } else {
                    held = v;
                    phase = 0;
                }
            }
        }
        expected.push(held);
    }

    let mut observed: Vec<u8> = Vec::new();
    for (i, (ab, v)) in stim.into_iter().enumerate() {
        a_drv.write(Logic::from_bool(ab));
        n_drv.write(Bits::<8>::from_u8(v));
        exec.tick_clock(&mut clk);
        let ob = o_obs.read();
        observed.push(ob.as_u128() as u8);
        let al = Logic::from_bool(ab);
        let nb = Bits::<8>::from_u8(v);
        let exp = Bits::<8>::from_u8(expected[i]);
        eq.record(
            &[("halt", std::slice::from_ref(&al)), ("n", &nb.as_array()[..])],
            &[("o", &ob.as_array()[..])],
            &[("o", &exp.as_array()[..])],
        );
    }

    assert_eq!(
        observed, expected,
        "the SIMULATOR disagrees with the source-level model — a `continue` that \
         cost a cycle would look exactly like this.\nobserved = {observed:02X?}\n\
         expected = {expected:02X?}"
    );

    eq.finish();
}
