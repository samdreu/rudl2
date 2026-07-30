//! Item 4 — hierarchical clocked submodule instantiation (the multi-clock enabler).
//!
//! The same dual-clock design as `two_domain_counter`, but expressed as **one
//! coherent hierarchical component**: a `#[hardware(structural)]` parent
//! (`two_domain_top`) that instantiates the fast producer, the synchronizer, and
//! the slow consumer, threading each child's own clock through. The parent has no
//! `always_ff` of its own — it is pure hierarchy.
//!
//! Transpiling `two_domain_top` (see `copper-transpile --module two_domain_top
//! --hierarchy`) emits real SystemVerilog with `.clk(wr_clk)` / `.clk(rd_clk)`
//! port connections and internal nets — a self-contained dual-clock design
//! Verilator lints clean. The *simulation* still hand-wires the children in the
//! testbench (sim-as-unit is a deferred, separate capability), exactly as
//! `two_domain_counter` does — so this example's `main` is the sim authority and
//! the structural parent is the transpilation authority for the same design.
//!
//! What `main` checks: the 2:1 timeline (matching `two_domain_counter`), and the
//! **rate-independent CDC invariant** — under *any* fast:slow tick interleaving,
//! a flag crossing through the 2-FF synchronizer is monotone (never glitches low
//! once synchronized) and eventually propagates. That is the observable content
//! of "a well-formed multi-clock design behaves correctly under any relative tick
//! rate, provided every crossing goes through a synchronizer."

use copper_core::port::{wire, In, Out};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

// ── Clock domains ─────────────────────────────────────────────────────────────
struct ClkFast;
impl ClockDomain for ClkFast {}

struct ClkSlow;
impl ClockDomain for ClkSlow {}

// ── Fast-domain producer: counter + latched threshold flag ────────────────────
#[hardware(sequential)]
async fn fast_counter(
    clk: Clock<ClkFast>,
    count_out: Out<Bits<8>, ClkFast>,
    flag_out: Out<Logic, ClkFast>,
) {
    let mut count: Bits<8> = Bits::zero();
    let mut latched = Logic::Zero;
    loop {
        if count[3] == Logic::One {
            latched = Logic::One;
        }
        count_out.write(count);
        flag_out.write(latched);
        clk.tick().await;
        count = count + Bits::from_lit::<1>();
    }
}

// ── Synchronizer: ClkFast → ClkSlow 2-FF, concrete (transpilable) ─────────────
//
// The library `copper::sync_2ff` is generic over source/destination domains; the
// transpiler only handles concrete modules, so the example uses a concrete
// specialization (ClkFast → ClkSlow). Behaviourally identical: two destination
// flip-flops, two `ClkSlow` cycles of latency.
#[hardware(synchronizer)]
async fn flag_sync(clk: Clock<ClkSlow>, d: In<Logic, ClkFast>, q: Out<Logic, ClkSlow>) {
    let mut ff1 = Logic::Zero;
    let mut ff2 = Logic::Zero;
    loop {
        q.write(ff2);
        clk.tick().await;
        // ff2 captures the OLD ff1 (ff1 is reassigned after) — the two stages stay
        // distinct; reversing these lines would collapse them into one flop.
        ff2 = ff1;
        ff1 = d.read();
    }
}

// ── Slow-domain consumer: reads the synchronized flag ─────────────────────────
#[hardware(sequential)]
async fn slow_consumer(
    clk: Clock<ClkSlow>,
    flag_in: In<Logic, ClkSlow>,
    out: Out<Logic, ClkSlow>,
) {
    loop {
        out.write(flag_in.read());
        clk.tick().await;
    }
}

// ── The hierarchy: one structural parent wiring the three children together ───
//
// This is the item-4 payload. `two_domain_top` receives *both* clocks and
// instantiates each child on its native domain, threading `wr_clk`/`rd_clk`
// through and wiring the flag `ClkFast → sync_2ff → ClkSlow` crossing with
// internal nets. It has no clock of its own to tick. Transpiling it emits a real
// hierarchical dual-clock module (parent + `.clk(...)`-wired children). It is not
// spawned in the sim below (its children are hand-wired) — sim-as-unit is a
// deferred capability; here it is the transpilation authority for the design.
// Not spawned in the sim (transpile-only, sim-as-unit deferred) — hence unused here.
#[allow(dead_code)]
#[hardware(structural)]
async fn two_domain_top(
    wr_clk: Clock<ClkFast>,
    rd_clk: Clock<ClkSlow>,
    count_out: Out<Bits<8>, ClkFast>,
    flag_sync_out: Out<Logic, ClkSlow>,
) {
    let flag = wire::<Logic, ClkFast>(Logic::Zero);
    let synced = wire::<Logic, ClkSlow>(Logic::Zero);
    fast_counter(wr_clk, count_out, flag.0);
    flag_sync(rd_clk.clone(), flag.1, synced.0);
    slow_consumer(rd_clk, synced.1, flag_sync_out);
}

