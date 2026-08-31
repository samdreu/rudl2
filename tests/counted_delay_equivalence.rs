//! A tick inside a counted `for` — sim ≡ transpiled SystemVerilog.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/counted_delay_dut.rs");
const SRC: &str = include_str!("fixtures/counted_delay_dut.rs");

#[test]
fn a_counted_delay_matches_the_simulator() {
    let mut eq = EquivalenceTest::new("counted_delay", SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (n_drv, n_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (o_out, o_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = o_out.dirty_handle();
    exec.spawn_wired(counted_delay(clk.clone(), n_in, o_out), vec![dh], vec![]);

    let stim: [u8; 12] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC];

    // From the source: the write happens on cycles 0, 3, 6, 9 — the loop body is
    // one write and three ticks.
    let mut expected: Vec<u8> = Vec::new();
    let mut held = 0u8;
    for (i, v) in stim.into_iter().enumerate() {
        if i % 3 == 0 {
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
