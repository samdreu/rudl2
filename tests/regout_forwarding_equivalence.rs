//! **Cause L** — sequential forwarding into a `RegOut` drive: sim ≡ transpiled
//! SystemVerilog.
//!
//! A register assignment emits no statement on the pre-edge path; it becomes a
//! non-blocking update at the clock edge. A `RegOut` written afterwards is sampled
//! at that same edge, so without forwarding its drive reads the register's *pre-edge*
//! value and lands a full cycle late.
//!
//! The bug was invisible for as long as it was because the whole corpus writes its
//! `RegOut` BEFORE mutating — the write-before-tick Moore idiom (`waiter`,
//! `handshake`, the BaseJump modules) — which is the one ordering it does not touch.
//! It surfaced while building a DUT for cause K, and reproduces with no nested loop
//! and no control flow at all: measured at 4, 6, 14, 17 in the simulator against
//! 0, 4, 6, 14 under Verilator.
//!
//! ## Why only `RegOut`
//!
//! The rule is about where a drive is *evaluated*, not where it is written. A
//! `RegOut` becomes `acc <= <expr>` in `always_ff`, evaluated with pre-edge values →
//! forward it. A plain `Out` becomes a continuous `assign o = <expr>`, observed after
//! the edge when the flop already holds the assigned value → forwarding it applies
//! the update twice. Both directions are measured: `tests/lfsr_equivalence.rs` is the
//! plain-`Out` half and fails under Verilator if its drive is forwarded.
//!
//! ## The two cases that pinned down where the choice belongs (L-1, L-2)
//!
//! The rule above is stated in terms of the emitted form, not the port type, and
//! that distinction is forced rather than stylistic:
//!
//! * **L-1** — a plain `Out` written CONDITIONALLY is turned into an implicit-hold
//!   register in `always_ff` by `vlir_lower`, so it is sampled at the edge like a
//!   `RegOut` despite its declaration. An output that looks conditional can also be
//!   un-registered again by `hoist_moore_output_defaults`. So "is this drive sampled
//!   at the edge?" is not answerable from the SHIR alone, and the first fix — which
//!   keyed on `CHIRPort::registered` in `shir_lower` — got this case wrong.
//! * **L-2** — a `let` wire read after a register assignment lowers to a continuous
//!   `assign` over the flop, so a drive that samples it at the edge misses the
//!   assignment entirely. Forwarding has to see through the wire, which then leaves
//!   the wire dead; `-Wall` treats an assigned-but-unread signal as an error, so
//!   `vlir_lower::drop_unread_wires` removes it.
//!
//! Both are fixed by carrying BOTH forms on `SHIRStmt::PortDrive` and letting
//! `vlir_lower::split_output_reg` choose — the one point where a drive actually
//! becomes a non-blocking assignment.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, Out, RegOut};
use copper_core::types::Bits;
use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/regout_forwarding_dut.rs");
const SRC: &str = include_str!("fixtures/regout_forwarding_dut.rs");

/// Uneven, with a zero: a lowering that dropped, duplicated or shifted a cycle
/// cannot pass on a constant stimulus.
const STEP: [u8; 8] = [3, 1, 7, 0, 2, 9, 4, 5];

