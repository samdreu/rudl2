//! A `match` arm that drives an output while the wildcard arm does not is an
//! ENABLED REGISTER — the output holds on the cycles the arm does not fire.
//!
//! `vlir_lower::hoist_moore_output_defaults` gives a Moore output decoded from a
//! state `case` a default at the top of the block, so an encoding the state
//! register can never reach does not make the output look conditional and get
//! wrongly registered. It decided "driven on every path" by intersecting the
//! explicit arms only — ignoring the `default` arm, which is the one path that IS
//! reachable from the source. `match s { One => o.write(n), _ => {} }` therefore
//! got an unconditional `o = n` hoisted over it and was driven every cycle, with
//! the hold silently gone.
//!
//! This DUT also pins the `match` half of the forwarding seam (`TODO` cause N):
//! `s` is assigned through the two arms of an `if` — so its forwarded value is a
//! merged mux — and then used as the scrutinee in the same segment.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/match_on_updated_reg_dut.rs");
const MATCH_SRC: &str = include_str!("fixtures/match_on_updated_reg_dut.rs");

#[test]
fn a_match_scrutinee_reads_the_registers_own_update() {
    let mut eq = EquivalenceTest::new("match_on_updated_reg", MATCH_SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (n_drv, n_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (o_out, o_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = o_out.dirty_handle();
    exec.spawn_wired(match_on_updated_reg(clk.clone(), n_in, o_out), vec![dh], vec![]);

    let stim: [u8; 10] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA];

    // From the source: `s` is Zero on entry, so the toggle makes it One on the
    // even cycles, and those are the cycles the arm fires on.
    let mut expected: Vec<u8> = Vec::new();
    let mut held = 0u8;
    for (i, v) in stim.into_iter().enumerate() {
        if i % 2 == 0 {
            held = v;
        }
        expected.push(held);
    }

    let mut observed: Vec<u8> = Vec::new();
    for (i, v) in stim.into_iter().enumerate() {
        n_drv.write(Bits::<8>::from_u8(v));
        exec.tick_clock(&mut clk);
        let ob = o_obs.read();
        observed.push(ob.as_u128() as u8);
        let nb = Bits::<8>::from_u8(v);
        let exp = Bits::<8>::from_u8(expected[i]);
        eq.record(
            &[("n", &nb.as_array()[..])],
            &[("o", &ob.as_array()[..])],
            &[("o", &exp.as_array()[..])],
        );
    }

    assert_eq!(
        observed, expected,
        "the SIMULATOR disagrees with the source-level model: observed = {observed:02X?}, \
         expected = {expected:02X?}"
    );

    eq.finish();
}
