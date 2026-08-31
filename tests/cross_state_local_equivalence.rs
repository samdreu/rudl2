//! A local captured in one FSM state and read in another — sim ≡ transpiled
//! SystemVerilog.
//!
//! Control extraction flattens a body into `match pc` arms, and an arm scopes
//! its own locals, so a `let` in one state read from another was reported as an
//! undefined variable. The same body without the wait — so without extraction —
//! has always transpiled and made the local a register. That is the language's
//! central rule ("every value live across an await becomes a register"), so the
//! two lowering paths disagreed about the one thing they must not.
//!
//! Cross-state locals are now hoisted to pre-loop `let mut` declarations, which
//! the existing register path already handles — the same treatment `pc` gets.
//!
//! Why this test and not just a shape assertion: the register has to **hold**
//! between the capturing state and the reading state. The stimulus changes `d`
//! while the module is in between, so a `captured` that tracked `d` rather than
//! holding it would produce a different trace — and the transpiled module and
//! the simulator would have to agree on the wrong answer to pass.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/cross_state_local_dut.rs");
const SRC: &str = include_str!("fixtures/cross_state_local_dut.rs");

#[test]
fn a_cross_state_local_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::new("capture_after_wait", SRC);

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (g_drv, g_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (d_drv, d_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (o_out, o_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = o_out.dirty_handle();
    exec.spawn_wired(
        capture_after_wait(clk.clone(), g_in, d_in, o_out),
        vec![dh],
        vec![],
    );

    // `d` changes on every cycle, including the cycles between the capture and
    // the read — the capture must survive those changes.
    let stim: [(bool, u8); 14] = [
        (false, 0x11),
        (false, 0x22),
        (true, 0x33), // capture 0x33 here
        (false, 0x44), // d moves on while the value is held
        (false, 0x55),
        (true, 0x66), // capture 0x66
        (false, 0x77),
        (true, 0x88), // capture 0x88
        (false, 0x99),
        (false, 0xAA),
        (true, 0xBB),
        (false, 0xCC),
        (true, 0xDD),
        (false, 0xEE),
    ];

    let captured_values: Vec<u8> = stim.iter().filter(|(g, _)| *g).map(|(_, d)| *d).collect();
    let mut observed: Vec<u8> = Vec::new();

    for (g, dv) in stim {
        g_drv.write(Logic::from_bool(g));
        d_drv.write(Bits::<8>::from_u8(dv));
        exec.tick_clock(&mut clk);

        let gl = Logic::from_bool(g);
        let db = Bits::<8>::from_u8(dv);
        let ob = o_obs.read();
        observed.push(ob.as_u128() as u8);
        // The reference column is the simulator itself: the claim under test is
        // that the two LOWERING PATHS agree, which is what Verilating the
        // transpiled module against this trace checks. The independent property
        // — that a captured value is actually held — is asserted below, since
        // no per-cycle reference model can state it without hand-tracing the FSM
        // this test exists to validate.
        eq.record(
            &[("go", std::slice::from_ref(&gl)), ("d", &db.as_array()[..])],
            &[("o", &ob.as_array()[..])],
            &[("o", &ob.as_array()[..])],
        );
    }

    // Independent of the DUT's structure: every value the output ever takes must
    // be one the module was told to CAPTURE (or the reset 0). If `captured` had
    // tracked `d` instead of holding, values like 0x44 or 0x55 — present on `d`
    // between a capture and its read — would show up here.
    for v in &observed {
        assert!(
            *v == 0 || captured_values.contains(v),
            "output took value 0x{v:02X}, which was never captured; observed = {observed:02X?}, \
             captured = {captured_values:02X?}"
        );
    }
    assert!(
        observed.iter().any(|v| *v != 0),
        "the output never changed — nothing was captured at all: {observed:02X?}"
    );

    eq.finish();
}