/// `n = n + step; acc.write(n);` — `acc` shows the step just applied, so at cycle i
/// it is the running sum INCLUDING `step[i]`. One missing forward shifts every value.
#[test]
fn assign_then_write_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("assign_then_write", SRC, Some("assign_then_write"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (s_drv, s_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (a_out, a_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = a_out.dirty_handle();
    exec.spawn_wired(assign_then_write(clk.clone(), s_in, a_out), vec![dh], vec![]);

    for (i, &st) in STEP.iter().enumerate() {
        s_drv.write(Bits::<8>::from_u8(st));
        exec.tick_clock(&mut clk);

        let expected = STEP[..=i].iter().fold(0u8, |a, &s| a.wrapping_add(s));
        let sb = Bits::<8>::from_u8(st);
        let ab = a_obs.read();
        let eb = Bits::<8>::from_u8(expected);
        eq.record(
            &[("step", &sb.as_array()[..])],
            &[("acc", &ab.as_array()[..])],
            &[("acc", &eb.as_array()[..])],
        );
    }

    eq.finish();
}

/// `acc.write(n); n = n + step;` — `acc` shows the value from BEFORE this cycle's
/// step, so cycle 0 is 0. The ordering that was always correct; here to prove the fix
/// did not simply shift the error to the other spelling.
#[test]
fn write_then_assign_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("write_then_assign", SRC, Some("write_then_assign"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (s_drv, s_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (a_out, a_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = a_out.dirty_handle();
    exec.spawn_wired(write_then_assign(clk.clone(), s_in, a_out), vec![dh], vec![]);

    for (i, &st) in STEP.iter().enumerate() {
        s_drv.write(Bits::<8>::from_u8(st));
        exec.tick_clock(&mut clk);

        let expected = STEP[..i].iter().fold(0u8, |a, &s| a.wrapping_add(s));
        let sb = Bits::<8>::from_u8(st);
        let ab = a_obs.read();
        let eb = Bits::<8>::from_u8(expected);
        eq.record(
            &[("step", &sb.as_array()[..])],
            &[("acc", &ab.as_array()[..])],
            &[("acc", &eb.as_array()[..])],
        );
    }

    eq.finish();
}

/// The sharpest guard: the two orderings are different programs, so they must not
/// emit the same module. They did — that identity IS the bug, and it is the thing
/// most likely to come back if the forwarding is ever dropped from a lowering path.
#[test]
fn the_two_orderings_do_not_emit_the_same_module() {
    let cfg = copper_codegen::EmitConfig::default();
    let after = copper_codegen::transpile_source(SRC, Some("assign_then_write"), &cfg)
        .expect("assign_then_write transpiles");
    let before = copper_codegen::transpile_source(SRC, Some("write_then_assign"), &cfg)
        .expect("write_then_assign transpiles");

    // Compare the bodies, not the text: the module names differ trivially.
    let body = |sv: &str| {
        sv.split("always_ff")
            .nth(1)
            .expect("a sequential module has an always_ff")
            .to_string()
    };
    assert_ne!(
        body(&after),
        body(&before),
        "`n = n + step; acc.write(n);` and `acc.write(n); n = n + step;` are different \
         programs; emitting one module for both is TODO cause L"
    );
    assert!(
        body(&after).contains("acc <= (n + step)"),
        "the assigned value must reach the registered drive, got:\n{after}"
    );
    assert!(
        body(&before).contains("acc <= n;"),
        "the write-before-tick ordering must keep the pre-edge value, got:\n{before}"
    );
}

/// **L-1** — a conditionally-driven plain `Out` is edge-sampled despite its port type.
#[test]
fn a_conditional_plain_out_sim_matches_transpiled_verilog() {
    let mut eq =
        EquivalenceTest::for_module("conditional_plain_out", SRC, Some("conditional_plain_out"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (s_drv, s_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (e_drv, e_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (o_out, o_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
    let dh = o_out.dirty_handle();
    exec.spawn_wired(
        conditional_plain_out(clk.clone(), s_in, e_in, o_out),
        vec![dh],
        vec![],
    );

    // `enable` alternates, so the HOLD cycles are exercised too: an `Out` left
    // unwritten keeps its value, and that hold must survive the forwarding.
    let mut n = 0u8;
    let mut shown = 0u8;

    for (i, &st) in STEP.iter().enumerate() {
        let en = i % 2 == 0;
        s_drv.write(Bits::<8>::from_u8(st));
        e_drv.write(if en { Logic::One } else { Logic::Zero });
        exec.tick_clock(&mut clk);

        n = n.wrapping_add(st);
        if en {
            shown = n;
        }

        let sb = Bits::<8>::from_u8(st);
        let el = if en { Logic::One } else { Logic::Zero };
        let ob = o_obs.read();
        let eb = Bits::<8>::from_u8(shown);
        eq.record(
            &[("step", &sb.as_array()[..]), ("enable", std::slice::from_ref(&el))],
            &[("o", &ob.as_array()[..])],
            &[("o", &eb.as_array()[..])],
        );
    }

    eq.finish();
}

/// **L-2** — forwarding must see through a `let` wire, and the dead wire must go.
#[test]
fn a_wire_feeding_a_regout_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("wire_into_regout", SRC, Some("wire_into_regout"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (s_drv, s_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (a_out, a_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = a_out.dirty_handle();
    exec.spawn_wired(wire_into_regout(clk.clone(), s_in, a_out), vec![dh], vec![]);

    let mut n = 0u8;
    for &st in STEP.iter() {
        s_drv.write(Bits::<8>::from_u8(st));
        exec.tick_clock(&mut clk);
        n = n.wrapping_add(st);

        let sb = Bits::<8>::from_u8(st);
        let ab = a_obs.read();
        let eb = Bits::<8>::from_u8(n.wrapping_add(1));
        eq.record(
            &[("step", &sb.as_array()[..])],
            &[("acc", &ab.as_array()[..])],
            &[("acc", &eb.as_array()[..])],
        );
    }

    eq.finish();
}

/// Shape pins for L-1 and L-2. The equivalence tests above would also fail if these
/// regressed, but they would fail as a number being off by one — these say why.
#[test]
fn the_edge_sampled_forms_carry_the_assigned_value() {
    let cfg = copper_codegen::EmitConfig::default();

    let cond = copper_codegen::transpile_source(SRC, Some("conditional_plain_out"), &cfg)
        .expect("conditional_plain_out transpiles");
    assert!(
        cond.contains("o <= (n + step)"),
        "a conditionally-driven plain `Out` is registered, so its drive is sampled at \
         the edge and must carry the assigned value (L-1), got:\n{cond}"
    );

    let wired = copper_codegen::transpile_source(SRC, Some("wire_into_regout"), &cfg)
        .expect("wire_into_regout transpiles");
    assert!(
        wired.contains("acc <= ((n + step) + 8'd1)"),
        "forwarding must see through the `let` wire (L-2), got:\n{wired}"
    );
    assert!(
        !wired.contains("bumped"),
        "the wire has no readers left once inlined; leaving it is `UNUSEDSIGNAL` under \
         the `-Wall` the equivalence harness runs, got:\n{wired}"
    );
}
