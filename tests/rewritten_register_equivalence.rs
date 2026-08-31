//! REGRESSION: a register assigned more than once inside one branch must end the
//! branch with its LAST value.
//!
//! ## The bug this pins (found 2026-08-24)
//!
//! `shir_lower::extract_updates_from_stmts` merges a branch's register updates
//! into one muxed next-value. It looked up each register's value in the branch
//! with `.find()` — the **first** update — so a register written twice kept the
//! earlier write and every later one was discarded:
//!
//! ```systemverilog
//! t <= (t + 4'd1);                                  // emitted
//! //   the `if t == 3 { t = 0; }` reset — GONE
//! ```
//!
//! That is the mod-N counter idiom (`t = t + 1; if t == N { t = 0; }`) — how
//! anyone writes a clock divider — and it silently produced a free-running counter
//! that wrapped at its full width instead. Silent because the surviving value is
//! perfectly well-typed; nothing downstream could tell it was the wrong one.
//!
//! ## Why it had gone unnoticed
//!
//! Outside a branch the merge does not happen: the updates are emitted as separate
//! non-blocking assignments and SystemVerilog's last-write-wins makes the result
//! right anyway (the earlier assignment is dead code). So the bug needed a register
//! rewritten inside an `if` or a `match` arm, and no example or fixture in the
//! corpus had one — every FSM arm here assigns each register once.
//!
//! It surfaced while measuring a control-extraction question, in a DUT whose wait
//! loop happened to be a mod-3 divider. Worth recording as the *class*: a merge
//! that silently keeps one of several candidates will keep the wrong one for as
//! long as nothing writes twice.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::types::{Bits, Logic};
use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/rewritten_register_dut.rs");
const SRC: &str = include_str!("fixtures/rewritten_register_dut.rs");

#[test]
fn divide_by_three_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("divider", SRC, Some("divider"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (o_out, o_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = o_out.dirty_handle();
    exec.spawn_wired(divider(clk.clone(), o_out), vec![dh], vec![]);

    // Independent reference: a divide-by-three. Run well past 16 cycles — with the
    // reset dropped the counter wrapped at its full 4-bit width, which only shows
    // up after the third division.
    let mut t = 0u8;
    let mut n = 0u8;

    for _ in 0..20 {
        exec.tick_clock(&mut clk);

        // The output is written at the top of the iteration, so it shows the count
        // as it stood BEFORE this cycle's division step.
        let shown = n;
        t += 1;
        if t == 3 {
            t = 0;
            n = n.wrapping_add(1);
        }

        let ob = o_obs.read();
        let eb = Bits::<8>::from_u8(shown);
        eq.record(
            &[],
            &[("tick_out", &ob.as_array()[..])],
            &[("tick_out", &eb.as_array()[..])],
        );
    }

    eq.finish();
}

#[test]
fn rewritten_inside_one_branch_sim_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("wrapping_counter", SRC, Some("wrapping_counter"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (u_drv, u_in) = wire::<Logic, MainClk>(Logic::Zero);
    let (o_out, o_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let dh = o_out.dirty_handle();
    exec.spawn_wired(wrapping_counter(clk.clone(), u_in, o_out), vec![dh], vec![]);

    let mut v = 0u8;
    for &u in &[
        true, true, false, true, true, true, true, false, true, true, true, true, true,
    ] {
        u_drv.write(Logic::from_bool(u));
        exec.tick_clock(&mut clk);

        let shown = v;
        if u {
            v = v.wrapping_add(1);
            if v == 5 {
                v = 0;
            }
        }

        let ul = Logic::from_bool(u);
        let ob = o_obs.read();
        let eb = Bits::<8>::from_u8(shown);
        eq.record(
            &[("up", std::slice::from_ref(&ul))],
            &[("out", &ob.as_array()[..])],
            &[("out", &eb.as_array()[..])],
        );
    }

    eq.finish();
}

/// Shape pin. The behavioural checks above would also pass if the emitted SV
/// happened to agree on the sampled cycles, so assert the merged value directly:
/// the arm's result must be the CONDITIONAL one, not the bare increment.
#[test]
fn a_branch_keeps_its_last_assignment() {
    let sv = copper_codegen::transpile_source(
        SRC,
        Some("divider"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("divider should transpile");

    assert!(
        sv.contains("t <= (((t + 4'd1) == 4'd3) ? 4'd0 : (t + 4'd1));"),
        "the arm's value for `t` must be its LAST assignment — the reset. A bare \
         `t <= (t + 4'd1)` is the dropped-reset bug, got:\n{sv}"
    );
}
