//! P3 — the CDC synchronizer primitive, anchored to independent hardware.
//!
//! Closes the last ⚠️ in `design_docs/TIMING_COVERAGE_MATRIX.md` (**pattern 5 —
//! CDC / synchronizer latency**). `tests/two_domain_hierarchy_cdc.rs` already
//! anchors a whole dual-clock *hierarchy*, but it measures the crossing through a
//! counter and a consumer, so the synchronizer's own latency is inferred rather
//! than observed. This file isolates the primitive and measures it directly.
//!
//! It is also the first `cargo test` coverage of the **standard-library**
//! `copper::sync_2ff` (`src/sync.rs`) — the one sanctioned way to cross a clock
//! domain. Until now it was exercised only by `cargo run --example`, and the
//! hierarchy test used a private copy of the body rather than the library module.
//!
//! Four things are cross-checked on the same primitive:
//!
//!   1. the **library generic** `copper::sync_2ff` in the Copper simulator
//!      (ground truth for Copper's semantics),
//!   2. a **concrete specialization** (`tests/fixtures/sync_2ff_dut.rs`) asserted
//!      behaviourally identical in the sim — so anchoring it anchors the library,
//!   3. that specialization **transpiled** and run under Verilator (sim ↔ Copper-SV),
//!   4. an **independent hand-written SystemVerilog reference**
//!      (`examples/cdc/sv/sync_2ff_ref.sv`) — the non-circular anchor: Copper's
//!      synchronizer timing versus an outside implementation of the same textbook
//!      idiom.
//!
//! **Measured latency: one destination cycle at the observation point**, holding a
//! two-flip-flop storage structure. See `synchronizer_behaves_as_two_flops_not_one`
//! for why those are consistent, and the note on `LATENCY_DST_CYCLES` for why the
//! "two-cycle" figure in the CDC examples' prose counts a different path.
//!
//! **Finding, since fixed:** writing these tests surfaced a real under-approximation
//! in the shared register inference — it reported one flip-flop for the 2-FF
//! synchronizer where the sim, independent hardware, and codegen all have two.
//! Fixed 2026-08-21 by the back-edge clause in `Cfg::registers`; the structural
//! check is now `register_inference_matches_the_independent_reference`.
//!
//! If Verilator is not installed the Verilator arms are skipped (the sim arms
//! still run), mirroring the rest of the suite.

use copper::sync_2ff;
use copper_core::port::{wire, In, Out};
use copper_core::{Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

mod common;
use common::verilator_available;
use std::path::Path;
use std::process::Command;

/// Per-invocation nonce for temporary directories. Two tests in one binary can
/// transpile or Verilate the same top module at the same moment, and a directory
/// keyed on the process id alone is then shared: one test's cleanup deletes the
/// file the other's Verilator is about to read (seen on a 96-core host,
/// 2026-09-03). Same rule as the Verilator work dir in CLAUDE.md.
static TMP_NONCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct SrcClk;
impl ClockDomain for SrcClk {}
struct DstClk;
impl ClockDomain for DstClk {}

// The concrete twin of the library module, shared with the transpiler.
include!("fixtures/sync_2ff_dut.rs");
const FIXTURE_SRC: &str = include_str!("fixtures/sync_2ff_dut.rs");

const REFERENCE_SV: &str = "examples/cdc/sv/sync_2ff_ref.sv";

/// Observable latency of `sync_2ff`, in destination cycles, measured at the
/// post-edge observation point: a `d` that is high at destination edge *n*
/// appears on `q` after edge *n+1*.
///
/// This is **not** in tension with the "two destination cycles" figure in
/// `examples/cdc/two_domain_counter.rs`: that prose counts the full path from a
/// *fast-domain register's* output to the slow observation, which includes the
/// producer's own registered delay. The primitive in isolation contributes one
/// observable cycle while holding two flip-flops — see
/// `synchronizer_behaves_as_two_flops_not_one`.
const LATENCY_DST_CYCLES: usize = 1;

// ── Simulation harness ────────────────────────────────────────────────────────

/// Which synchronizer implementation to run — the library generic or the
/// concrete fixture twin. Both are spawned identically; only the body differs.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Dut {
    LibraryGeneric,
    ConcreteFixture,
}

