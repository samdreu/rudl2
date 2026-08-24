//! Repeating waits — `loop { <test>; clk.tick().await; }` nested inside the
//! module's own loop, left by `break`: sim ≡ transpiled SystemVerilog.
//!
//! This is control extraction "increment B". Increment A flattened ticks nested in
//! `if`/`match` by inlining each continuation into the branches; a nested loop adds
//! the one thing that model lacked — a state that can loop back to **itself**.
//!
//! ## The lowering
//!
//! A nested loop gets a head state H, and its body is lowered once with H as the
//! back-edge target: the body's trailing tick emits `pc = H` (stay another cycle),
//! and `break` inlines the loop's continuation (leave, in the *same* cycle —
//! breaking is not a clock boundary, which is why it is inlined rather than
//! jumped to). Entering must not cost a cycle either, so the already-lowered body
//! is cloned into the entry point. Cloning the lowered form rather than
//! re-lowering the source is what keeps this cheap: the two copies share the
//! sub-states their ticks allocated, so a wait costs ONE extra state.
//!
//! `if ready { break; }` also forces a split that increment A would not have made:
//! the branch contains no tick, but control still leaves it, so the continuation
//! has to be inlined per-branch just the same. That is why "diverges" replaced
//! "contains a tick" as the splitting rule.
//!
//! ## Scope: the tick must be the body's LAST statement — decided, not deferred
//!
//! `tick_first_waiter` in the fixture is the refused ordering. Testing *after* the
//! tick puts the read in the window where an `Immediate` read consumes the value
//! the just-past edge produced while the flip-flop it lowers to samples the value
//! present before its own edge. Measured, the transpiled module reacted a full
//! cycle earlier than the simulator on every detection — and holding each stimulus
//! value for two cycles did **not** reconcile them, so it is not a
//! which-of-two-samples question but two different windows.
//!
//! **Copper does not choose between the two samplings; it declines to let a design
//! depend on which** (2026-08-24). Same disposition as the pre-tick alignment
//! hazard: the divergent program is made unwritable. The cost is low because the
//! supported ordering expresses the same designs, and because the divergence needs
//! an input that changes mid-cycle — a clocked producer in the same domain is
//! stable across the window and both models agree.
//!
//! Worth knowing: this is the ordering `examples/cpu/rv32i_cpu.rs` is written in,
//! but the CPU is blocked ahead of it on three other things (a `Vec` port,
//! host-side Rust at construction, a run-time memory preload), so the restriction
//! is not what keeps it out. `TODO` has the measurement.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::types::{Bits, Logic};
use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/wait_loop_dut.rs");
const SRC: &str = include_str!("fixtures/wait_loop_dut.rs");

/// Deliberately uneven: runs of low `go` of length 2, 1 and 3, so a wait that
/// held for a fixed number of cycles instead of a variable one cannot pass.
const GO: [bool; 12] = [
    false, false, true, false, true, true, false, false, false, true, true, false,
];
const ACK: [bool; 12] = [
    false, true, false, true, true, false, true, false, true, false, true, true,
];

#[test]
fn wait_until_go_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("waiter", SRC, Some("waiter"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (g_drv, g_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (c_out, c_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = c_out.dirty_handle();
    exec.spawn_wired(waiter(clk.clone(), g_in, c_out), vec![dh], vec![]);

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

#[test]
fn two_sequential_waits_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("handshake", SRC, Some("handshake"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (r_drv, r_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (a_drv, a_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (d_out, d_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = d_out.dirty_handle();
    exec.spawn_wired(handshake(clk.clone(), r_in, a_in, d_out), vec![dh], vec![]);

    // Two waits with a tick between them. Note what that middle tick costs: the
    // cycle in which `req` is seen is ALSO the middle tick's cycle, because
    // breaking out of the first wait takes no time — so the module is testing
    // `ack` on the very next cycle, not the one after. Getting this wrong is a
    // one-cycle error the sim catches immediately, which is the point of writing
    // the reference as the sequence rather than as an index formula.
    enum S {
        Req,
        Ack,
    }
    let mut state = S::Req;
    let mut n = 0u8;
    let mut shown = 0u8;

    for (&r, &a) in GO.iter().zip(ACK.iter()) {
        r_drv.write(Logic::from_bool(r));
        a_drv.write(Logic::from_bool(a));
        exec.tick_clock(&mut clk);

        let expected = shown;
        state = match state {
            S::Req if r => S::Ack,
            S::Req => S::Req,
            S::Ack if a => {
                n = n.wrapping_add(1);
                S::Req
            }
            S::Ack => S::Ack,
        };
        shown = n;

        let rl = Logic::from_bool(r);
        let al = Logic::from_bool(a);
        let db = d_obs.read();
        let eb = Bits::<8>::from_u8(expected);
        eq.record(
            &[
                ("req", std::slice::from_ref(&rl)),
                ("ack", std::slice::from_ref(&al)),
            ],
            &[("done", &db.as_array()[..])],
            &[("done", &eb.as_array()[..])],
        );
    }

    eq.finish();
}

/// Shape pins. A wait must be a state that can stay put, and it must cost ONE
/// extra state — re-lowering the body at the entry instead of cloning the lowered
/// form would double the state count per loop, which is the thing to notice if
/// this ever regresses.
#[test]
fn a_wait_is_a_self_looping_state() {
    let sv = copper_codegen::transpile_source(
        SRC,
        Some("waiter"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("waiter should transpile");

    // `pc <= (cond ? <leave> : <stay>)` — the stay branch names the state itself.
    assert!(
        sv.contains("8'd1: begin") && sv.contains("8'd1)"),
        "expected a self-looping wait state, got:\n{sv}"
    );
    assert!(
        !sv.contains("8'd2"),
        "one wait should cost ONE extra state; a third means the body was re-lowered \
         at the entry instead of cloned, got:\n{sv}"
    );
}

/// The refused ordering, with its diagnostic. Flips loudly if the mid-phase-read
/// question is ever settled.
#[test]
fn ticking_before_the_test_is_refused() {
    let err = copper_codegen::transpile_source(
        SRC,
        Some("tick_first_waiter"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect_err(
        "This ordering is refused BY DESIGN, not pending support. If it now transpiles, a \
         language decision was reversed — that needs sign-off, and a hardware-anchored check \
         that the module no longer reacts a cycle earlier than the simulator.",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("LAST statement") && msg.contains("clk.tick().await"),
        "the diagnostic must state the ordering rule: {msg}"
    );
}
