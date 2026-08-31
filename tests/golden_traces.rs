//! Frozen golden traces (gate G3, `design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md`).
//!
//! The equivalence harness recomputes its `expected` model every run, so it
//! catches "the sim diverged from the model" but NOT "the sim silently changed
//! from yesterday's output" — a refactor that shifts every trace by a cycle can
//! keep passing if the model shifts with it. These frozen snapshots are the
//! bit-exact tripwire: each representative design's simulator trace is captured
//! once and committed to `tests/golden_traces/*.trace`; this test recomputes and
//! asserts byte-equality.
//!
//! The designs span the timing-pattern matrix (see `TIMING_COVERAGE_MATRIX.md`):
//! a plain register (counter), a feedback register (lfsr), a const-generic
//! datapath (shift_register), two sequence detectors (det_110101, det_010 Moore),
//! and a multi-tick pipeline (mac_pipeline).
//!
//! **Updating:** intentional behavioral changes re-bless the goldens with
//!   `BLESS_GOLDEN=1 cargo test --test golden_traces`
//! then commit the diff (review it — an unexpected diff is the bug this guards).

use std::path::PathBuf;

use copper_sim::SchedulerMode;

/// Compare `actual` against the committed golden for `name`, or (re)write it when
/// `BLESS_GOLDEN` is set. A missing golden without `BLESS_GOLDEN` is a hard error
/// so CI can never pass by silently regenerating.
fn check_golden(name: &str, actual: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden_traces");
    let path = dir.join(format!("{name}.trace"));

    if std::env::var_os("BLESS_GOLDEN").is_some() {
        std::fs::create_dir_all(&dir).expect("create golden dir");
        std::fs::write(&path, actual).expect("write golden");
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden trace {}\n  run `BLESS_GOLDEN=1 cargo test --test golden_traces` to create it",
            path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "golden trace drift for `{name}` — the simulator's output changed from the \
         committed snapshot.\nIf this change is intentional, re-bless with \
         `BLESS_GOLDEN=1 cargo test --test golden_traces` and commit the diff."
    );
}