/// Run the synchronizer over a schedule of destination cycles.
///
/// `schedule[i]` is the sequence of `d` values applied one per **source** tick
/// during destination cycle `i`; the destination clock ticks once at the end of
/// each cycle. Returns `q` observed after each destination tick.
///
/// Driving `d` per source tick (rather than once per destination cycle) is what
/// lets a pulse rise *and fall* entirely between two destination edges — the
/// classic missed-pulse case, exercised by `narrow_pulse_between_edges_is_dropped`.
fn trace_with(dut: Dut, schedule: &[Vec<u8>]) -> Vec<u8> {
    let mut clk_src = Clock::<SrcClk>::new();
    let mut clk_dst = Clock::<DstClk>::new();
    let mut exec = HardwareExecutor::new();

    let (d_drv, d_in) = wire::<Logic, SrcClk>(Logic::Zero);
    let (q_out, q_obs) = wire::<Logic, DstClk>(Logic::Zero);
    let dh_q = q_out.dirty_handle();
    let reads = vec![d_in.wire_id()];

    match dut {
        Dut::LibraryGeneric => {
            exec.spawn_wired(sync_2ff(clk_dst.clone(), d_in, q_out), vec![dh_q], reads)
        }
        Dut::ConcreteFixture => {
            exec.spawn_wired(sync_2ff_concrete(clk_dst.clone(), d_in, q_out), vec![dh_q], reads)
        }
    };

    let mut observed = Vec::with_capacity(schedule.len());
    for cycle in schedule {
        for &v in cycle {
            d_drv.write(if v == 1 { Logic::One } else { Logic::Zero });
            exec.tick_clock(&mut clk_src);
        }
        exec.tick_clock(&mut clk_dst);
        observed.push(if q_obs.read() == Logic::One { 1 } else { 0 });
    }
    observed
}

/// The library generic under a uniform `src_per_dst` interleaving, with one `d`
/// value held for the whole destination cycle.
fn trace(src_per_dst: usize, d_seq: &[u8]) -> Vec<u8> {
    let schedule: Vec<Vec<u8>> = d_seq.iter().map(|&d| vec![d; src_per_dst]).collect();
    trace_with(Dut::LibraryGeneric, &schedule)
}

/// The `d` value standing at each destination edge — the only thing a real
/// synchronizer samples, and therefore all the SystemVerilog testbench needs.
fn d_at_edges(schedule: &[Vec<u8>]) -> Vec<u8> {
    schedule.iter().map(|c| *c.last().expect("each cycle drives d at least once")).collect()
}

// ── Directed sim tests (no Verilator needed) ──────────────────────────────────

#[test]
fn sync_2ff_observable_latency_is_one_destination_cycle() {
    // `d` rises before destination edge 0 and is held.
    let observed = trace(1, &[1; 8]);
    let first_high = observed.iter().position(|&q| q == 1).expect("q must eventually assert");
    assert_eq!(
        first_high, LATENCY_DST_CYCLES,
        "sync_2ff observable latency changed; trace = {observed:?}"
    );
    assert!(observed[first_high..].iter().all(|&q| q == 1), "q must stay high: {observed:?}");
}

#[test]
fn sync_2ff_latency_is_measured_from_the_rise_not_from_reset() {
    // `d` rises at destination cycle 3 instead of 0; the latency must follow the
    // rise, not be an artifact of start-up state.
    let d_seq = [0, 0, 0, 1, 1, 1, 1, 1];
    let observed = trace(1, &d_seq);
    let rise = d_seq.iter().position(|&d| d == 1).unwrap();
    let first_high = observed.iter().position(|&q| q == 1).expect("q must eventually assert");
    assert_eq!(first_high, rise + LATENCY_DST_CYCLES, "trace = {observed:?}");
}

#[test]
fn sync_2ff_latency_is_independent_of_source_tick_rate() {
    // The defining CDC property: latency is denominated in *destination* cycles.
    // Running the source domain faster must not change the destination-cycle
    // trace at all.
    let d_seq = [0, 0, 1, 1, 1, 1, 0, 0, 0, 1, 1, 1];
    let baseline = trace(1, &d_seq);
    for src_per_dst in [2, 3, 5, 8] {
        assert_eq!(
            trace(src_per_dst, &d_seq),
            baseline,
            "{src_per_dst}:1 source:destination interleaving changed the destination-cycle trace"
        );
    }
}

