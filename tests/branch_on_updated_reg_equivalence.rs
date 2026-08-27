//! A register assigned and then branched on in the SAME segment — sim ≡ SV.
//!
//! `shir_lower::lower_stmt_list` forwards a segment's register assignments into
//! every `PortWrite` it lowers (`Forwarding::at_edge`), which is what makes
//! `n = n + 1; acc.write(n)` mean what it says (TODO cause L). It did NOT
//! forward them into an `if` condition or a `match` scrutinee, so a segment that
//! updates a register and then branches on it emitted the register's PRE-edge
//! value inside `always_ff` and fired a cycle late.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/branch_on_updated_reg_dut.rs");
const SRC: &str = include_str!("fixtures/branch_on_updated_reg_dut.rs");

#[test]
fn a_register_branched_on_after_its_own_update_matches_verilog() {
    let mut eq = EquivalenceTest::for_module(
        "branch_on_updated_reg",
        SRC,
        Some("branch_on_updated_reg"),
    );

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (n_drv, n_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (o_out, o_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = o_out.dirty_handle();
    exec.spawn_wired(branch_on_updated_reg(clk.clone(), n_in, o_out), vec![dh], vec![]);

    // A distinct byte every cycle, so a guard that fires one cycle early or late
    // captures a DIFFERENT value rather than the same one at a different time.
    let stim: [u8; 12] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC];

    // Reference model, written from the source rather than from the lowering:
    // `c` counts 1,2,3→0 and the write happens on the cycle where the
    // just-incremented `c` is 3 — cycles 2, 5, 8, 11 (0-based).
    let mut expected: Vec<u8> = Vec::new();
    let mut held = 0u8;
    let mut c: u8 = 0;
    for v in stim {
        c = c.wrapping_add(1);
        if c == 3 {
            held = v;
            c = 0;
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

/// The bit-select twin: the guard `c[0] == Logic::One` reads a single bit of
/// the just-updated register, so forwarding puts the `Index` on a compound
/// expression and the emitter must produce a select-legal form (the width-cast
/// `1'((c + 8'd1))`, since `(c + 8'd1)[0]` is illegal SV).
#[test]
fn a_bit_select_on_a_just_updated_register_matches_verilog() {
    let mut eq = EquivalenceTest::for_module(
        "branch_on_updated_reg_bit",
        SRC,
        Some("branch_on_updated_reg_bit"),
    );

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (n_drv, n_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (o_out, o_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = o_out.dirty_handle();
    exec.spawn_wired(branch_on_updated_reg_bit(clk.clone(), n_in, o_out), vec![dh], vec![]);

    let stim: [u8; 12] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC];

    // Reference model from the source: `c` counts 1,2,3,… and the write fires
    // on the cycles where the just-incremented `c` is odd — 0, 2, 4, … (0-based).
    let mut expected: Vec<u8> = Vec::new();
    let mut held = 0u8;
    let mut c: u8 = 0;
    for v in stim {
        c = c.wrapping_add(1);
        if c & 1 == 1 {
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
