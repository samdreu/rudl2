//! Differential equivalence harness for the levelized scheduler (item 6, phase 2 —
//! `design_docs/LEVELIZED_SCHEDULING_SCOPE.md`).
//!
//! **The correctness spine of the migration.** The fixpoint scheduler is the
//! oracle; the levelized scheduler is only allowed to ship once it is *provably
//! indistinguishable* from it. This harness runs the **same design + stimulus**
//! under both [`SchedulerMode`]s and asserts **every wire holds an identical value
//! after every phase of every cycle** (`pre_edge_settle` / `post_edge_settle` are
//! stepped in lockstep and compared between). A divergence is localized to the
//! exact design, cycle, and phase that breaks — caught the moment it appears.
//!
//! Designs here deliberately span the topologies the levelized DAG must get right:
//! a combinational chain (producer→consumer ordering), fan-out + fan-in (edge
//! dedup / in-degree accounting), a plain-`Out` sequential register feeding
//! combinational logic, a `RegOut` pipeline (registered outputs must NOT induce
//! combinational edges), and an independent two-clock design (multi-domain task
//! sets, other-domain tasks quiescent under one topo order).
//!
//! Hardware-anchored corpus breadth lives elsewhere: `tests/golden_traces.rs` runs
//! the frozen (Verilator-matched) goldens under the levelized scheduler too, and
//! `tests/poll_order_fuzz.rs` adds it to the poll-order sweep.

use copper_core::port::{registered_wire, wire, In, Out, RegOut};
use copper_core::types::{Bits, Clock, ClockDomain};
use copper_macros::hardware;
use copper_sim::{HardwareExecutor, SchedulerMode};

// Single clock domain for the single-domain scenarios.
struct Dm;
impl ClockDomain for Dm {}

// Two independent domains for the multi-domain scenario.
struct Fast;
impl ClockDomain for Fast {}
struct Slow;
impl ClockDomain for Slow {}

// ── modules (real #[hardware] macro path) ─────────────────────────────────────

#[hardware(sequential)]
async fn counter(clk: Clock<Dm>, out: Out<Bits<8>, Dm>) {
    let mut v = Bits::<8>::from_lit::<0>();
    loop {
        out.write(v.clone());
        clk.tick().await;
        v = v + Bits::<8>::from_u8(1);
    }
}

/// Combinational `out = in + 1`.
#[hardware(combinational)]
fn add_one(in_i: In<Bits<8>, Dm>, out: Out<Bits<8>, Dm>) {
    out.write(in_i.read() + Bits::<8>::from_u8(1));
}

/// Combinational `out = a + b` — the fan-in node.
#[hardware(combinational)]
fn add2(a: In<Bits<8>, Dm>, b: In<Bits<8>, Dm>, out: Out<Bits<8>, Dm>) {
    out.write(a.read() + b.read());
}

/// A plain-`Out` register: `q` captures `d` at the edge.
#[hardware(sequential)]
async fn dff(clk: Clock<Dm>, d: In<Bits<8>, Dm>, q: Out<Bits<8>, Dm>) {
    loop {
        q.write(d.read());
        clk.tick().await;
    }
}

/// A registered-output accumulator: `out` (a `RegOut`) trails the running sum by
/// one cycle. Its output must be a phase *source* in the graph, never a comb sink.
#[hardware(sequential)]
async fn accum(clk: Clock<Dm>, a: In<Bits<8>, Dm>, out: RegOut<Bits<8>, Dm>) {
    let mut acc = Bits::<8>::from_lit::<0>();
    loop {
        acc = acc + a.read();
        out.write(acc.clone());
        clk.tick().await;
    }
}

/// Combinational OR gate: `out = ext | fb`. Wired with `fb` fed back from `out`
/// (through `buf`) this forms a set-dominant latch — a *convergent* combinational
/// loop (monotone: a set bit never clears), the case the per-SCC iteration exists
/// for. Not caught by the oscillation threshold, so it stays a legal iterated SCC.
#[hardware(combinational)]
fn or_gate(ext: In<Bits<8>, Dm>, fb: In<Bits<8>, Dm>, out: Out<Bits<8>, Dm>) {
    out.write(ext.read() | fb.read());
}