#[test]
fn source_domain_ticks_do_not_advance_the_synchronizer() {
    // Quiescence across domains: ticking one clock must leave the other domain's
    // tasks completely still, however much `d` moves in the meantime.
    let mut clk_src = Clock::<SrcClk>::new();
    let clk_dst = Clock::<DstClk>::new();
    let mut exec = HardwareExecutor::new();

    let (d_drv, d_in) = wire::<Logic, SrcClk>(Logic::Zero);
    let (q_out, q_obs) = wire::<Logic, DstClk>(Logic::Zero);
    let dh_q = q_out.dirty_handle();
    let reads = vec![d_in.wire_id()];
    exec.spawn_wired(sync_2ff(clk_dst.clone(), d_in, q_out), vec![dh_q], reads);

    for i in 0..20 {
        d_drv.write(if i % 2 == 0 { Logic::One } else { Logic::Zero });
        exec.tick_clock(&mut clk_src);
        assert_eq!(
            q_obs.read(),
            Logic::Zero,
            "q moved on a SOURCE-domain tick (iteration {i}) — the destination \
             task must be quiescent until its own clock ticks"
        );
    }
}

#[test]
fn narrow_pulse_between_edges_is_dropped() {
    // A pulse that rises and falls entirely between two destination edges is
    // invisible to the synchronizer — it samples `d` only at its own edge. This
    // is correct CDC behaviour (it is why real designs use a toggle/handshake for
    // events rather than a bare 2-FF), and pinning it keeps the semantics honest.
    let schedule = vec![
        vec![0, 0, 0], // cycle 0: idle
        vec![0, 1, 0], // cycle 1: pulse rises AND falls between edges → d==0 at the edge
        vec![0, 0, 0], // cycle 2
        vec![0, 0, 0], // cycle 3
        vec![0, 0, 0], // cycle 4
    ];
    assert_eq!(d_at_edges(&schedule), vec![0, 0, 0, 0, 0], "the pulse must not straddle an edge");
    assert_eq!(
        trace_with(Dut::LibraryGeneric, &schedule),
        vec![0; 5],
        "a pulse that never stands at a destination edge must be dropped"
    );
}

#[test]
fn pulse_held_across_an_edge_is_captured() {
    // The complement of the case above: the same one-source-cycle pulse, moved so
    // that it *is* standing at a destination edge, must propagate.
    let schedule = vec![
        vec![0, 0, 0], // cycle 0: idle
        vec![0, 0, 1], // cycle 1: pulse stands at the edge
        vec![0, 0, 0], // cycle 2
        vec![0, 0, 0], // cycle 3
        vec![0, 0, 0], // cycle 4
    ];
    assert_eq!(d_at_edges(&schedule), vec![0, 1, 0, 0, 0]);
    let observed = trace_with(Dut::LibraryGeneric, &schedule);
    let high = observed.iter().position(|&q| q == 1).expect("a pulse at an edge must be captured");
    assert_eq!(high, 1 + LATENCY_DST_CYCLES, "trace = {observed:?}");
}

/// A **one**-flip-flop "synchronizer" — not a real CDC primitive, just the
/// degenerate neighbour of `sync_2ff`. Its only job is to be the thing
/// `synchronizer_behaves_as_two_flops_not_one` distinguishes the library from.
#[hardware(synchronizer)]
async fn sync_1ff(clk: Clock<DstClk>, d: In<Logic, SrcClk>, q: Out<Logic, DstClk>) {
    let mut ff1 = Logic::Zero;
    loop {
        q.write(ff1);
        clk.tick().await;
        ff1 = d.read();
    }
}

#[test]
fn synchronizer_behaves_as_two_flops_not_one() {
    // The observable latency is one destination cycle, but the *storage* is two
    // flip-flops — that is the whole point of a 2-FF synchronizer (stage 1 absorbs
    // metastability, stage 2 presents a settled value). Those are consistent: with
    // two flops, `q` after edge n is `d` as it stood at edge n-1.
    //
    // Latency and storage are separate claims, so check the storage one by its
    // observable consequence rather than by argument: collapse the two stages into
    // one and the behaviour must change — a single flop asserts a full destination
    // cycle earlier. This is the failure mode `src/sync.rs`'s own comment warns
    // about (reversing the two assignments), and it is what would silently destroy
    // the metastability guard.
    let mut clk_src = Clock::<SrcClk>::new();
    let mut clk_dst = Clock::<DstClk>::new();
    let mut exec = HardwareExecutor::new();
    let (d_drv, d_in) = wire::<Logic, SrcClk>(Logic::Zero);
    let (q_out, q_obs) = wire::<Logic, DstClk>(Logic::Zero);
    let dh_q = q_out.dirty_handle();
    let reads = vec![d_in.wire_id()];
    exec.spawn_wired(sync_1ff(clk_dst.clone(), d_in, q_out), vec![dh_q], reads);

    let mut one_flop = Vec::new();
    for _ in 0..8 {
        d_drv.write(Logic::One);
        exec.tick_clock(&mut clk_src);
        exec.tick_clock(&mut clk_dst);
        one_flop.push(if q_obs.read() == Logic::One { 1 } else { 0 });
    }

    let two_flop = trace(1, &[1; 8]);
    assert_ne!(two_flop, one_flop, "sync_2ff must not behave like a single flop");
    let two_at = two_flop.iter().position(|&q| q == 1).unwrap();
    let one_at = one_flop.iter().position(|&q| q == 1).unwrap();
    assert_eq!(
        two_at,
        one_at + 1,
        "the second stage must cost exactly one more destination cycle \
         (2-FF {two_flop:?} vs 1-FF {one_flop:?})"
    );
}

