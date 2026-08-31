//! P3 — behavioral test for `examples/cdc/two_domain_counter.rs`.
//!
//! The example self-checks only the *boundary*: that the synchronized flag is high
//! from slow cycle 5 under a 2:1 interleaving. That is one number, at one clock
//! ratio, on the last signal in the chain — it passes equally well when the stages
//! are individually wrong and their errors cancel, which is exactly what happened to
//! `two_domain_hierarchy_cdc.rs` (see `design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md`).
//!
//! This test asserts the **per-stage decomposition** instead, at three clock ratios,
//! so a compensating pair of errors cannot hide:
//!
//!   `count` → `flag_raw` (fast domain) → `sync_q` (2-FF synchronizer) → `consumer`
//!
//! The properties are stated so they hold at *every* ratio rather than pinning a
//! cycle number, because a rate-dependent assertion tells you nothing about whether
//! the crossing is actually rate-independent.
//!
//! Note the example itself observes a *proxy* for `flag_raw` (`fast_count >= 8`)
//! rather than the wire. This test reads the real wire, by cloning the `In` before it
//! is moved into the synchronizer.

mod common;

use copper::sync_2ff;
use copper_core::port::{wire, In, Out};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct ClkFast;
impl ClockDomain for ClkFast {}
struct ClkSlow;
impl ClockDomain for ClkSlow {}

// Kept identical to `examples/cdc/two_domain_counter.rs`.
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
        if count[3] == Logic::One {
            latched = Logic::One;
        }
        count = count + Bits::from_lit::<1>();
    }
}

#[hardware(sequential)]
async fn slow_consumer(clk: Clock<ClkSlow>, flag_in: In<Logic, ClkSlow>, out: Out<Logic, ClkSlow>) {
    loop {
        out.write(flag_in.read());
        clk.tick().await;
    }
}

/// One row per slow cycle: `(count, flag_raw, sync_q, consumer)`.
fn trace(fast_per_slow: usize, slow_cycles: usize) -> Vec<(u8, u8, u8, u8)> {
    let mut clk_fast = Clock::<ClkFast>::new();
    let mut clk_slow = Clock::<ClkSlow>::new();
    let mut exec = HardwareExecutor::new();

    let (count_port, count_obs) = wire::<Bits<8>, ClkFast>(Bits::zero());
    let (flag_port, flag_in) = wire::<Logic, ClkFast>(Logic::Zero);
    let (sync_q_port, sync_in) = wire::<Logic, ClkSlow>(Logic::Zero);
    let (consumer_port, consumer_obs) = wire::<Logic, ClkSlow>(Logic::Zero);

    // Observe the intermediate wires directly — `In` is `Clone`, so a copy can be
    // kept before the original is moved into the downstream module.
    let (flag_obs, sync_obs) = (flag_in.clone(), sync_in.clone());

    let dh = (
        count_port.dirty_handle(),
        flag_port.dirty_handle(),
        sync_q_port.dirty_handle(),
        consumer_port.dirty_handle(),
    );
    let sync_reads = vec![flag_in.wire_id()];
    let cons_reads = vec![sync_in.wire_id()];

    exec.spawn_wired(
        fast_counter(clk_fast.clone(), count_port, flag_port),
        vec![dh.0, dh.1],
        vec![],
    );
    exec.spawn_wired(sync_2ff(clk_slow.clone(), flag_in, sync_q_port), vec![dh.2], sync_reads);
    exec.spawn_wired(
        slow_consumer(clk_slow.clone(), sync_in, consumer_port),
        vec![dh.3],
        cons_reads,
    );

    let bit = |l: Logic| u8::from(l == Logic::One);
    (0..slow_cycles)
        .map(|_| {
            for _ in 0..fast_per_slow {
                exec.tick_clock(&mut clk_fast);
            }
            exec.tick_clock(&mut clk_slow);
            (
                count_obs.read().as_u128() as u8,
                bit(flag_obs.read()),
                bit(sync_obs.read()),
                bit(consumer_obs.read()),
            )
        })
        .collect()
}