/// Combinational buffer `out = in` — closes the feedback path of the latch.
#[hardware(combinational)]
fn buf(in_i: In<Bits<8>, Dm>, out: Out<Bits<8>, Dm>) {
    out.write(in_i.read());
}

#[hardware(sequential)]
async fn counter_fast(clk: Clock<Fast>, out: Out<Bits<8>, Fast>) {
    let mut v = Bits::<8>::from_lit::<0>();
    loop {
        out.write(v.clone());
        clk.tick().await;
        v = v + Bits::<8>::from_u8(1);
    }
}

#[hardware(combinational)]
fn add_one_fast(in_i: In<Bits<8>, Fast>, out: Out<Bits<8>, Fast>) {
    out.write(in_i.read() + Bits::<8>::from_u8(1));
}

#[hardware(sequential)]
async fn counter_slow(clk: Clock<Slow>, out: Out<Bits<8>, Slow>) {
    let mut v = Bits::<8>::from_lit::<0>();
    loop {
        out.write(v.clone());
        clk.tick().await;
        v = v + Bits::<8>::from_u8(1);
    }
}

#[hardware(combinational)]
fn add_one_slow(in_i: In<Bits<8>, Slow>, out: Out<Bits<8>, Slow>) {
    out.write(in_i.read() + Bits::<8>::from_u8(1));
}

// ── single-domain harness ─────────────────────────────────────────────────────

/// One scheduler's instantiation of a single-domain design: an executor, its
/// clock, and typed closures to drive the inputs and to read *every* wire.
struct Instance {
    clk: Clock<Dm>,
    exec: HardwareExecutor,
    drive: Box<dyn Fn(&[u128])>,
    snapshot: Box<dyn Fn() -> Vec<u128>>,
}

/// Build the design under both schedulers, drive identical stimulus, and assert
/// every wire agrees after **each phase** (pre-edge and post-edge) of every cycle.
fn assert_schedulers_agree(
    name: &str,
    build: impl Fn(SchedulerMode) -> Instance,
    stimulus: &[Vec<u128>],
) {
    let mut fx = build(SchedulerMode::Fixpoint);
    let mut lv = build(SchedulerMode::Levelized);

    // Baseline: agree at reset, before any clocking.
    assert_eq!(
        (fx.snapshot)(),
        (lv.snapshot)(),
        "{name}: divergence at reset (before cycle 0)"
    );

    for (cyc, step) in stimulus.iter().enumerate() {
        (fx.drive)(step);
        (lv.drive)(step);

        fx.exec.pre_edge_settle::<Dm>();
        lv.exec.pre_edge_settle::<Dm>();
        assert_eq!(
            (fx.snapshot)(),
            (lv.snapshot)(),
            "{name}: levelized diverged from fixpoint at cycle {cyc}, PRE-edge settle"
        );

        fx.exec.post_edge_settle::<Dm>(&mut fx.clk);
        lv.exec.post_edge_settle::<Dm>(&mut lv.clk);
        assert_eq!(
            (fx.snapshot)(),
            (lv.snapshot)(),
            "{name}: levelized diverged from fixpoint at cycle {cyc}, POST-edge settle"
        );
    }
}

/// Convenience: `n` cycles of empty stimulus (for self-driven designs).
fn no_input(n: usize) -> Vec<Vec<u128>> {
    vec![vec![]; n]
}

// ── scenarios ─────────────────────────────────────────────────────────────────