#[test]
fn register_inference_matches_the_independent_reference() {
    // G2 structural reg-for-reg match against the independent hand-written SV.
    // `NameExact` is warranted here because the reference uses the same stage names
    // as the design (`ff1`, `ff2`) — it is the same textbook idiom, not a
    // differently-encoded reimplementation.
    //
    // HISTORY: until 2026-08-21 this was pinned as a KNOWN GAP. Inference reported
    // one flip-flop here where the simulator's behaviour, this reference, and
    // codegen all have two: `ff2` is defined post-tick and read pre-tick, so its
    // live range crosses the loop back edge but no tick, and the rule keyed only on
    // ticks. Fixed by adding the back-edge clause to `Cfg::registers` (which see for
    // why such a local is a genuine flip-flop and not a wire). Synchronizers had
    // never been reconciled because `register_reconciliation.rs` filtered on
    // `#[hardware(sequential)]`; that filter is now lifted, so this shape is covered
    // corpus-wide too.
    let sv = std::fs::read_to_string(REFERENCE_SV).expect("read independent reference");
    copper_analysis::assert_source_registers_match_reference_sv(
        FIXTURE_SRC,
        Some("sync_2ff_concrete"),
        &sv,
        copper_analysis::RegMatch::NameExact,
    );
}

// ── Verilator arms ────────────────────────────────────────────────────────────

/// Self-checking single-clock testbench: apply the `d` standing at each
/// destination edge, tick `rd_clk`, and check `q` against the Copper sim.
///
/// Only the edge-time value of `d` is applied — a real synchronizer samples
/// nothing else. That makes `narrow_pulse_between_edges_is_dropped` a genuine
/// prediction about the hardware, not a restatement of the sim.
fn testbench(top: &str, d_at_edges: &[u8], expected_q: &[u8]) -> String {
    let mut tb = String::new();
    tb.push_str(&format!("#include \"V{top}.h\"\n#include \"verilated.h\"\n#include <iostream>\n\n"));
    tb.push_str("int main(int argc, char** argv) {\n");
    tb.push_str("    Verilated::commandArgs(argc, argv);\n");
    tb.push_str(&format!("    V{top}* top = new V{top}();\n"));
    tb.push_str("    int failures = 0;\n");
    tb.push_str("    top->rd_clk = 0; top->d = 0; top->eval();\n\n");
    for (i, (&d, &q)) in d_at_edges.iter().zip(expected_q).enumerate() {
        tb.push_str(&format!("    top->d = {d};\n"));
        tb.push_str("    top->rd_clk = 0; top->eval(); top->rd_clk = 1; top->eval();\n");
        tb.push_str(&format!(
            "    if (top->q != {q}) {{ std::cout << \"FAIL cycle {i}: d={d} expected q={q} got \" << (int)top->q << std::endl; failures++; }}\n"
        ));
    }
    tb.push_str("    delete top;\n");
    tb.push_str("    if (failures == 0) { std::cout << \"All tests passed!\" << std::endl; return 0; }\n");
    tb.push_str("    std::cout << failures << \" failure(s)\" << std::endl; return 1;\n");
    tb.push_str("}\n");
    tb
}

