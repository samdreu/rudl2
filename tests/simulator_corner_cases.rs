//! Executor-level semantic corner cases (from `TODO`, SIMULATOR section).
//!
//! These exercise the settle engine itself rather than any one module's behavior:
//! that an acyclic combinational chain converges in a single settle regardless of
//! spawn order (the levelized topological walk), and that a genuine combinational
//! cycle with no fixed point is *detected* and reported rather than silently
//! producing garbage or hanging.
//!
//! Kept deliberately independent of the transpiler/Verilator path: the properties
//! here are about the simulator's scheduler, and the expected values are computed
//! from the known per-module transforms — not read back from the simulator.

use copper_core::port::{wire, In, Out};
use copper_core::types::Bits;
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

// ── combinational building blocks ─────────────────────────────────────────────

#[hardware(combinational)]
fn add_one(a: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    o.write(a.read() + Bits::from_lit::<1>());
}

#[hardware(combinational)]
fn invert(a: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    o.write(!a.read());
}

/// A three-stage acyclic combinational chain `in → add_one → invert → add_one →
/// out` settles in a **single** `poll_tasks`, and — crucially — does so even when
/// the stages are spawned *out of* topological order. The levelized scheduler
/// discovers the producer→consumer order structurally, so the whole chain
/// propagates in one settle with no manual re-poll.
#[test]
fn acyclic_combinational_chain_converges_in_one_settle() {
    let mut exec = HardwareExecutor::new();

    let (in_drv, in_in) = wire::<Bits<8>, ()>(Bits::zero());
    let (w1_out, w1_in) = wire::<Bits<8>, ()>(Bits::zero());
    let (w2_out, w2_in) = wire::<Bits<8>, ()>(Bits::zero());
    let (out_out, out_obs) = wire::<Bits<8>, ()>(Bits::zero());

    let (in_id, w1_read_id, w2_read_id) = (in_in.wire_id(), w1_in.wire_id(), w2_in.wire_id());

    // Spawn the LAST stage first and the first stage last: order must not matter.
    let s3_dh = out_out.dirty_handle();
    exec.spawn_wired(add_one(w2_in, out_out), vec![s3_dh], vec![w2_read_id]);
    let s2_dh = w2_out.dirty_handle();
    exec.spawn_wired(invert(w1_in, w2_out), vec![s2_dh], vec![w1_read_id]);
    let s1_dh = w1_out.dirty_handle();
    exec.spawn_wired(add_one(in_in, w1_out), vec![s1_dh], vec![in_id]);

    for x in [0u8, 1, 42, 200, 254, 255] {
        in_drv.write(Bits::<8>::from_u8(x));
        exec.poll_tasks(); // exactly one settle

        // out = ((!(x + 1)) + 1), all wrapping in u8.
        let expected = (!(x.wrapping_add(1))).wrapping_add(1);
        assert_eq!(
            out_obs.read().as_u8(),
            expected,
            "chain did not settle correctly for input {x}"
        );
    }
}

// ── combinational oscillation detection ───────────────────────────────────────

#[hardware(combinational)]
fn invert_a(a: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    o.write(!a.read());
}

#[hardware(combinational)]
fn buffer_a(a: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    o.write(a.read());
}

/// A genuine combinational cycle with **no fixed point**: `y = !x` feeds a buffer
/// `x = y`, so `x = !x`. This never settles; the executor must *detect* the cycle
/// (statically-condensed SCC iterated to the oscillation threshold) and panic with
/// a structural "combinational loop" report rather than hang or emit a garbage
/// value. This is the guardrail for un-broken feedback (no `RegOut`/synchronizer).
#[test]
#[should_panic(expected = "Combinational loop detected")]
fn non_convergent_combinational_loop_is_detected() {
    let mut exec = HardwareExecutor::new();

    // wire_x: driven by the buffer, read by the inverter.
    let (x_out, x_in) = wire::<Bits<8>, ()>(Bits::zero());
    // wire_y: driven by the inverter, read by the buffer.
    let (y_out, y_in) = wire::<Bits<8>, ()>(Bits::zero());

    let (x_read_id, y_read_id) = (x_in.wire_id(), y_in.wire_id());

    // inverter: y = !x
    let inv_dh = y_out.dirty_handle();
    exec.spawn_wired(invert_a(x_in, y_out), vec![inv_dh], vec![x_read_id]);
    // buffer: x = y  → closes the loop with net inversion (x = !x)
    let buf_dh = x_out.dirty_handle();
    exec.spawn_wired(buffer_a(y_in, x_out), vec![buf_dh], vec![y_read_id]);

    exec.poll_tasks(); // must panic: the SCC never converges
}
