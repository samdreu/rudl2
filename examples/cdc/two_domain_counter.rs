use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_core::port::{wire, In, Out};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

// ── Clock domains ─────────────────────────────────────────────────────────────
// Two independent clock domains. In the testbench ClkFast is ticked twice per
// ClkSlow tick (2:1 ratio), matching the kind of relationship where a fast
// producer and a slow consumer must communicate safely.

struct ClkFast;
impl ClockDomain for ClkFast {}

struct ClkSlow;
impl ClockDomain for ClkSlow {}

// ── Producer: counter in the fast domain ──────────────────────────────────────
// Increments every ClkFast tick. `latched` goes high once count reaches 8
// (bit 3 first sets) and stays high — modelling a "threshold crossed" condition
// that needs to propagate safely to the slow domain.
#[hardware(sequential)]
async fn fast_counter(
    clk: Clock<ClkFast>,
    count_out: Out<Bits<8>, ClkFast>,
    flag_out: Out<Logic, ClkFast>,
) {
    let mut count: Bits<8> = Bits::zero();
    let mut latched = Logic::Zero;
    loop {
        count_out.write(count);
        flag_out.write(latched);
        clk.tick().await;
        // Sticky threshold, updated AFTER the edge so it reads the pre-edge `count`
        // exactly as `if (cnt[3]) latch <= 1'b1;` does. Updating it *before* the
        // writes puts a register assignment in the pre-tick segment with no input
        // read to pin the phase — the simulator then runs that segment a phase early
        // and silently disagrees with the synthesized hardware. Measured against the
        // independent reference in `sv/two_domain_hierarchy.sv`; see
        // design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md.
        if count[3] == Logic::One {
            latched = Logic::One;
        }
        count = count + Bits::from_lit::<1>();
    }
}

// ── Crossing ClkFast → ClkSlow ────────────────────────────────────────────────
// The crossing goes through the library synchronizer `copper::sync_2ff` — two
// flip-flops clocked by the *destination* domain. It accepts a signal tagged with
// any source domain because it is the designated crossing point. Two things make
// an *un*synchronized crossing impossible:
//   1. The phantom domain types: passing the ClkFast wire directly to a ClkSlow
//      module is a compile error —
//        // slow_consumer(clk_slow.clone(), flag_to_sync, consumer_port);
//        // error[E0308]: expected `In<Logic, ClkSlow>`, found `In<Logic, ClkFast>`
//   2. The CDC check in `#[hardware(sequential)]`: a regular module may not even
//      *declare* a foreign-domain port. Only a `#[hardware(synchronizer)]` module
//      (like `sync_2ff`) may, so hand-rolling an ad-hoc crossing won't compile.
use copper::sync_2ff;

// ── Consumer: reads the synchronized flag in the slow domain ──────────────────
#[hardware(sequential)]
async fn slow_consumer(
    clk: Clock<ClkSlow>,
    flag_in: In<Logic, ClkSlow>,
    out: Out<Logic, ClkSlow>,
) {
    loop {
        clk.tick().await;
        // Read AFTER the tick. A pure combinational passthrough transpiles to
        // `assign out = flag_in` (zero cycles), but a *leading* read is classified
        // `Deferred` and samples at the pre-edge, while a clocked producer updates at
        // the post-edge — so the leading-read form lags a cycle in the simulator and
        // silently disagrees with its own SV. A trailing read is `Immediate` and
        // tracks the producer, matching the netlist. See D2 in
        // design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md.
        out.write(flag_in.read());
    }
}

