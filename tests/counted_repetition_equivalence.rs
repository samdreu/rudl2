//! Counted repetition — a `for` whose body does work, indexes by the loop
//! variable, and takes its clock boundary from an inner counted `for`.
//!
//! This is the UART serialiser's shape, and the reason `TODO` cause M is filed as
//! REPETITION rather than counted delay: a fix that only handles
//! `for _ in 0..N { tick }` leaves it rejected. `i` is a counter register here, so
//! `d.read()[i]` lowers to a dynamic bit select rather than an unrolled slice.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/counted_repetition_dut.rs");
const SRC: &str = include_str!("fixtures/counted_repetition_dut.rs");

/// Two full bytes, so the outer loop's wrap is covered as well as its body.
const CYCLES: usize = 32;
const BYTE: u8 = 0xB4; // 1011_0100 — asymmetric, so LSB-first vs MSB-first differ

#[test]
fn a_counted_repetition_serialises_lsb_first_and_matches_verilog() {
    let mut eq = EquivalenceTest::new("counted_repetition", SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (d_drv, d_in) = wire::<Bits<8>, MainClk>(Bits::from_u8(BYTE));
    let (s_out, s_obs) = registered_wire::<Logic, MainClk>(&clk, Logic::Zero);
    let dh = s_out.dirty_handle();
    exec.spawn_wired(counted_repetition(clk.clone(), d_in, s_out), vec![dh], vec![]);

    d_drv.write(Bits::<8>::from_u8(BYTE));

    let mut observed: Vec<bool> = Vec::new();
    for _ in 0..CYCLES {
        exec.tick_clock(&mut clk);
        let sb = s_obs.read();
        observed.push(sb == Logic::One);
        let db = Bits::<8>::from_u8(BYTE);
        eq.record(
            &[("d", &db.as_array()[..])],
            &[("serial", std::slice::from_ref(&sb))],
            &[("serial", std::slice::from_ref(&sb))],
        );
    }

    // Independent of the FSM this test exists to validate: whatever states the
    // transpiler allocated, the line must carry `BYTE` LSB-first, each bit held
    // for the two cycles the inner `for` waits, repeating every 16.
    let expected: Vec<bool> = (0..CYCLES)
        .map(|c| (BYTE >> ((c / 2) % 8)) & 1 == 1)
        .collect();
    assert_eq!(
        observed, expected,
        "serial line is not {BYTE:#010b} LSB-first at two cycles per bit\n\
         observed = {observed:?}\nexpected = {expected:?}"
    );

    eq.finish();
}
