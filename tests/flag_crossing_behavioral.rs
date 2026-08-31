//! P3 — behavioral test for `examples/cdc/flag_crossing.rs`.
//!
//! The example checks the crossing only for *self-consistency*: it records the
//! simulator's own output and asserts the recording matches itself, with a comment
//! saying outright there is no Verilog reference. That proves the design simulates,
//! not that it behaves. This test asserts the **handoff semantics** instead.
//!
//! The chain is `trigger → event_source → [ClkFast|ClkSlow] → sync_2ff → event_sink`,
//! where both the source and the sink are combinational passthroughs, so the whole
//! crossing should cost exactly the synchronizer's latency and nothing more.
//!
//! The properties below are stated at three clock ratios, because a crossing that
//! only behaves at one fast:slow ratio is not a crossing — and because the
//! rate-independence *is* the claim. The synchronizer's own latency is separately
//! anchored to independent hand-written Verilog in
//! `tests/cdc_synchronizer_anchor.rs`; this test is about the end-to-end path.
//!
//! **The dropped-pulse case is a feature, not a bug** — see
//! `a_pulse_that_falls_before_the_edge_is_dropped`.

mod common;

use copper::sync_2ff;
use copper_core::port::{wire, In, Out};
use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct ClkFast;
impl ClockDomain for ClkFast {}
struct ClkSlow;
impl ClockDomain for ClkSlow {}

// Kept identical to `examples/cdc/flag_crossing.rs`.
#[hardware(sequential)]
async fn event_source(
    clk: Clock<ClkFast>,
    trigger: In<Logic, ClkFast>,
    event: Out<Logic, ClkFast>,
) {
    loop {
        event.write(trigger.read());
        clk.tick().await;
    }
}

#[hardware(sequential)]
async fn event_sink(
    clk: Clock<ClkSlow>,
    event_synced: In<Logic, ClkSlow>,
    seen: Out<Logic, ClkSlow>,
) {
    loop {
        seen.write(event_synced.read());
        clk.tick().await;
    }
}

/// Drive `trigger` per **fast** tick and observe once per **slow** cycle.
///
/// `schedule[i]` is the trigger value at each fast tick of slow cycle `i`, which is
/// what lets a pulse rise *and fall* between two slow edges — the case a bare 2-FF
/// synchronizer cannot see.
///
/// Returns `(event, synced, seen)` per slow cycle.
fn run(schedule: &[Vec<u8>]) -> Vec<(u8, u8, u8)> {
    let mut clk_fast = Clock::<ClkFast>::new();
    let mut clk_slow = Clock::<ClkSlow>::new();
    let mut exec = HardwareExecutor::new();

    let (trig_drv, trig_in) = wire::<Logic, ClkFast>(Logic::Zero);
    let (event_out, event_in) = wire::<Logic, ClkFast>(Logic::Zero);
    let (synced_out, synced_in) = wire::<Logic, ClkSlow>(Logic::Zero);
    let (seen_out, seen_obs) = wire::<Logic, ClkSlow>(Logic::Zero);

    // Observers on the intermediate wires, cloned before the originals are moved.
    let (event_obs, synced_obs) = (event_in.clone(), synced_in.clone());

    let dh = (event_out.dirty_handle(), synced_out.dirty_handle(), seen_out.dirty_handle());
    let source_reads = vec![trig_in.wire_id()];
    let sync_reads = vec![event_in.wire_id()];
    let sink_reads = vec![synced_in.wire_id()];

    exec.spawn_wired(event_source(clk_fast.clone(), trig_in, event_out), vec![dh.0], source_reads);
    exec.spawn_wired(sync_2ff(clk_slow.clone(), event_in, synced_out), vec![dh.1], sync_reads);
    exec.spawn_wired(event_sink(clk_slow.clone(), synced_in, seen_out), vec![dh.2], sink_reads);

    let bit = |l: Logic| u8::from(l == Logic::One);
    schedule
        .iter()
        .map(|cycle| {
            for &v in cycle {
                trig_drv.write(if v == 1 { Logic::One } else { Logic::Zero });
                exec.tick_clock(&mut clk_fast);
            }
            exec.tick_clock(&mut clk_slow);
            (bit(event_obs.read()), bit(synced_obs.read()), bit(seen_obs.read()))
        })
        .collect()
}