/// counter → add_one → add_one: a pure combinational chain downstream of a
/// register. Producer-before-consumer ordering; one topo pass must settle it.
#[test]
fn diff_comb_chain() {
    fn build(mode: SchedulerMode) -> Instance {
        let clk = Clock::<Dm>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);

        let (c_out, c_in) = wire::<Bits<8>, Dm>(Bits::zero());
        let (a_out, a_in) = wire::<Bits<8>, Dm>(Bits::zero());
        let (b_out, b_in) = wire::<Bits<8>, Dm>(Bits::zero());

        let cd = c_out.dirty_handle();
        exec.spawn_wired(counter(clk.clone(), c_out), vec![cd], vec![]);

        let ad = a_out.dirty_handle();
        let a_reads = vec![c_in.wire_id()];
        let c_probe = c_in.clone();
        exec.spawn_wired(add_one(c_in, a_out), vec![ad], a_reads);

        let bd = b_out.dirty_handle();
        let b_reads = vec![a_in.wire_id()];
        let a_probe = a_in.clone();
        exec.spawn_wired(add_one(a_in, b_out), vec![bd], b_reads);

        Instance {
            clk,
            exec,
            drive: Box::new(|_| {}),
            snapshot: Box::new(move || {
                vec![
                    c_probe.read().as_u128(),
                    a_probe.read().as_u128(),
                    b_in.read().as_u128(),
                ]
            }),
        }
    }
    assert_schedulers_agree("comb_chain", build, &no_input(8));
}

/// Diamond: counter fans out to two add_one nodes whose outputs fan into add2.
/// Exercises edge dedup and in-degree accounting (add2 has two in-edges).
#[test]
fn diff_diamond_fanout_fanin() {
    fn build(mode: SchedulerMode) -> Instance {
        let clk = Clock::<Dm>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);

        let (c_out, c_in) = wire::<Bits<8>, Dm>(Bits::zero());
        let (x_out, x_in) = wire::<Bits<8>, Dm>(Bits::zero());
        let (y_out, y_in) = wire::<Bits<8>, Dm>(Bits::zero());
        let (z_out, z_in) = wire::<Bits<8>, Dm>(Bits::zero());

        let cd = c_out.dirty_handle();
        exec.spawn_wired(counter(clk.clone(), c_out), vec![cd], vec![]);

        // Two consumers of the same producer wire `c` (fan-out).
        let c_for_x = c_in.clone();
        let x_reads = vec![c_for_x.wire_id()];
        let xd = x_out.dirty_handle();
        exec.spawn_wired(add_one(c_for_x, x_out), vec![xd], x_reads);

        let y_reads = vec![c_in.wire_id()];
        let c_probe = c_in.clone();
        let yd = y_out.dirty_handle();
        exec.spawn_wired(add_one(c_in, y_out), vec![yd], y_reads);

        // Fan-in: add2 reads both x and y.
        let z_reads = vec![x_in.wire_id(), y_in.wire_id()];
        let x_probe = x_in.clone();
        let y_probe = y_in.clone();
        let zd = z_out.dirty_handle();
        exec.spawn_wired(add2(x_in, y_in, z_out), vec![zd], z_reads);

        Instance {
            clk,
            exec,
            drive: Box::new(|_| {}),
            snapshot: Box::new(move || {
                vec![
                    c_probe.read().as_u128(),
                    x_probe.read().as_u128(),
                    y_probe.read().as_u128(),
                    z_in.read().as_u128(),
                ]
            }),
        }
    }
    assert_schedulers_agree("diamond", build, &no_input(8));
}

/// A testbench-driven plain-`Out` register feeding combinational logic:
/// input `d` → dff → `q` → add_one → `r`. Stimulus drives `d` each cycle.
#[test]
fn diff_register_then_comb() {
    fn build(mode: SchedulerMode) -> Instance {
        let clk = Clock::<Dm>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);

        let (d_drv, d_in) = wire::<Bits<8>, Dm>(Bits::zero());
        let (q_out, q_in) = wire::<Bits<8>, Dm>(Bits::zero());
        let (r_out, r_in) = wire::<Bits<8>, Dm>(Bits::zero());

        let qd = q_out.dirty_handle();
        let dff_reads = vec![d_in.wire_id()];
        let d_probe = d_in.clone();
        exec.spawn_wired(dff(clk.clone(), d_in, q_out), vec![qd], dff_reads);

        let rd = r_out.dirty_handle();
        let add_reads = vec![q_in.wire_id()];
        let q_probe = q_in.clone();
        exec.spawn_wired(add_one(q_in, r_out), vec![rd], add_reads);

        Instance {
            clk,
            exec,
            drive: Box::new(move |step| d_drv.write(Bits::<8>::from_u8(step[0] as u8))),
            snapshot: Box::new(move || {
                vec![
                    d_probe.read().as_u128(),
                    q_probe.read().as_u128(),
                    r_in.read().as_u128(),
                ]
            }),
        }
    }
    let stim: Vec<Vec<u128>> = [3u128, 7, 7, 1, 0, 9, 9, 2].iter().map(|&v| vec![v]).collect();
    assert_schedulers_agree("register_then_comb", build, &stim);
}