/// Run the hand-wired dual-clock design for `slow_cycles` slow ticks, ticking the
/// fast domain `fast_per_slow` times per slow tick. Returns the per-slow-cycle
/// observed `(fast_count, flag_sync)`.
fn run_schedule(fast_per_slow: usize, slow_cycles: usize) -> Vec<(u8, Logic)> {
    let mut clk_fast = Clock::<ClkFast>::new();
    let mut clk_slow = Clock::<ClkSlow>::new();
    let mut exec = HardwareExecutor::new();

    let (count_port, count_obs) = wire::<Bits<8>, ClkFast>(Bits::zero());
    let (flag_port, flag_to_sync) = wire::<Logic, ClkFast>(Logic::Zero);
    let (sync_q_port, sync_to_consumer) = wire::<Logic, ClkSlow>(Logic::Zero);
    let (consumer_port, consumer_obs) = wire::<Logic, ClkSlow>(Logic::Zero);

    let dh_count = count_port.dirty_handle();
    let dh_flag = flag_port.dirty_handle();
    let dh_sync_q = sync_q_port.dirty_handle();
    let dh_consumer = consumer_port.dirty_handle();

    exec.spawn_wired(
        fast_counter(clk_fast.clone(), count_port, flag_port),
        vec![dh_count, dh_flag],
    );
    exec.spawn_wired(
        flag_sync(clk_slow.clone(), flag_to_sync, sync_q_port),
        vec![dh_sync_q],
    );
    exec.spawn_wired(
        slow_consumer(clk_slow.clone(), sync_to_consumer, consumer_port),
        vec![dh_consumer],
    );

    let mut trace = Vec::with_capacity(slow_cycles);
    for _ in 0..slow_cycles {
        for _ in 0..fast_per_slow {
            exec.tick_clock(&mut clk_fast);
        }
        exec.tick_clock(&mut clk_slow);
        let fast_count = count_obs.read().as_u128() as u8;
        let _ = &count_obs; // count is observed in the fast domain for display
        trace.push((fast_count, consumer_obs.read()));
    }
    trace
}

/// The rate-independent CDC invariant: the synchronized flag is monotone (never
/// falls back to `Zero` once it has synchronized to `One`) and, given enough
/// cycles, eventually asserts. True for *any* fast:slow interleaving because the
/// crossing goes through the 2-FF synchronizer.
fn assert_cdc_invariant(trace: &[(u8, Logic)], label: &str) -> bool {
    let mut seen_one = false;
    for (i, (_, flag_sync)) in trace.iter().enumerate() {
        if *flag_sync == Logic::One {
            seen_one = true;
        } else if seen_one {
            eprintln!("{label}: flag_sync glitched back to Zero at slow cycle {i}");
            return false;
        }
    }
    if !seen_one {
        eprintln!("{label}: flag_sync never asserted over {} cycles", trace.len());
        return false;
    }
    true
}

fn main() {
    println!("=== CDC Two-Domain Hierarchy (structural parent) ===");

    // 2:1 schedule — matches `two_domain_counter`'s validated timeline: the
    // latched flag first reaches the slow domain (through the 2-FF synchronizer)
    // at slow cycle 5 (2-slow-cycle synchronizer latency after the fast count
    // crosses 8 at slow cycle 3).
    let trace_2to1 = run_schedule(2, 10);
    println!("\n2:1 fast:slow");
    println!("{:>5}  {:>10}  {:>9}", "slow", "fast_count", "flag_sync");
    for (i, (fast_count, flag_sync)) in trace_2to1.iter().enumerate() {
        println!(
            "{:>5}  {:>10}  {:>9}",
            i,
            fast_count,
            if *flag_sync == Logic::One { "1" } else { "0" }
        );
    }
    let mut all_pass = true;
    for (i, (_, flag_sync)) in trace_2to1.iter().enumerate() {
        let expected = if i >= 5 { Logic::One } else { Logic::Zero };
        if *flag_sync != expected {
            eprintln!("2:1 mismatch at slow cycle {i}");
            all_pass = false;
        }
    }
    all_pass &= assert_cdc_invariant(&trace_2to1, "2:1");

    // 3:1 schedule — a *different* relative tick rate. The exact assert cycle
    // shifts (the fast count crosses 8 sooner in slow-cycle terms), but the
    // rate-independent CDC invariant must still hold: monotone + eventually
    // asserts. This is the observable "correct under any interleaving" property.
    let trace_3to1 = run_schedule(3, 10);
    all_pass &= assert_cdc_invariant(&trace_3to1, "3:1");

    // 1:1 schedule — the slowest crossing. Same invariant.
    let trace_1to1 = run_schedule(1, 16);
    all_pass &= assert_cdc_invariant(&trace_1to1, "1:1");

    println!(
        "\nRate-independent CDC invariant (monotone + eventually asserts) across 2:1 / 3:1 / 1:1: {}",
        if all_pass { "✓" } else { "✗" }
    );
    if !all_pass {
        std::process::exit(1);
    }
}