fn main() {
    let mut clk_fast = Clock::<ClkFast>::new();
    let mut clk_slow = Clock::<ClkSlow>::new();
    let mut exec = HardwareExecutor::new();

    // fast_counter → count_out wire (ClkFast domain)
    let (count_port, count_obs)         = wire::<Bits<8>, ClkFast>(Bits::zero());
    // fast_counter → sync_2ff wire: ClkFast-tagged, metastable in ClkSlow context
    let (flag_port, flag_to_sync)       = wire::<Logic, ClkFast>(Logic::Zero);
    // sync_2ff → slow_consumer wire (ClkSlow domain, metastability resolved)
    let (sync_q_port, sync_to_consumer) = wire::<Logic, ClkSlow>(Logic::Zero);
    // slow_consumer → output (ClkSlow domain)
    let (consumer_port, consumer_obs)   = wire::<Logic, ClkSlow>(Logic::Zero);

    let dh_count    = count_port.dirty_handle();
    let dh_flag     = flag_port.dirty_handle();
    let dh_sync_q   = sync_q_port.dirty_handle();
    let dh_consumer = consumer_port.dirty_handle();

    let sync_reads = vec![flag_to_sync.wire_id()];
    let consumer_reads = vec![sync_to_consumer.wire_id()];
    exec.spawn_wired(
        fast_counter(clk_fast.clone(), count_port, flag_port),
        vec![dh_count, dh_flag],
        vec![],
    );
    exec.spawn_wired(
        sync_2ff(clk_slow.clone(), flag_to_sync, sync_q_port),
        vec![dh_sync_q],
        sync_reads,
    );
    exec.spawn_wired(
        slow_consumer(clk_slow.clone(), sync_to_consumer, consumer_port),
        vec![dh_consumer],
        consumer_reads,
    );

    // Testbench: 2:1 fast:slow ratio.
    // ClkFast ticks twice, then ClkSlow ticks once, per slow cycle.
    //
    // Expected timeline:
    //   slow  fast_count  flag_raw  flag_sync  notes
    //      0           2         0          0
    //      1           4         0          0
    //      2           6         0          0
    //      3           8         1          0  flag latches in fast domain
    //      4          10         1          0  in flight through the synchronizer
    //      5          12         1          1  synchronized — 2-slow-cycle latency ✓
    //      6+         ..         1          1
    //
    // The 2-FF synchronizer adds two destination-clock cycles of latency: the flag
    // is captured by the first flip-flop on one ClkSlow edge and forwarded to the
    // second on the next, which is exactly what the post-edge (hardware-standard)
    // executor convention reproduces. (The old single-cycle figure was an artifact
    // of the previous compressed-execution timing.)
    //
    // Note on assignment order in sync_2ff: `ff2 = ff1` must precede `ff1 = d`
    // because Copper's sequential-forwarding semantics make later assignments in
    // the same segment visible to subsequent reads. Reversing the order would let
    // ff2 see the NEW ff1 immediately — collapsing both FFs into one register and
    // eliminating the metastability guard entirely.

    println!("=== CDC Two-Domain Counter (2:1 fast:slow) ===");
    println!("{:>5}  {:>10}  {:>8}  {:>9}  {}", "slow", "fast_count", "flag_raw", "flag_sync", "");

    let mut all_pass = true;

    for slow_cycle in 0..10usize {
        // Tick fast domain twice (producer runs at 2× rate)
        exec.tick_clock(&mut clk_fast);
        exec.tick_clock(&mut clk_fast);
        // Tick slow domain once (synchronizer and consumer update)
        exec.tick_clock(&mut clk_slow);

        let fast_count = count_obs.read().as_u128() as u8;
        // flag_raw: testbench reads the latched flag from the fast domain.
        // In real hardware this wire is ClkFast-tagged and must not be read
        // directly by ClkSlow logic — that is the CDC violation the synchronizer prevents.
        let flag_raw  = consumer_obs.read(); // we observe sync output, not raw fast signal

        // flag_raw (latched) first asserts at slow_cycle 3 (fast count first reaches 8).
        // The 2-FF synchronizer resolves over two destination-clock edges, so flag_sync
        // appears at slow_cycle 5 (2-slow-cycle latency).
        let flag_sync = consumer_obs.read();
        let expected  = if slow_cycle >= 5 { Logic::One } else { Logic::Zero };
        let pass = flag_sync == expected;
        if !pass { all_pass = false; }

        // Read the raw fast-domain flag separately for display only.
        // (In a full Copper CDC check this read would be flagged as a domain violation.)
        let raw_display = count_obs.read()[3]; // bit 3 of count, not the latched flag

        println!("{:>5}  {:>10}  {:>8}  {:>9}  {}",
            slow_cycle, fast_count,
            if fast_count >= 8 { "1" } else { "0" },  // show latched fast-domain flag
            if flag_sync == Logic::One { "1" } else { "0" },
            if pass { "✓" } else { "✗" });
        let _ = raw_display;
    }

    println!("\n2-slow-cycle synchronizer latency verified: {}",
        if all_pass { "✓" } else { "✗" });
    if !all_pass { std::process::exit(1); }
}