/// A `RegOut` accumulator feeding a combinational consumer. The registered output
/// commits at the edge, so it must be treated as a phase source (no comb edge) —
/// yet both schedulers must still agree cycle-by-cycle.
#[test]
fn diff_regout_pipeline() {
    fn build(mode: SchedulerMode) -> Instance {
        let clk = Clock::<Dm>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);

        let (a_drv, a_in) = wire::<Bits<8>, Dm>(Bits::zero());
        let (acc_out, acc_in) = registered_wire::<Bits<8>, Dm>(&clk, Bits::zero());
        let (r_out, r_in) = wire::<Bits<8>, Dm>(Bits::zero());

        let accd = acc_out.dirty_handle();
        let acc_reads = vec![a_in.wire_id()];
        let a_probe = a_in.clone();
        exec.spawn_wired(accum(clk.clone(), a_in, acc_out), vec![accd], acc_reads);

        // Combinational consumer of the registered output.
        let rd = r_out.dirty_handle();
        let r_reads = vec![acc_in.wire_id()];
        let acc_probe = acc_in.clone();
        exec.spawn_wired(add_one(acc_in, r_out), vec![rd], r_reads);

        Instance {
            clk,
            exec,
            drive: Box::new(move |step| a_drv.write(Bits::<8>::from_u8(step[0] as u8))),
            snapshot: Box::new(move || {
                vec![
                    a_probe.read().as_u128(),
                    acc_probe.read().as_u128(),
                    r_in.read().as_u128(),
                ]
            }),
        }
    }
    let stim: Vec<Vec<u128>> = [1u128, 2, 3, 4, 5, 0, 7, 1].iter().map(|&v| vec![v]).collect();
    assert_schedulers_agree("regout_pipeline", build, &stim);
}

/// A **convergent combinational cycle**: `or_gate(ext, b) → a`, `buf(a) → b`, with
/// `b` fed back into `or_gate`. The two modules form one strongly-connected
/// component in the comb graph, so the levelized scheduler must iterate them to a
/// fixpoint *within the SCC* (`iterate_scc`) rather than single-pass them — and
/// must reach the same set-latch state as the fixpoint scheduler every phase.
#[test]
fn diff_convergent_comb_loop() {
    fn build(mode: SchedulerMode) -> Instance {
        let clk = Clock::<Dm>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);

        let (ext_drv, ext_in) = wire::<Bits<8>, Dm>(Bits::zero());
        let (a_out, a_in) = wire::<Bits<8>, Dm>(Bits::zero());
        let (b_out, b_in) = wire::<Bits<8>, Dm>(Bits::zero());

        // or_gate reads the external input and the feedback wire `b`, drives `a`.
        let od = a_out.dirty_handle();
        let or_reads = vec![ext_in.wire_id(), b_in.wire_id()];
        let ext_probe = ext_in.clone();
        let b_feedback = b_in.clone();
        exec.spawn_wired(or_gate(ext_in, b_feedback, a_out), vec![od], or_reads);

        // buf reads `a`, drives the feedback wire `b` — closing the cycle.
        let bd = b_out.dirty_handle();
        let buf_reads = vec![a_in.wire_id()];
        let a_probe = a_in.clone();
        exec.spawn_wired(buf(a_in, b_out), vec![bd], buf_reads);

        Instance {
            clk,
            exec,
            drive: Box::new(move |step| ext_drv.write(Bits::<8>::from_u8(step[0] as u8))),
            snapshot: Box::new(move || {
                vec![
                    ext_probe.read().as_u128(),
                    a_probe.read().as_u128(),
                    b_in.read().as_u128(),
                ]
            }),
        }
    }
    // Pulses set bits that then latch (monotone) — values must evolve identically.
    let stim: Vec<Vec<u128>> =
        [1u128, 0, 2, 0, 0, 0x80, 0, 4].iter().map(|&v| vec![v]).collect();
    assert_schedulers_agree("convergent_comb_loop", build, &stim);
}

