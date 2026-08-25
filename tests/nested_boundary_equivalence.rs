//! **Cause K** — a loop whose body ends in another tick-bearing loop: sim ≡
//! transpiled SystemVerilog.
//!
//! Control extraction's state model is "a state is the code between two ticks", and
//! a nested loop used to have to supply that boundary itself: `tick_is_last_statement`
//! demanded a *direct* `clk.tick().await` as the body's last statement. A body whose
//! boundary comes from a loop INSIDE it was declined, and the module fell through to
//! the linear path — which then reported the tick-ordering rule, advice no author of
//! this shape can act on, since the body has no tick in statement position at all.
//!
//! The flattener itself already handled the shape: a `break` inlines the enclosing
//! loop's continuation in the same cycle, so the outer back edge is taken when the
//! inner loop exits, which is exactly what this needs. The fix was therefore in the
//! **gate** (`body_ends_at_a_clock_boundary`), plus the diagnostic, plus one real
//! bug the relaxation exposed — see below.
//!
//! ## Two things this cost, both worth keeping in view
//!
//! **1. `find_tick_in_expr` did not descend into a nested `loop`.** Control
//! extraction ends by reusing a real tick node from the source for its single
//! trailing tick. That search and the gate must agree about where a tick can live;
//! they had silently drifted, and it went unnoticed only because every flattenable
//! module also happened to have a tick at the top level of its own loop. Cause K is
//! the first shape whose ONLY tick is inside a nested loop, and the pass declined it
//! silently while the linear path blamed an unrelated construct.
//!
//! **2. Reachability accepted a program the simulator livelocks on.** The parent CFG
//! folded a tick-bearing nested loop into one node with a `Tick` out-edge,
//! unconditionally. That is right for a counted `for` (it runs its body, so it
//! ticks) and wrong for a `loop` that can `break` before its first tick. With the
//! gate relaxed, `loop { loop { if a { break; } loop { if b { break; } tick; } } }`
//! transpiled to an FSM running one cycle per iteration while the simulator spun
//! forever — measured at 99.5% CPU with no progress. `may_exit_without_tick` now
//! records the zero-tick path for `check_reachability` alone, leaving the `Tick`
//! edge (which liveness needs) intact.
//!
//! ## Scope of what is checked here
//!
//! The DUT's inner loop never breaks, and the fixture explains why that is forced
//! rather than chosen: the tick-last rule makes every exitable inner loop capable of
//! a zero-tick exit, which rule 2 now rejects. So cause K's **back edge** is not
//! writable with `loop` alone — it needs a counted `for`, which always ticks and
//! needs no exit test. What is pinned here is the **entry** path, which is the other
//! half of the crux: entering the outer loop must not cost a cycle.

mod common;

use common::EquivalenceTest;
use copper_core::port::{registered_wire, wire, In, RegOut};
use copper_core::types::{Bits, Logic};
use copper_core::{Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

include!("fixtures/nested_boundary_dut.rs");
const SRC: &str = include_str!("fixtures/nested_boundary_dut.rs");

/// Uneven on purpose, and never all-equal: a lowering that dropped or duplicated a
/// cycle would still match a constant stimulus.
const STEP: [u8; 10] = [3, 1, 7, 0, 2, 2, 9, 4, 1, 5];

#[test]
fn a_loop_ending_in_a_tick_bearing_loop_matches_transpiled_verilog() {
    let mut eq = EquivalenceTest::for_module("nested_boundary", SRC, Some("nested_boundary"));

    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let (s_drv, s_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    let (a_out, a_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
    let (b_out, b_obs) = registered_wire::<Logic, MainClk>(&clk, Logic::Zero);
    let dh = a_out.dirty_handle();
    let bh = b_out.dirty_handle();
    exec.spawn_wired(
        nested_boundary(clk.clone(), s_in, a_out, b_out),
        vec![dh, bh],
        vec![],
    );

    // Reference as the sequence rather than a closed form. `acc` is what pins the
    // shared cycle: it shows the value `n` held DURING the cycle (written before the
    // update), so cycle 0 shows 0 and each later cycle shows the running sum of the
    // steps applied before it — one lost cycle at entry shifts every value. `busy`
    // pins the weaker but separate claim that the outer prefix runs at all and that
    // an unwritten `RegOut` then holds it.
    let mut n = 0u8;

    for &st in STEP.iter() {
        s_drv.write(Bits::<8>::from_u8(st));
        exec.tick_clock(&mut clk);

        let expected_acc = n;
        n = n.wrapping_add(st);

        let sb = Bits::<8>::from_u8(st);
        let ab = a_obs.read();
        let bb = b_obs.read();
        let eb = Bits::<8>::from_u8(expected_acc);
        let ebusy = Logic::One;
        eq.record(
            &[("step", &sb.as_array()[..])],
            &[
                ("acc", &ab.as_array()[..]),
                ("busy", std::slice::from_ref(&bb)),
            ],
            &[
                ("acc", &eb.as_array()[..]),
                ("busy", std::slice::from_ref(&ebusy)),
            ],
        );
    }

    eq.finish();
}

/// Shape pin: entering the outer loop must not burn a cycle. The outer body's
/// prefix has to land in state 0 *alongside* the inner loop's prefix — if the
/// entry were lowered as its own state, state 0 would do nothing but `pc <= 1`.
#[test]
fn entering_the_outer_loop_costs_no_cycle() {
    let sv = copper_codegen::transpile_source(
        SRC,
        Some("nested_boundary"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("a loop ending in a tick-bearing loop must transpile");

    assert!(
        sv.contains("case (pc)"),
        "the shape must flatten to a pc FSM, got:\n{sv}"
    );
    // State 0 carries the accumulate, not just a jump.
    let state0 = sv
        .split("8'd0: begin")
        .nth(1)
        .expect("an FSM with a state 0");
    assert!(
        state0.contains("acc <="),
        "state 0 must carry the outer prefix AND the inner loop's first iteration in one \
         cycle; `acc` missing from it means entering the nested loop cost a clock cycle:\n{sv}"
    );
}