/// One trace line per cycle. Kept deliberately simple/stable so diffs are legible.
fn line(cycle: usize, inputs: &[(&str, u128)], outputs: &[(&str, u128)]) -> String {
    let fmt = |kv: &[(&str, u128)]| {
        kv.iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!("{cycle:>3}: [{}] -> [{}]\n", fmt(inputs), fmt(outputs))
}

// Each design lives in its own module so the fixtures' shared names (`MainClk`,
// the file-scope `enum State`, etc.) don't collide.

mod counter {
    use crate::line;
    use copper_core::port::{wire, In, Out};
    use copper_core::types::{Bits, Clock, ClockDomain};
    use copper_macros::hardware;
    use copper_sim::{HardwareExecutor, SchedulerMode};
    struct MainClk;
    impl ClockDomain for MainClk {}
    include!("fixtures/counter_dut.rs");

    pub fn trace(mode: SchedulerMode) -> String {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);
        let (step_drv, step_in) = wire::<Bits<8>, MainClk>(Bits::from_u8(3));
        let (out_drv, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
        let dh = out_drv.dirty_handle();
        let reads = vec![step_in.wire_id()];
        exec.spawn_wired(counter(clk.clone(), step_in, out_drv), vec![dh], reads);

        let steps = [3u8, 3, 3, 5, 5, 1, 1, 1];
        let mut s = String::new();
        for (i, &st) in steps.iter().enumerate() {
            step_drv.write(Bits::from_u8(st));
            exec.tick_clock(&mut clk);
            s += &line(i, &[("step", st as u128)], &[("out", out_obs.read().as_u128())]);
        }
        s
    }
}

mod lfsr {
    use crate::line;
    use copper_core::port::{wire, In, Out};
    use copper_core::types::{Bits, Clock, ClockDomain, Logic};
    use copper_macros::hardware;
    use copper_sim::{HardwareExecutor, SchedulerMode};
    struct MainClk;
    impl ClockDomain for MainClk {}
    include!("fixtures/lfsr_dut.rs");

    pub fn trace(mode: SchedulerMode) -> String {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);
        let (rst_drv, rst_in) = wire::<Logic, MainClk>(Logic::One);
        let (yumi_drv, yumi_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (o_drv, o_obs) = wire::<Bits<32>, MainClk>(Bits::from_u32(0));
        let dh = o_drv.dirty_handle();
        let reads = vec![rst_in.wire_id(), yumi_in.wire_id()];
        exec.spawn_wired(lfsr(clk.clone(), rst_in, yumi_in, o_drv), vec![dh], reads);

        // one reset cycle, then advance the LFSR by holding yumi high
        let stim: [(u8, u8); 10] = [
            (1, 0), (0, 1), (0, 1), (0, 1), (0, 1), (0, 1), (0, 1), (0, 1), (0, 0), (0, 1),
        ];
        let logic = |b: u8| if b == 1 { Logic::One } else { Logic::Zero };
        let mut s = String::new();
        for (i, &(rst, yumi)) in stim.iter().enumerate() {
            rst_drv.write(logic(rst));
            yumi_drv.write(logic(yumi));
            exec.tick_clock(&mut clk);
            s += &line(
                i,
                &[("reset", rst as u128), ("yumi", yumi as u128)],
                &[("o", o_obs.read().as_u128())],
            );
        }
        s
    }
}

mod shift_register {
    use crate::line;
    use copper_core::port::{wire, In, Out};
    use copper_core::types::{Bits, Clock, ClockDomain, Logic};
    use copper_macros::hardware;
    use copper_sim::{HardwareExecutor, SchedulerMode};
    struct MainClk;
    impl ClockDomain for MainClk {}
    include!("fixtures/shift_register_dut.rs");

    pub fn trace(mode: SchedulerMode) -> String {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);
        let (d_drv, d_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (en_drv, en_in) = wire::<Logic, MainClk>(Logic::One);
        let (dir_drv, dir_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
        let (out_drv, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
        let dh = out_drv.dirty_handle();
        let reads = vec![d_in.wire_id(), en_in.wire_id(), dir_in.wire_id(), rstn_in.wire_id()];
        exec.spawn_wired(
            shift_register::<8, 7>(d_in, clk.clone(), en_in, dir_in, rstn_in, out_drv),
            vec![dh],
            reads,
        );

        // reset, then shift in a bit pattern (left), then switch direction.
        let logic = |b: u8| if b == 1 { Logic::One } else { Logic::Zero };
        // (d, en, dir, rstn)
        let stim: [(u8, u8, u8, u8); 10] = [
            (0, 1, 0, 0), // reset
            (1, 1, 0, 1),
            (0, 1, 0, 1),
            (1, 1, 0, 1),
            (1, 1, 0, 1),
            (0, 1, 1, 1), // shift right now
            (1, 1, 1, 1),
            (0, 0, 1, 1), // disabled: hold
            (1, 1, 1, 1),
            (0, 1, 0, 1),
        ];
        let mut s = String::new();
        for (i, &(d, en, dir, rstn)) in stim.iter().enumerate() {
            d_drv.write(logic(d));
            en_drv.write(logic(en));
            dir_drv.write(logic(dir));
            rstn_drv.write(logic(rstn));
            exec.tick_clock(&mut clk);
            s += &line(
                i,
                &[("d", d as u128), ("en", en as u128), ("dir", dir as u128), ("rstn", rstn as u128)],
                &[("out", out_obs.read().as_u128())],
            );
        }
        s
    }
}

mod pattern_detector {
    use crate::line;
    use copper_core::port::{wire, In, Out};
    use copper_core::types::{Clock, ClockDomain, Logic};
    use copper_macros::hardware;
    use copper_sim::{HardwareExecutor, SchedulerMode};
    struct MainClk;
    impl ClockDomain for MainClk {}
    include!("fixtures/pattern_detector_dut.rs");

    pub fn trace(mode: SchedulerMode) -> String {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);
        let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
        let (in_drv, in_port) = wire::<Logic, MainClk>(Logic::Zero);
        let (out_drv, out_obs) = wire::<Logic, MainClk>(Logic::Zero);
        let dh = out_drv.dirty_handle();
        let reads = vec![rstn_in.wire_id(), in_port.wire_id()];
        exec.spawn_wired(det_110101(clk.clone(), rstn_in, in_port, out_drv), vec![dh], reads);

        // reset, then feed 110101 (a detection) with a trailing overlap.
        let logic = |b: u8| if b == 1 { Logic::One } else { Logic::Zero };
        let stim: [(u8, u8); 10] = [
            (0, 0), (1, 1), (1, 1), (1, 0), (1, 1), (1, 0), (1, 1), (1, 1), (1, 0), (0, 1),
        ];
        let mut s = String::new();
        for (i, &(rstn, b)) in stim.iter().enumerate() {
            rstn_drv.write(logic(rstn));
            in_drv.write(logic(b));
            exec.tick_clock(&mut clk);
            s += &line(
                i,
                &[("rstn", rstn as u128), ("in", b as u128)],
                &[("out", out_obs.read().as_bool() as u128)],
            );
        }
        s
    }
}

mod det_010 {
    use crate::line;
    use copper_core::port::{wire, In, Out};
    use copper_core::types::{Clock, ClockDomain, Logic};
    use copper_macros::hardware;
    use copper_sim::{HardwareExecutor, SchedulerMode};
    struct MainClk;
    impl ClockDomain for MainClk {}
    include!("fixtures/det_010_dut.rs");

    pub fn trace(mode: SchedulerMode) -> String {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);
        let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
        let (in_drv, in_port) = wire::<Logic, MainClk>(Logic::Zero);
        let (out_drv, out_obs) = wire::<Logic, MainClk>(Logic::Zero);
        let dh = out_drv.dirty_handle();
        let reads = vec![rstn_in.wire_id(), in_port.wire_id()];
        exec.spawn_wired(det_010(clk.clone(), rstn_in, in_port, out_drv), vec![dh], reads);

        // reset, then a stream containing "010" twice (overlap on the trailing 0).
        let logic = |b: u8| if b == 1 { Logic::One } else { Logic::Zero };
        let stim: [(u8, u8); 10] = [
            (0, 0), (1, 0), (1, 1), (1, 0), (1, 0), (1, 1), (1, 0), (1, 1), (1, 0), (1, 0),
        ];
        let mut s = String::new();
        for (i, &(rstn, b)) in stim.iter().enumerate() {
            rstn_drv.write(logic(rstn));
            in_drv.write(logic(b));
            exec.tick_clock(&mut clk);
            s += &line(
                i,
                &[("rstn", rstn as u128), ("in", b as u128)],
                &[("out", out_obs.read().as_bool() as u128)],
            );
        }
        s
    }
}

mod mac_pipeline {
    use crate::line;
    use copper_core::port::{wire, In, Out};
    use copper_core::types::{Bits, Clock, ClockDomain};
    use copper_macros::hardware;
    use copper_sim::{HardwareExecutor, SchedulerMode};
    struct MainClk;
    impl ClockDomain for MainClk {}
    include!("fixtures/mac_pipeline_dut.rs");

    pub fn trace(mode: SchedulerMode) -> String {
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);
        let (a_drv, a_in) = wire::<Bits<8>, MainClk>(Bits::zero());
        let (b_drv, b_in) = wire::<Bits<8>, MainClk>(Bits::zero());
        let (c_drv, c_in) = wire::<Bits<8>, MainClk>(Bits::zero());
        let (out_drv, out_obs) = wire::<Bits<8>, MainClk>(Bits::zero());
        let dh = out_drv.dirty_handle();
        let reads = vec![a_in.wire_id(), b_in.wire_id(), c_in.wire_id()];
        exec.spawn_wired(mac_pipeline(clk.clone(), a_in, b_in, c_in, out_drv), vec![dh], reads);

        // three input groups held across the 3-cycle pipeline period.
        let groups: [(u8, u8, u8); 9] = [
            (2, 3, 4), (2, 3, 4), (2, 3, 4),
            (5, 6, 7), (5, 6, 7), (5, 6, 7),
            (10, 10, 5), (10, 10, 5), (10, 10, 5),
        ];
        let mut s = String::new();
        for (i, &(av, bv, cv)) in groups.iter().enumerate() {
            a_drv.write(Bits::from_u8(av));
            b_drv.write(Bits::from_u8(bv));
            c_drv.write(Bits::from_u8(cv));
            exec.tick_clock(&mut clk);
            s += &line(
                i,
                &[("a", av as u128), ("b", bv as u128), ("c", cv as u128)],
                &[("out", out_obs.read().as_u128())],
            );
        }
        s
    }
}