// ── multi-domain scenario ─────────────────────────────────────────────────────

/// Two independent clock domains, each a counter→add_one comb chain, ticked in an
/// uneven interleaving. The topo order is global; when one domain's clock ticks,
/// the other domain's tasks are quiescent sources (their phase barriers keep them
/// Pending) and being polled once in the pass must not perturb them. Compared
/// after each tick (phase stepping is per-domain here).
#[test]
fn diff_multi_domain_independent() {
    struct MdInstance {
        clk_fast: Clock<Fast>,
        clk_slow: Clock<Slow>,
        exec: HardwareExecutor,
        snapshot: Box<dyn Fn() -> Vec<u128>>,
    }

    fn build(mode: SchedulerMode) -> MdInstance {
        let clk_fast = Clock::<Fast>::new();
        let clk_slow = Clock::<Slow>::new();
        let mut exec = HardwareExecutor::new().with_scheduler_mode(mode);

        let (fc_out, fc_in) = wire::<Bits<8>, Fast>(Bits::zero());
        let (fa_out, fa_in) = wire::<Bits<8>, Fast>(Bits::zero());
        let (sc_out, sc_in) = wire::<Bits<8>, Slow>(Bits::zero());
        let (sa_out, sa_in) = wire::<Bits<8>, Slow>(Bits::zero());

        let fcd = fc_out.dirty_handle();
        exec.spawn_wired(counter_fast(clk_fast.clone(), fc_out), vec![fcd], vec![]);
        let fad = fa_out.dirty_handle();
        let fa_reads = vec![fc_in.wire_id()];
        let fc_probe = fc_in.clone();
        exec.spawn_wired(add_one_fast(fc_in, fa_out), vec![fad], fa_reads);

        let scd = sc_out.dirty_handle();
        exec.spawn_wired(counter_slow(clk_slow.clone(), sc_out), vec![scd], vec![]);
        let sad = sa_out.dirty_handle();
        let sa_reads = vec![sc_in.wire_id()];
        let sc_probe = sc_in.clone();
        exec.spawn_wired(add_one_slow(sc_in, sa_out), vec![sad], sa_reads);

        MdInstance {
            clk_fast,
            clk_slow,
            exec,
            snapshot: Box::new(move || {
                vec![
                    fc_probe.read().as_u128(),
                    fa_in.read().as_u128(),
                    sc_probe.read().as_u128(),
                    sa_in.read().as_u128(),
                ]
            }),
        }
    }

    // 0 = tick Fast, 1 = tick Slow.
    let schedule = [0u8, 0, 1, 0, 1, 1, 0, 1, 0, 0];
    let mut fx = build(SchedulerMode::Fixpoint);
    let mut lv = build(SchedulerMode::Levelized);
    assert_eq!((fx.snapshot)(), (lv.snapshot)(), "multi_domain: reset divergence");

    for (i, &which) in schedule.iter().enumerate() {
        if which == 0 {
            fx.exec.tick_clock(&mut fx.clk_fast);
            lv.exec.tick_clock(&mut lv.clk_fast);
        } else {
            fx.exec.tick_clock(&mut fx.clk_slow);
            lv.exec.tick_clock(&mut lv.clk_slow);
        }
        assert_eq!(
            (fx.snapshot)(),
            (lv.snapshot)(),
            "multi_domain: levelized diverged from fixpoint after schedule step {i} (domain {which})"
        );
    }
}
