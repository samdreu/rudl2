//! Memory in a module that needs **control extraction**: sim ≡ transpiled
//! SystemVerilog.
//!
//! Until 2026-08-25 this combination did not exist, because it could not be
//! written. `chir_lower::validate_memory_usage` decided the memory staging rules
//! over the *segments* the lowered loop body splits into at `clk.tick().await` —
//! and `control_extract` runs first, rewriting any body whose ticks live inside
//! branches or loops into a single-tick `match pc` FSM. Every access in the module
//! then landed in one segment, so an ordinary ROM read
//!
//! ```text
//! loop { rom.read(addr); tick; data.write(rom.data()); }
//! ```
//!
//! — which transpiles — was refused with *"read before the `clk.tick().await` that
//! produces it"* the moment anything else in the module needed extraction. The
//! rules now live in `copper_analysis::check_memory_staging`, on the source, where
//! the ticks are still there to be counted.
//!
//! The third DUT is the sibling rule the same move exposed: two writes to one port
//! on exclusive branches are a multiplexer, not a bus conflict, and the per-phase
//! COUNT that used to decide it said otherwise.
//!
//! Acceptance is not correctness, which is what this file is for: every DUT is run
//! in the simulator against an independent reference model AND Verilated against
//! the emitted SystemVerilog, one cycle at a time.

mod common;

use common::{logic, EquivalenceTest};
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain, Logic, Memory};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/extracted_rom_dut.rs");
const SRC: &str = include_str!("fixtures/extracted_rom_dut.rs");

/// The ROM's contents, stated once — the same rule the fixture's `from_fn` states.
fn rom(i: usize) -> u16 {
    (i * 3 + 7) as u16
}

/// Distinct addresses, so a wrong pipeline depth or a wrong phase shows up as a
/// wrong word rather than an accidentally-equal one.
const ADDRS: [usize; 12] = [0, 5, 2, 9, 15, 1, 7, 3, 11, 4, 13, 6];

/// `rom_paced` staged its address on the cycles where `pc == 0` — every third one,
/// since the counted `for` adds two more boundaries. The edge that ends such a
/// cycle captures the word; the next cycle writes it to the `RegOut`, which commits
/// at *its* edge. So a word staged on cycle `i` is observable from cycle `i + 1`
/// and holds until the next one lands.
fn paced_expected() -> Vec<u16> {
    let mut q = 0u16;
    let mut staged = 0usize;
    ADDRS
        .iter()
        .enumerate()
        .map(|(i, &a)| {
            match i % 3 {
                0 => staged = a,
                1 => q = rom(staged),
                _ => {}
            }
            q
        })
        .collect()
}

