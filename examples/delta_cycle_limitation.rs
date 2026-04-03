/// Delta-cycle behaviour: stable convergence and combinational loop resolution.
///
/// This example demonstrates two distinct scenarios:
///
/// ## Part 1 — Stable circuit with unconditional emit (converges cleanly)
///
/// Two chained combinational passthroughs both call emit! on every delta cycle
/// regardless of whether their output changed.  Value-change detection means the
/// executor only marks a signal dirty when the stored value actually changes, so
/// the circuit converges after two delta cycles despite the unconditional emits:
///
///   pass 1: a emits 5 (0→5, dirty), b emits 6 (0→6, dirty)
///   pass 2: a emits 5 (unchanged), b emits 6 (unchanged) → fixed point
///
/// ## Part 2 — Genuine combinational loop (resolves to X)
///
/// A NOT gate wired to its own output genuinely oscillates.  Because the module
/// reads from the same Arc it emits to, every pass flips the value:
///
///   pass 1: reads Zero, emits One  (Zero→One, dirty). consecutive_dirty = 1.
///   pass 2: reads One,  emits Zero (One→Zero, dirty). consecutive_dirty = 2.
///   ...
///   pass 20: consecutive_dirty hits OSCILLATION_THRESHOLD.
///            set_unknown() drives the output to Logic::X.
///   pass 21: reads X, emits NOT(X) = X (unchanged). → fixed point.
///
/// The final value is Logic::X — correctly modelling Verilog simulator behaviour
/// where a combinational loop resolves to an unknown/indeterminate state.
///
/// Run this example:
///
///     cargo run --example delta_cycle_limitation
use copper_core::Logic;
use copper_sim::{delta_yield, emit, HardwareExecutor};
use std::sync::{Arc, Mutex};

// ── Part 1: stable chained passthroughs ──────────────────────────────────────

async fn passthrough(input: Arc<Mutex<u8>>) -> u8 {
    loop {
        emit!(*input.lock().unwrap());
        delta_yield().await;
    }
}

async fn increment(input: Arc<Mutex<u8>>) -> u8 {
    loop {
        emit!(input.lock().unwrap().wrapping_add(1));
        delta_yield().await;
    }
}

// ── Part 2: self-inverter (genuine combinational loop) ────────────────────────

async fn self_inverter(wire: Arc<Mutex<Logic>>) -> Logic {
    loop {
        let current = *wire.lock().unwrap();
        emit!(match current {
            Logic::Zero => Logic::One,
            Logic::One  => Logic::Zero,
            Logic::X    => Logic::X,
        });
        delta_yield().await;
    }
}

fn main() {
    // ── Part 1 ───────────────────────────────────────────────────────────────
    println!("=== Part 1: stable chained passthroughs ===");
    {
        let mut exec = HardwareExecutor::new();

        let raw = Arc::new(Mutex::new(5u8));
        let wire_a = exec.spawn_function_typed(0u8, passthrough(Arc::clone(&raw)));
        let wire_b = exec.spawn_function_typed(0u8, increment(Arc::clone(&wire_a)));

        exec.poll_tasks();

        println!("wire_a = {} (expected 5)", *wire_a.lock().unwrap());
        println!("wire_b = {} (expected 6)", *wire_b.lock().unwrap());
        assert_eq!(*wire_a.lock().unwrap(), 5);
        assert_eq!(*wire_b.lock().unwrap(), 6);
    }

    println!();

    // ── Part 2 ───────────────────────────────────────────────────────────────
    println!("=== Part 2: self-inverter loop (X propagation) ===");
    {
        let mut exec = HardwareExecutor::new();

        // `wire` is both the input the module reads AND the signal it drives.
        // Using spawn_into_with_unknown wires this correctly: the caller creates
        // the Arc, passes a clone to the module as input, and passes the Arc
        // itself as the emit target.
        let wire = Arc::new(Mutex::new(Logic::Zero));
        exec.spawn_into_with_unknown(
            Arc::clone(&wire),
            self_inverter(Arc::clone(&wire)),
        );

        exec.poll_tasks();

        // The executor detected the oscillation (consecutive_dirty hit 20),
        // injected Logic::X, and the module settled: NOT(X) = X.
        let result = *wire.lock().unwrap();
        println!("self-inverter output = {:?} (expected X)", result);
        assert_eq!(result, Logic::X);
    }
}