/// Verilate `sv_file` with `top`, build `tb_src` against it, run it, and report
/// whether the self-checking testbench passed. Runs in an isolated work dir.
fn run_verilator(sv_file: &Path, top: &str, tb_src: &str) -> Result<(), String> {
    let work = std::env::temp_dir().join(format!("copper_sync2ff_{top}_{}_{}", std::process::id(), TMP_NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("mkdir: {e}"))?;

    let sv_abs =
        std::fs::canonicalize(sv_file).map_err(|e| format!("canonicalize {sv_file:?}: {e}"))?;
    let tb_path = work.join(format!("tb_{top}.cpp"));
    std::fs::write(&tb_path, tb_src).map_err(|e| format!("write tb: {e}"))?;

    let out = common::verilator_command()
        .current_dir(&work)
        .args([
            "--cc",
            "--exe",
            "--build",
            "--top-module",
            top,
            "-Wall",
            "-Wno-DECLFILENAME",
            "-CFLAGS",
            "-std=c++14",
        ])
        .arg(&sv_abs)
        .arg(&tb_path)
        .output()
        .map_err(|e| format!("verilator: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "verilator build failed for {top}:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    let exe = work.join("obj_dir").join(format!("V{top}"));
    let run = Command::new(&exe).output().map_err(|e| format!("run {exe:?}: {e}"))?;
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let ok = run.status.success();
    let _ = std::fs::remove_dir_all(&work);
    if ok { Ok(()) } else { Err(format!("testbench mismatch for {top}:\n{stdout}")) }
}

/// The stimulus every Verilator arm runs: a rise, a fall, a re-rise, a pulse
/// standing at an edge, and a pulse that falls between edges.
fn anchor_schedule() -> Vec<Vec<u8>> {
    vec![
        vec![0, 0, 0],
        vec![0, 0, 0],
        vec![1, 1, 1], // rise, held
        vec![1, 1, 1],
        vec![1, 1, 1],
        vec![0, 0, 0], // fall
        vec![0, 0, 0],
        vec![0, 1, 0], // pulse that falls before the edge (must be invisible)
        vec![0, 0, 0],
        vec![0, 0, 1], // pulse standing at the edge (must be captured)
        vec![0, 0, 0],
        vec![0, 0, 0],
        vec![1, 1, 1], // re-rise, held
        vec![1, 1, 1],
        vec![0, 0, 0],
        vec![0, 0, 0],
    ]
}

#[test]
fn independent_verilog_matches_sim() {
    if !verilator_available() {
        return;
    }
    assert!(Path::new(REFERENCE_SV).exists(), "missing independent reference {REFERENCE_SV}");

    // The non-circular anchor: an outside hand-written 2-FF synchronizer must
    // reproduce Copper's synchronizer timing cycle-for-cycle. Nothing in this arm
    // touches the Copper transpiler.
    let schedule = anchor_schedule();
    let expected = trace_with(Dut::ConcreteFixture, &schedule);
    let tb = testbench("sync_2ff_ref", &d_at_edges(&schedule), &expected);
    run_verilator(Path::new(REFERENCE_SV), "sync_2ff_ref", &tb)
        .expect("independent hand-written SV must match the Copper simulator");
}

#[test]
fn transpiled_sync_2ff_matches_sim() {
    if !verilator_available() {
        return;
    }
    let sv = copper_codegen::transpile_source(
        FIXTURE_SRC,
        Some("sync_2ff_concrete"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("transpile the concrete synchronizer");

    let work = std::env::temp_dir().join(format!("copper_sync2ff_sv_{}_{}", std::process::id(), TMP_NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::create_dir_all(&work).unwrap();
    let sv_path = work.join("sync_2ff_concrete.sv");
    std::fs::write(&sv_path, &sv).unwrap();

    // The transpiled module clocks on `clk`, not `rd_clk`; wrap it so the shared
    // testbench port names apply to both back ends.
    let wrapper = "\nmodule sync_2ff_ref_shim (input logic rd_clk, input logic d, output logic q);\n    sync_2ff_concrete u_dut (.clk(rd_clk), .d(d), .q(q));\nendmodule\n";
    std::fs::write(&sv_path, format!("{sv}{wrapper}")).unwrap();

    let schedule = anchor_schedule();
    let expected = trace_with(Dut::ConcreteFixture, &schedule);
    let tb = testbench("sync_2ff_ref_shim", &d_at_edges(&schedule), &expected);
    let result = run_verilator(&sv_path, "sync_2ff_ref_shim", &tb);
    let _ = std::fs::remove_dir_all(&work);
    result.expect("transpiled synchronizer must match the Copper simulator");
}
