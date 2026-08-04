//! Executor guardrails and under-specified behaviors (P0, `TODO` TESTING plan;
//! SIMULATOR section: "Module completion panics", "Untracked outputs",
//! "Multiple driver behavior").
//!
//! Where a guardrail exists (a malformed module whose future returns), this pins
//! the panic. Where the behavior is currently *un-specified* or a known trap
//! (untracked outputs), this pins the actual observed behavior so any future
//! change to it is deliberate and visible — not silently regressed.

use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, HardwareModule};

#[hardware(combinational)]
fn add_one(a: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    o.write(a.read() + Bits::from_lit::<1>());
}

#[hardware(combinational)]
fn passthrough(a: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    o.write(a.read());
}

// ── module-completion panic ───────────────────────────────────────────────────

/// A `#[hardware]` module's body is a non-terminating `loop { .. }`; its future
/// must never resolve. A malformed module whose future returns `Poll::Ready`
/// (here constructed directly to bypass the macro's loop-shape enforcement) is a
/// hard error: the executor asserts `is_pending` on every poll and panics, rather
/// than silently dropping a "finished" hardware task.
#[test]
#[should_panic(expected = "returned Poll::Ready")]
fn module_whose_future_returns_panics() {
    let mut exec = HardwareExecutor::new();
    // `__new` is the macro-internal constructor; used here only to fabricate the
    // malformed (immediately-completing) module the macro itself would never emit.
    let malformed = HardwareModule::__new(async {});
    exec.spawn_wired(malformed, vec![], vec![]);
    exec.poll_tasks(); // must panic in the settle
}

// ── untracked output (documented trap: no dirty handle ⇒ no re-settle) ─────────

/// `spawn_wired` with an **empty** `dirties` vec runs the module but leaves its
/// output *untracked*: the executor cannot see the output change, so it draws no
/// producer→consumer dependency edge and never re-settles a downstream consumer on
/// its behalf. This pins the resulting observable behavior for a producer→consumer
/// pair where the producer's combinational output is untracked. It is the hazard
/// the `spawn_untracked`/empty-`dirties` docs warn about, and the motivation for
/// the still-open "untracked outputs warning/error" TODO — captured here so a fix
/// (or a regression) is caught against a concrete baseline.
#[test]
fn untracked_producer_output_settle_behavior_is_pinned() {
    let mut exec = HardwareExecutor::new();

    let (in_drv, in_in) = wire::<Bits<8>, ()>(Bits::zero());
    let (w_out, w_in) = wire::<Bits<8>, ()>(Bits::zero());
    let (out_out, out_obs) = wire::<Bits<8>, ()>(Bits::zero());

    let in_id = in_in.wire_id();
    let w_read_id = w_in.wire_id();

    // Consumer spawned FIRST, tracked. Producer spawned SECOND, UNTRACKED
    // (empty dirties) — so no comb edge producer→consumer is recorded.
    let cons_dh = out_out.dirty_handle();
    exec.spawn_wired(passthrough(w_in, out_out), vec![cons_dh], vec![w_read_id]);
    exec.spawn_wired(add_one(in_in, w_out), vec![], vec![in_id]); // untracked output

    in_drv.write(Bits::<8>::from_u8(5));

    // OBSERVED BASELINE (the hazard): because the producer's output is untracked,
    // no producer→consumer edge exists, the consumer is polled without the
    // producer's new value, and a single settle leaves `out` STALE at its old
    // value — silently, with no warning or error. The producer did compute 6 into
    // the intermediate wire; the consumer just didn't get re-polled to see it.
    exec.poll_tasks();
    assert_eq!(
        out_obs.read().as_u8(),
        0,
        "untracked-output hazard: consumer is stale after one settle (in+1 = 6 not propagated)"
    );

    // A *second* settle propagates it (the consumer re-reads the now-updated wire),
    // confirming the value is merely late, not lost — the trap is the missing
    // single-settle convergence, which the empty-`dirties` docs warn about and the
    // open "untracked outputs warning/error" TODO would surface automatically.
    exec.poll_tasks();
    assert_eq!(out_obs.read().as_u8(), 6, "second settle propagates the untracked producer's output");
}
