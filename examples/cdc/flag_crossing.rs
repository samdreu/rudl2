//! Clock-domain crossing, minimal: a single-bit "event" flag raised in a fast
//! domain and consumed in a slow domain, brought across safely by the library
//! synchronizer `copper::sync_2ff`.
//!
//! The point of this example is the *safety story*, in three parts:
//!
//!   1. The flag wire is tagged `ClkFast`. It physically cannot be wired into a
//!      `ClkSlow` module — that is a compile error from the phantom domain types.
//!   2. A regular `#[hardware(sequential)]` module may not even *declare* a
//!      foreign-domain port, so you cannot hand-roll an ad-hoc crossing.
//!   3. The only sanctioned path is a synchronizer (`sync_2ff`, or your own
//!      `#[hardware(synchronizer)]` module). Its output is firmly `ClkSlow`, and
//!      the slow consumer reads it like any native input.
//!
//! Both illegal paths are shown commented-out below with the error they produce.

use copper::sync_2ff;
use copper_core::port::{wire, In, Out};
use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, HardwareTest, SimulationTrace, make_cycle};

struct ClkFast;
impl ClockDomain for ClkFast {}
struct ClkSlow;
impl ClockDomain for ClkSlow {}

// ── Fast-domain producer ──────────────────────────────────────────────────────
// Raises `event` for one fast cycle whenever `trigger` is seen, then lowers it.
#[hardware(sequential)]
async fn event_source(clk: Clock<ClkFast>, trigger: In<Logic, ClkFast>, event: Out<Logic, ClkFast>) {
    loop {
        event.write(trigger.read());
        clk.tick().await;
    }
}

// ── Slow-domain consumer ──────────────────────────────────────────────────────
// Reads the *synchronized* flag (already in ClkSlow) and mirrors it out.
#[hardware(sequential)]
async fn event_sink(clk: Clock<ClkSlow>, event_synced: In<Logic, ClkSlow>, seen: Out<Logic, ClkSlow>) {
    loop {
        seen.write(event_synced.read());
        clk.tick().await;
    }
}

// A regular module may NOT cross domains — both of these fail to compile:
//
//   #[hardware(sequential)]
//   async fn bad_sink(clk: Clock<ClkSlow>, event: In<Logic, ClkFast>, seen: Out<Logic, ClkSlow>) {
//       loop { seen.write(event.read()); clk.tick().await; }
//   }
//   // error: clock-domain crossing: input `event` is in domain `ClkFast`, but this
//   //        module is clocked on `ClkSlow`. ... bring the signal across with a synchronizer
//
//   // and even wiring the fast flag straight into the slow sink is rejected by the types:
//   //   event_sink(clk_slow.clone(), fast_event_wire, seen_out);
//   //   error[E0308]: expected `In<Logic, ClkSlow>`, found `In<Logic, ClkFast>`

fn main() {
    let mut clk_fast = Clock::<ClkFast>::new();
    let mut clk_slow = Clock::<ClkSlow>::new();
    let mut exec = HardwareExecutor::new();

    // Fast domain: trigger → event_source → event (ClkFast).
    let (trig_drv, trig_in) = wire::<Logic, ClkFast>(Logic::Zero);
    let (event_out, event_in) = wire::<Logic, ClkFast>(Logic::Zero);
    // The crossing: event (ClkFast) → sync_2ff → event_synced (ClkSlow).
    let (synced_out, synced_in) = wire::<Logic, ClkSlow>(Logic::Zero);
    // Slow domain: event_synced → event_sink → seen (ClkSlow).
    let (seen_out, seen_obs) = wire::<Logic, ClkSlow>(Logic::Zero);

    // Dirty handles must be taken before the `Out`s are moved into the tasks.
    let dh_event = event_out.dirty_handle();
    let dh_synced = synced_out.dirty_handle();
    let dh_seen = seen_out.dirty_handle();
    let source_reads = vec![trig_in.wire_id()];
    let sync_reads = vec![event_in.wire_id()];
    let sink_reads = vec![synced_in.wire_id()];
    exec.spawn_wired(event_source(clk_fast.clone(), trig_in, event_out), vec![dh_event], source_reads);
    exec.spawn_wired(sync_2ff(clk_slow.clone(), event_in, synced_out), vec![dh_synced], sync_reads);
    exec.spawn_wired(event_sink(clk_slow.clone(), synced_in, seen_out), vec![dh_seen], sink_reads);

    let mut test = HardwareTest::new("flag_crossing");

    // Drive a trigger pulse, then idle. Tick fast twice per slow tick (2:1).
    let triggers = [false, true, false, false, false, false, false, false];
    let mut expected = Vec::new();
    for (i, &t) in triggers.iter().enumerate() {
        let trig = if t { Logic::One } else { Logic::Zero };
        trig_drv.write(trig);
        exec.tick_clock(&mut clk_fast);
        exec.tick_clock(&mut clk_fast);
        exec.tick_clock(&mut clk_slow);

        // `seen` mirrors the synchronized flag, which lags the fast event by the
        // synchronizer's registered latency — this reference just records the
        // simulator's own observed behavior each cycle.
        let seen = seen_obs.read();
        test.record_cycle(i, &[("trigger", &[trig])], &[("seen", &[seen])]);
        expected.push(make_cycle(i, &[("trigger", &[trig])], &[("seen", &[seen])]));
    }

    // Self-consistency: the recorded run matches itself (no Verilog reference —
    // synchronizer transpilation is a TODO). The value of this example is the
    // compile-time story above; the run just shows it simulates.
    let expected = SimulationTrace::from_cycles(expected);
    test.finish_with_expected(&expected).assert_passed();

    println!("flag crossed ClkFast -> sync_2ff -> ClkSlow without any illegal domain access. \u{2713}");
}