#[test]
fn a_counted_pause_around_a_rom_read_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("rom_paced", SRC, Some("rom_paced"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (a_drv, a_in) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (d_out, d_obs) = registered_wire::<Bits<16>, MainClk>(&clk, Bits::zero());
    let dh = d_out.dirty_handle();
    exec.spawn_wired(rom_paced(clk.clone(), a_in, d_out), vec![dh], vec![]);

    let expected = paced_expected();
    for (i, &a) in ADDRS.iter().enumerate() {
        a_drv.write(Bits::<4>::from_usize(a));
        exec.tick_clock(&mut clk);

        let ab = Bits::<4>::from_usize(a);
        let db = d_obs.read();
        let exp = Bits::<16>::from_u16(expected[i]);
        eq.record(
            &[("addr", &ab.as_array()[..])],
            &[("data", &db.as_array()[..])],
            &[("data", &exp.as_array()[..])],
        );
    }

    eq.finish();
}

/// `go` low holds the design in its wait, so the staging cycles are chosen by the
/// stimulus rather than by a counter — the second way a tick gets nested (a
/// data-dependent `while`) and the one a counter cannot imitate.
const GO: [bool; 12] = [
    false, true, true, false, false, true, false, true, true, true, false, false,
];

/// A cycle whose `go` is high tests, falls out of the wait and stages its address
/// in that same cycle (no tick separates the test from the `read`); the following
/// cycle observes the word and writes the `RegOut`, which commits at its edge. So
/// the output on cycle `i` is the word addressed on cycle `i - 1`, and holds
/// otherwise.
fn gated_expected() -> Vec<u16> {
    let mut q = 0u16;
    (0..ADDRS.len())
        .map(|i| {
            if i > 0 && GO[i - 1] {
                q = rom(ADDRS[i - 1]);
            }
            q
        })
        .collect()
}

#[test]
fn a_data_dependent_wait_around_a_rom_read_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("rom_gated", SRC, Some("rom_gated"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (g_drv, g_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (a_drv, a_in) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (d_out, d_obs) = registered_wire::<Bits<16>, MainClk>(&clk, Bits::zero());
    let dh = d_out.dirty_handle();
    // The wire ids have to be taken before the ports move into the module.
    let reads = vec![g_in.wire_id(), a_in.wire_id()];
    exec.spawn_wired(rom_gated(clk.clone(), g_in, a_in, d_out), vec![dh], reads);

    let expected = gated_expected();
    for (i, &a) in ADDRS.iter().enumerate() {
        g_drv.write(logic(GO[i]));
        a_drv.write(Bits::<4>::from_usize(a));
        exec.tick_clock(&mut clk);

        let ab = Bits::<4>::from_usize(a);
        let gb = logic(GO[i]);
        let db = d_obs.read();
        let exp = Bits::<16>::from_u16(expected[i]);
        eq.record(
            &[("go", &[gb][..]), ("addr", &ab.as_array()[..])],
            &[("data", &db.as_array()[..])],
            &[("data", &exp.as_array()[..])],
        );
    }

    eq.finish();
}

/// `sel` picks which branch drives the write bus; the address it does NOT pick is
/// never driven in that cycle, which is the whole claim under test.
const SEL: [bool; 12] = [
    true, true, false, true, false, false, true, true, false, true, true, false,
];

/// The address the `else` arm writes — a different one, so which arm drove the bus
/// is visible in the array afterwards.
fn alt_addr(i: usize) -> usize {
    15 - (ADDRS[i] % 16)
}

/// The ROM-less reference: a 16-word array, ReadFirst (the read sees the word the
/// same cycle's write replaces), and a `RegOut` that shows the captured word one
/// cycle after the edge that captured it.
fn arms_expected() -> Vec<u8> {
    let mut mem = [0u8; 16];
    let mut prev = 0u8;
    (0..ADDRS.len())
        .map(|i| {
            let a = ADDRS[i] % 16;
            let d = (i as u8) * 7 + 1;
            // ReadFirst: the edge captures the word this cycle's write is about to
            // replace. The capture is written to the `RegOut` in the next cycle,
            // which is why the output lags the address by exactly one.
            let captured = mem[a];
            if SEL[i] {
                mem[a] = d;
            } else {
                mem[alt_addr(i)] = d.wrapping_add(1);
            }
            let out = prev;
            prev = captured;
            out
        })
        .collect()
}

#[test]
fn exclusive_arm_writes_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("ram_arms", SRC, Some("ram_arms"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (s_drv, s_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (a_drv, a_in) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (b_drv, b_in) = wire::<Bits<4>, MainClk>(Bits::zero());
    let (d_drv, d_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (o_out, o_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = o_out.dirty_handle();
    let reads = vec![s_in.wire_id(), a_in.wire_id(), b_in.wire_id(), d_in.wire_id()];
    exec.spawn_wired(
        ram_arms(clk.clone(), s_in, a_in, b_in, d_in, o_out),
        vec![dh],
        reads,
    );

    let expected = arms_expected();
    for i in 0..ADDRS.len() {
        let a = ADDRS[i] % 16;
        let d = (i as u8) * 7 + 1;
        s_drv.write(logic(SEL[i]));
        a_drv.write(Bits::<4>::from_usize(a));
        b_drv.write(Bits::<4>::from_usize(alt_addr(i)));
        d_drv.write(Bits::<8>::from_u8(d));
        exec.tick_clock(&mut clk);

        let sb = logic(SEL[i]);
        let ab = Bits::<4>::from_usize(a);
        let bb = Bits::<4>::from_usize(alt_addr(i));
        let db = Bits::<8>::from_u8(d);
        let ob = o_obs.read();
        let exp = Bits::<8>::from_u8(expected[i]);
        eq.record(
            &[
                ("sel", &[sb][..]),
                ("a", &ab.as_array()[..]),
                ("b", &bb.as_array()[..]),
                ("d", &db.as_array()[..]),
            ],
            &[("o", &ob.as_array()[..])],
            &[("o", &exp.as_array()[..])],
        );
    }

    eq.finish();
}
