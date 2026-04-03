/// Delta-cycle convergence demonstration.
///
/// Copper's delta cycling asks "did any signal value change this pass?" to
/// decide whether to run another pass.  Because emit_to_current requires
/// T: PartialEq, it only marks a signal dirty when the newly emitted value
/// actually differs from the stored one.
///
/// This means a combinational module that unconditionally calls emit! with
/// the same value will NOT prevent convergence — the executor sees no changes
/// and stops after a small number of passes.
///
/// This example demonstrates that using delta_yield(), a future that suspends
/// for exactly one delta cycle so a combinational module can be re-evaluated
/// on every pass.  Despite both modules calling emit! unconditionally, the
/// circuit converges because the values stabilise after pass 1.
///
/// A true combinational loop — where outputs never stop changing — would
/// still exceed MAX_DELTA_CYCLES and panic.  That is the remaining genuine
/// limitation: value-change detection handles stable circuits with redundant
/// emits, but cannot resolve an oscillating combinational loop.
///
/// Run this example:
///
///     cargo run --example delta_cycle_limitation
use copper_sim::{delta_yield, emit, HardwareExecutor};
use std::sync::{Arc, Mutex};

// ── Two chained combinational passthrough modules ────────────────────────────
//
// comb_stage_a reads raw_input and forwards it unchanged.
// comb_stage_b reads wire_a (stage A's output) and adds 1.
//
// Both use delta_yield() so they are re-evaluated on every delta cycle.
// Both unconditionally call emit! regardless of whether their input changed.
//
// The signal values reach a fixed point after a single delta cycle:
//   pass 1 — a emits 5 (changed from 0), b emits 6 (changed from 0)  → dirty
//   pass 2 — a emits 5 (unchanged), b emits 6 (unchanged)             → not dirty → stop
//
// With value-change detection, the executor correctly identifies that no
// signal actually changed on pass 2 and terminates cleanly.

async fn comb_stage_a(raw_input: Arc<Mutex<u8>>) -> u8 {
    loop {
        let val = *raw_input.lock().unwrap();
        emit!(val); // always fires — even when val hasn't changed
        delta_yield().await;
    }
}

async fn comb_stage_b(wire_a: Arc<Mutex<u8>>) -> u8 {
    loop {
        let val = *wire_a.lock().unwrap();
        emit!(val.wrapping_add(1)); // always fires
        delta_yield().await;
    }
}

fn main() {
    let mut exec = HardwareExecutor::new();

    let raw_input = Arc::new(Mutex::new(5u8));
    let wire_a = exec.spawn_function_typed(0u8, comb_stage_a(Arc::clone(&raw_input)));
    let _wire_b = exec.spawn_function_typed(0u8, comb_stage_b(Arc::clone(&wire_a)));

    println!("Calling poll_tasks() with two unconditional-emit combinational modules.");
    println!("Values stabilise after 1 delta cycle; value-change detection stops the loop.\n");

    // Converges cleanly: pass 1 changes values (0→5 and 0→6), pass 2 emits
    // identical values so no signal is marked dirty, loop terminates.
    exec.poll_tasks();

    println!("wire_a = {}", *wire_a.lock().unwrap()); // 5
}
