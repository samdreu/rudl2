//! Behavioral test of the library clock-domain synchronizer `copper::sync_2ff`.
//!
//! Drives a single-bit signal tagged with a `Fast` source domain into a `Slow`
//! destination domain through the synchronizer, and checks it arrives delayed
//! (registered), not combinationally. Companion to the compile-time guarantees
//! in `copper-core/src/cdc.rs` and `src/sync.rs` — those prove the *illegal*
//! crossings don't compile; this proves the *legal* one behaves correctly.

use copper::sync_2ff;
use copper_core::port::wire;
use copper_core::{Clock, ClockDomain, Logic};
use copper_sim::HardwareExecutor;

struct Fast;
impl ClockDomain for Fast {}
struct Slow;
impl ClockDomain for Slow {}

fn logic(b: bool) -> Logic {
    if b {
        Logic::One
    } else {
        Logic::Zero
    }
}

#[test]
fn sync_2ff_crosses_delayed_not_combinational() {
    let mut clk = Clock::<Slow>::new();
    let mut exec = HardwareExecutor::new();

    // The source wire is tagged `Fast`; the synchronizer's output is `Slow`.
    let (d_drv, d_in) = wire::<Logic, Fast>(Logic::Zero);
    let (q_out, q_obs) = wire::<Logic, Slow>(Logic::Zero);
    let dh = q_out.dirty_handle();
    exec.spawn_wired(sync_2ff(clk.clone(), d_in, q_out), vec![dh]);

    // A pulse train on the fast-domain input.
    let stimulus = [false, true, true, true, false, false, true, false, true, true];

    // The synchronizer is registered, so the output is the input from one slow
    // cycle earlier (never the same cycle) — i.e. a pure delay, and in particular
    // the first cycle's output must be the reset value, not the driven value.
    let mut prev = false;
    for (i, &bit) in stimulus.iter().enumerate() {
        d_drv.write(logic(bit));
        exec.tick_clock(&mut clk);
        assert_eq!(
            q_obs.read(),
            logic(prev),
            "cycle {i}: synchronized output should be the previous cycle's input",
        );
        prev = bit;
    }
}

#[test]
fn sync_2ff_settles_to_a_held_input() {
    let mut clk = Clock::<Slow>::new();
    let mut exec = HardwareExecutor::new();

    let (d_drv, d_in) = wire::<Logic, Fast>(Logic::Zero);
    let (q_out, q_obs) = wire::<Logic, Slow>(Logic::Zero);
    let dh = q_out.dirty_handle();
    exec.spawn_wired(sync_2ff(clk.clone(), d_in, q_out), vec![dh]);

    // Hold the input high; after the synchronizer latency the output is high and
    // stays high.
    d_drv.write(Logic::One);
    for _ in 0..4 {
        exec.tick_clock(&mut clk);
    }
    assert_eq!(q_obs.read(), Logic::One, "held-high input should propagate");

    // Drop it; after the latency the output returns low.
    d_drv.write(Logic::Zero);
    for _ in 0..4 {
        exec.tick_clock(&mut clk);
    }
    assert_eq!(q_obs.read(), Logic::Zero, "held-low input should propagate");
}