const RATIOS: [usize; 3] = [1, 2, 3];

/// Hold the trigger from slow cycle `from` onward, at `ratio` fast ticks per slow.
fn held(ratio: usize, from: usize, cycles: usize) -> Vec<Vec<u8>> {
    (0..cycles).map(|i| vec![u8::from(i >= from); ratio]).collect()
}

fn first_high(rows: &[(u8, u8, u8)], f: fn(&(u8, u8, u8)) -> u8) -> Option<usize> {
    rows.iter().position(|r| f(r) == 1)
}

#[test]
fn a_held_flag_crosses_with_exactly_the_synchronizer_latency() {
    // Both `event_source` and `event_sink` are combinational passthroughs, so the
    // end-to-end cost must be the synchronizer's one slow cycle and nothing more. If
    // a passthrough silently added a cycle — the D2 divergence — this is where it
    // would show up.
    for ratio in RATIOS {
        let rows = run(&held(ratio, 2, 8));
        let event = first_high(&rows, |r| r.0).expect("event must assert");
        let synced = first_high(&rows, |r| r.1).expect("synced must assert");
        let seen = first_high(&rows, |r| r.2).expect("seen must assert");
        assert_eq!(synced, event + 1, "{ratio}:1 — synchronizer should cost one slow cycle");
        assert_eq!(seen, synced, "{ratio}:1 — the sink is a passthrough and must add none");
    }
}

#[test]
fn a_crossed_flag_stays_high() {
    // Metastability-free handoff: once the flag has resolved into the destination
    // domain it must not glitch back low while the source still asserts it.
    for ratio in RATIOS {
        let rows = run(&held(ratio, 2, 10));
        let seen = first_high(&rows, |r| r.2).expect("seen must assert");
        assert!(
            rows[seen..].iter().all(|r| r.2 == 1),
            "{ratio}:1 — `seen` glitched low after the flag had crossed"
        );
    }
}

#[test]
fn a_pulse_standing_at_the_edge_crosses_and_keeps_its_width() {
    // A one-slow-cycle pulse arrives as a one-slow-cycle pulse, delayed by the
    // synchronizer. Neither stretched nor swallowed.
    for ratio in RATIOS {
        let mut schedule = vec![vec![0u8; ratio]; 6];
        schedule[1] = vec![1u8; ratio];
        let rows = run(&schedule);
        let seen = first_high(&rows, |r| r.2).expect("a pulse held across an edge must cross");
        assert_eq!(seen, 2, "{ratio}:1 — expected the pulse at slow cycle 2, got {seen}");
        assert_eq!(
            rows.iter().filter(|r| r.2 == 1).count(),
            1,
            "{ratio}:1 — the pulse should cross exactly once, not stretch or repeat"
        );
    }
}

#[test]
fn a_pulse_that_falls_before_the_edge_is_dropped() {
    // NOT A BUG — this is the defining limitation of a bare 2-FF synchronizer, and
    // the reason a *level* crossing is safe while an *event* crossing needs a toggle
    // or a handshake. The synchronizer samples only at its own clock edge, so a pulse
    // that has already fallen by then never existed as far as the destination domain
    // is concerned. Pinning it keeps the semantics honest rather than letting a
    // future change quietly "fix" it into something hardware cannot do.
    //
    // Needs at least two fast ticks per slow cycle for the pulse to fit between edges.
    for ratio in [2usize, 3] {
        let mut schedule = vec![vec![0u8; ratio]; 6];
        let mut pulse = vec![0u8; ratio];
        pulse[0] = 1; // high on the first fast tick, low again by the slow edge
        schedule[1] = pulse;
        let rows = run(&schedule);
        assert!(
            rows.iter().all(|r| r.2 == 0),
            "{ratio}:1 — a pulse that falls before the destination edge must not cross: {rows:?}"
        );
    }
}

#[test]
fn nothing_crosses_without_a_trigger() {
    // No spurious events: an idle source produces an idle sink, at every ratio.
    for ratio in RATIOS {
        let rows = run(&vec![vec![0u8; ratio]; 8]);
        assert!(
            rows.iter().all(|r| r == &(0, 0, 0)),
            "{ratio}:1 — the crossing invented an event from an idle trigger: {rows:?}"
        );
    }
}