// Each golden is checked under BOTH schedulers against the SAME frozen snapshot.
// The fixpoint trace is the blessed one (no re-bless); asserting the levelized
// trace equals it too makes these hardware-anchored goldens (they match Verilator)
// a corpus-wide differential check of the levelized scheduler (item 6, phase 2).

#[test]
fn counter_trace_is_frozen() {
    check_golden("counter", &counter::trace(SchedulerMode::Fixpoint));
    check_golden("counter", &counter::trace(SchedulerMode::Levelized));
}

#[test]
fn lfsr_trace_is_frozen() {
    check_golden("lfsr", &lfsr::trace(SchedulerMode::Fixpoint));
    check_golden("lfsr", &lfsr::trace(SchedulerMode::Levelized));
}

#[test]
fn shift_register_trace_is_frozen() {
    check_golden("shift_register", &shift_register::trace(SchedulerMode::Fixpoint));
    check_golden("shift_register", &shift_register::trace(SchedulerMode::Levelized));
}

#[test]
fn pattern_detector_trace_is_frozen() {
    check_golden("pattern_detector", &pattern_detector::trace(SchedulerMode::Fixpoint));
    check_golden("pattern_detector", &pattern_detector::trace(SchedulerMode::Levelized));
}

#[test]
fn det_010_trace_is_frozen() {
    check_golden("det_010", &det_010::trace(SchedulerMode::Fixpoint));
    check_golden("det_010", &det_010::trace(SchedulerMode::Levelized));
}

#[test]
fn mac_pipeline_trace_is_frozen() {
    check_golden("mac_pipeline", &mac_pipeline::trace(SchedulerMode::Fixpoint));
    check_golden("mac_pipeline", &mac_pipeline::trace(SchedulerMode::Levelized));
}