/// First slow cycle at which `field` is high.
fn first_high(rows: &[(u8, u8, u8, u8)], field: fn(&(u8, u8, u8, u8)) -> u8) -> usize {
    rows.iter().position(|r| field(r) == 1).expect("signal must assert within the window")
}

const RATIOS: [usize; 3] = [1, 2, 3];

#[test]
fn synchronizer_costs_exactly_one_slow_cycle_at_every_ratio() {
    // The CDC property that matters: the 2-FF synchronizer's observable latency is
    // denominated in *destination* cycles and does not depend on how fast the source
    // domain runs. Matches the isolated measurement in
    // `tests/cdc_synchronizer_anchor.rs`, now confirmed in a real design.
    for ratio in RATIOS {
        let rows = trace(ratio, 14);
        let raw = first_high(&rows, |r| r.1);
        let sync = first_high(&rows, |r| r.2);
        assert_eq!(
            sync,
            raw + 1,
            "{ratio}:1 — synchronizer latency should be one slow cycle, got {} (flag_raw@{raw}, sync_q@{sync})",
            sync - raw
        );
    }
}

#[test]
fn the_consumer_tracks_the_synchronizer_with_no_added_latency() {
    // `slow_consumer` is a combinational passthrough (`assign out = flag_in`), so it
    // must add zero cycles. It used to add one — the D2 divergence — and that error
    // cancelled against a one-cycle-early flag, making the hierarchy anchor pass for
    // the wrong reason. Asserted per-stage here so the cancellation cannot recur.
    for ratio in RATIOS {
        let rows = trace(ratio, 14);
        assert_eq!(
            first_high(&rows, |r| r.3),
            first_high(&rows, |r| r.2),
            "{ratio}:1 — the passthrough must track the synchronizer, not lag it"
        );
        for (i, r) in rows.iter().enumerate() {
            assert_eq!(r.3, r.2, "{ratio}:1 cycle {i} — consumer must equal sync_q every cycle");
        }
    }
}

#[test]
fn the_crossing_is_monotone_once_synchronized() {
    // A flag that glitches low after synchronizing is the classic CDC failure. The
    // sticky latch never clears, so neither may anything downstream of it.
    for ratio in RATIOS {
        let rows = trace(ratio, 14);
        for (name, field) in [
            ("flag_raw", (|r: &(u8, u8, u8, u8)| r.1) as fn(&(u8, u8, u8, u8)) -> u8),
            ("sync_q", |r| r.2),
            ("consumer", |r| r.3),
        ] {
            let at = first_high(&rows, field);
            assert!(
                rows[at..].iter().all(|r| field(r) == 1),
                "{ratio}:1 — {name} glitched low after asserting"
            );
        }
    }
}

#[test]
fn the_flag_follows_the_threshold_it_latches() {
    // The design's actual intent: the flag is sticky on `count[3]`, i.e. it latches
    // once the counter has reached 8. It must not assert before the counter gets
    // there — a flag that leads its own trigger would mean the sticky update ran a
    // phase early, which is the pre-tick alignment hazard.
    for ratio in RATIOS {
        let rows = trace(ratio, 14);
        let raw = first_high(&rows, |r| r.1);
        let reached = rows
            .iter()
            .position(|r| r.0 >= 8)
            .expect("counter must reach the threshold in the window");
        assert!(
            raw >= reached,
            "{ratio}:1 — flag asserted at {raw} but the counter only reached 8 at {reached}"
        );
    }
}

#[test]
fn the_counter_advances_at_the_source_rate() {
    // Sanity on the producer side: the fast domain really is running `ratio` times
    // per slow cycle, so the rate-independence claims above are testing what they
    // say they are.
    for ratio in RATIOS {
        let rows = trace(ratio, 10);
        let counts: Vec<u8> = rows.iter().map(|r| r.0).collect();
        let expected: Vec<u8> = (1..=10).map(|i| (i * ratio) as u8).collect();
        assert_eq!(counts, expected, "{ratio}:1 — counter did not advance at the source rate");
    }
}
