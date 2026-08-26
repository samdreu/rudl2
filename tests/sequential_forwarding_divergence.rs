//! **D1: guarded. D2: fixed.** Two silent sim ≠ synthesized-SV divergences, both from
//! the pre-edge/post-edge alignment of the coroutine's loop segments. Every
//! assertion here records *today's* behaviour so it flips loudly when either is
//! fixed — do not "fix" this file by relaxing it.
//!
//! **Plan of record: `design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md`** — the variant
//! map, the hardware adjudication, the corpus status table, the two rejected fixes,
//! the guardrail acceptance criteria, and the phased plan. Read
//! `SYNCHRONOUS_SEMANTICS.md` §Output timing for context; this is a third member of
//! the same family as the multi-write-around-a-tick collapse.
//!
//! # Divergence 1 — a pre-tick register update is forwarded to a same-segment read
//!
//! ```text
//! loop { r = r + 1; o.write(r); clk.tick().await; }
//!   sim = [2, 3, 4, 5, 6, 7]
//!   sv  = [1, 2, 3, 4, 5, 6]     ← off by one, silently
//! ```
//!
//! The simulator's *sequential forwarding* makes the pre-tick assignment visible to
//! the write that follows it in the same segment. Codegen emits `r <= r + 1`
//! (non-blocking) with `assign o = r`, so the SV drives the *pre*-update value — a
//! flip-flop's Q cannot reflect its own D within the cycle.
//!
//! **The discriminator is a leading `In` read.** A leading read is classified
//! `Deferred` (impl-plan item 3) and injects `pre_edge_barrier()`, which pins the
//! pre-tick segment to the pre-edge phase. With no input read at all there is no
//! barrier, the loop top also runs during the previous tick's post-edge settle, and
//! the update lands an extra time. `add_with_leading_read` below is the identical
//! design plus one input read, and it agrees with its SV exactly.
//!
//! That is why the corpus is green: a sweep of 49 clocked modules found 8 with the
//! update-then-read shape, and the 6 that are equivalence-tested (`lfsr`,
//! `det_110101`, `shift_register`) all gate on input reads. Only `fast_counter`
//! (3 copies) has the shape *without* one.
//!
//! **Adjudicated against independent hardware, not by argument.**
//! `examples/cdc/sv/two_domain_hierarchy.sv::ref_fast_counter` is a hand-written
//! reference committed in `0d67f9e` (item 4, 2026-07-30) — long before this
//! divergence was known, so it is a genuine outside opinion. It agrees with the
//! transpiled SV and disagrees with the simulator: **the simulator is the one that
//! is wrong.**
//!
//! # Divergence 2 — FIXED 2026-08-21
//!
//! `loop { out.write(inp.read()); clk.tick().await; }` transpiles to
//! `assign out = inp;` — zero cycles. Its leading read used to classify `Deferred`,
//! so it sampled at the *pre*-edge while a clocked producer updates at the
//! *post*-edge, and the passthrough lagged one cycle. Adjudicated against
//! independent hand-written Verilog (a clocked producer feeding a passthrough gives
//! `mid == out`), then fixed in `classify_reads`: a read feeding a combinational
//! `Out` in a segment that assigns no register is `Immediate`, because there is no
//! register for a `pre_edge_barrier` to pin. `d2_is_fixed_and_d1_still_demonstrates_the_hazard`
//! guards the fix.
//!
//! # Why nothing caught either one (historical)
//!
//! `tests/two_domain_hierarchy_cdc.rs` — the independent-hardware anchor for the
//! whole dual-clock design — used to be green because these two divergences
//! **cancelled**: D1 made the flag assert a cycle early, D2 made the consumer a cycle
//! late, and the observable boundary landed on the reference's cycle. It took a
//! deliberate experiment (correct one side, watch the boundary move 5 → 6) to see it.
//! Both are resolved now and that anchor passes for the right reason; D1's fixture
//! here keeps the shape expressible via `allow_pretick_alignment` so the hazard stays
//! demonstrable.

mod common;
use common::{verilator_available, verilator_command};
use copper::sync_2ff;
use copper_core::port::{wire, In, Out, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;
use std::path::Path;

struct C;
impl ClockDomain for C {}
struct ClkFast;
impl ClockDomain for ClkFast {}
struct ClkSlow;
impl ClockDomain for ClkSlow {}

const CYCLES: usize = 13;

// ── Divergence 1: minimal case ────────────────────────────────────────────────

const ADD_SRC: &str = r#"
#[hardware(sequential, allow_pretick_alignment)]
async fn add_then_write(clk: Clock<C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        r = r + Bits::from_lit::<1>();
        o.write(r);
        clk.tick().await;
    }
}
"#;

#[hardware(sequential, allow_pretick_alignment)]
async fn add_then_write(clk: Clock<C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        r = r + Bits::from_lit::<1>();
        o.write(r);
        clk.tick().await;
    }
}

/// The *same* design plus one leading `In` read, which installs the pre-edge
/// barrier. Held identical in every other respect so the read is the only variable.
const ADD_READ_SRC: &str = r#"
#[hardware(sequential)]
async fn add_with_leading_read(clk: Clock<C>, en: In<Logic, C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        if en.read() == Logic::One {
            r = r + Bits::from_lit::<1>();
        }
        o.write(r);
        clk.tick().await;
    }
}
"#;

#[hardware(sequential)]
async fn add_with_leading_read(clk: Clock<C>, en: In<Logic, C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        if en.read() == Logic::One {
            r = r + Bits::from_lit::<1>();
        }
        o.write(r);
        clk.tick().await;
    }
}

fn sim_add_then_write() -> Vec<u8> {
    let mut clk = Clock::<C>::new();
    let mut exec = HardwareExecutor::new();
    let (p, obs) = wire::<Bits<8>, C>(Bits::zero());
    let dh = p.dirty_handle();
    exec.spawn_wired(add_then_write(clk.clone(), p), vec![dh], vec![]);
    (0..CYCLES)
        .map(|_| {
            exec.tick_clock(&mut clk);
            obs.read().as_u128() as u8
        })
        .collect()
}

fn sim_add_with_leading_read() -> Vec<u8> {
    let mut clk = Clock::<C>::new();
    let mut exec = HardwareExecutor::new();
    let (en_drv, en_in) = wire::<Logic, C>(Logic::One);
    let (p, obs) = wire::<Bits<8>, C>(Bits::zero());
    let dh = p.dirty_handle();
    let reads = vec![en_in.wire_id()];
    exec.spawn_wired(add_with_leading_read(clk.clone(), en_in, p), vec![dh], reads);
    en_drv.write(Logic::One);
    (0..CYCLES)
        .map(|_| {
            exec.tick_clock(&mut clk);
            obs.read().as_u128() as u8
        })
        .collect()
}

// ── Verilator plumbing ────────────────────────────────────────────────────────

/// Verilate `sv` (already written to `sv_path`) as `top`, tick `clk_port`, and
/// collect the whitespace-separated values each `probe` line prints per cycle.
fn run_sv(sv_path: &Path, top: &str, clk_port: &str, probes: &[&str], extra_init: &str) -> Vec<Vec<u32>> {
    let work = std::env::temp_dir().join(format!("copper_fwd_{top}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();

    let mut tb = format!(
        "#include \"V{top}.h\"\n#include \"verilated.h\"\n#include <iostream>\n\
         int main(int c, char** v) {{ Verilated::commandArgs(c, v);\n\
         V{top}* t = new V{top}(); t->{clk_port} = 0; {extra_init} t->eval();\n"
    );
    let body: String = probes.iter().map(|p| format!("std::cout << (int)t->{p} << \" \";")).collect();
    for _ in 0..CYCLES {
        tb.push_str(&format!(
            "t->{clk_port}=0; t->eval(); t->{clk_port}=1; t->eval(); {body} std::cout << std::endl;\n"
        ));
    }
    tb.push_str("return 0; }\n");
    let tb_path = work.join("tb.cpp");
    std::fs::write(&tb_path, tb).unwrap();

    let out = verilator_command()
        .current_dir(&work)
        .args([
            "--cc", "--exe", "--build", "--top-module", top,
            "-Wno-DECLFILENAME", "-Wno-WIDTHEXPAND", "-CFLAGS", "-std=c++14",
        ])
        .arg(std::fs::canonicalize(sv_path).unwrap())
        .arg(&tb_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "verilator build failed:\n{}", String::from_utf8_lossy(&out.stderr));

    let run = std::process::Command::new(work.join(format!("obj_dir/V{top}"))).output().unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&work);
    stdout
        .lines()
        .map(|l| l.split_whitespace().filter_map(|x| x.parse().ok()).collect())
        .collect()
}

/// Transpile `src`'s `top` to a temp `.sv` and run it, returning one column.
fn transpile_and_run(src: &str, top: &str, clk: &str, probe: &str, init: &str) -> Vec<u8> {
    let sv = copper_codegen::transpile_source(src, Some(top), &copper_codegen::EmitConfig::default())
        .expect("transpile");
    let dir = std::env::temp_dir().join(format!("copper_fwd_src_{top}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("{top}.sv"));
    std::fs::write(&p, &sv).unwrap();
    let rows = run_sv(&p, top, clk, &[probe], init);
    let _ = std::fs::remove_dir_all(&dir);
    rows.into_iter().map(|r| r[0] as u8).collect()
}

// ── Divergence 1 ──────────────────────────────────────────────────────────────

#[test]
fn pre_tick_update_is_forwarded_in_sim_but_not_in_hardware_known_gap() {
    let sim = sim_add_then_write();
    let expected_sim: Vec<u8> = (2..).take(CYCLES).collect();
    assert_eq!(sim, expected_sim, "simulator behaviour changed; if it now starts at 1 the gap is FIXED");

    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(ADD_SRC, "add_then_write", "clk", "o", "");
    let expected_sv: Vec<u8> = (1..).take(CYCLES).collect();
    assert_eq!(sv, expected_sv, "transpiled SV behaviour changed");
    assert_ne!(
        sim, sv,
        "sim and SV now AGREE — divergence 1 is FIXED. Delete this test, promote it \
         to a real equivalence check, and re-bless tests/two_domain_hierarchy_cdc.rs \
         (see compensation_is_what_makes_the_hierarchy_anchor_pass)."
    );
}

#[test]
fn a_leading_input_read_removes_the_divergence() {
    // The discriminator, isolated: identical design, one added input read, and the
    // divergence disappears. This is what makes the gap narrow rather than general,
    // and it is why the equivalence-tested corpus is green.
    let sim = sim_add_with_leading_read();
    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(ADD_READ_SRC, "add_with_leading_read", "clk", "o", "t->en = 1;");
    assert_eq!(
        sim, sv,
        "the leading-read form must stay sim ≡ SV — if this breaks, the pre-edge \
         barrier no longer pins the pre-tick segment"
    );
    assert_eq!(sim, (1..).take(CYCLES).collect::<Vec<u8>>());
}

// ── The hardware adjudication ─────────────────────────────────────────────────

const FAST_COUNTER_SRC: &str = r#"
#[hardware(sequential, allow_pretick_alignment)]
async fn fast_counter(clk: Clock<ClkFast>, count_out: Out<Bits<8>, ClkFast>, flag_out: Out<Logic, ClkFast>) {
    let mut count: Bits<8> = Bits::zero();
    let mut latched = Logic::Zero;
    loop {
        if count[3] == Logic::One { latched = Logic::One; }
        count_out.write(count);
        flag_out.write(latched);
        clk.tick().await;
        count = count + Bits::from_lit::<1>();
    }
}
"#;

#[hardware(sequential, allow_pretick_alignment)]
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

fn sim_fast_counter() -> Vec<(u8, u8)> {
    let mut clk = Clock::<ClkFast>::new();
    let mut exec = HardwareExecutor::new();
    let (cp, co) = wire::<Bits<8>, ClkFast>(Bits::zero());
    let (fp, fo) = wire::<Logic, ClkFast>(Logic::Zero);
    let (dc, df) = (cp.dirty_handle(), fp.dirty_handle());
    exec.spawn_wired(fast_counter(clk.clone(), cp, fp), vec![dc, df], vec![]);
    (0..CYCLES)
        .map(|_| {
            exec.tick_clock(&mut clk);
            (co.read().as_u128() as u8, u8::from(fo.read() == Logic::One))
        })
        .collect()
}

#[test]
fn independent_hardware_sides_with_codegen_against_the_simulator() {
    // The adjudication, run rather than argued. `ref_fast_counter` was committed in
    // `0d67f9e` (2026-07-30) as an outside implementation of this design, long
    // before the divergence was known — so it is a genuine third opinion, not a
    // restatement of either side.
    if !verilator_available() {
        return;
    }
    let sim = sim_fast_counter();

    let dir = std::env::temp_dir().join(format!("copper_fwd_ref_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let full = std::fs::read_to_string("examples/cdc/sv/two_domain_hierarchy.sv")
        .expect("read the independent reference");
    let start = full.find("module ref_fast_counter").expect("ref_fast_counter present");
    let end = full[start..].find("endmodule").unwrap() + start + "endmodule".len();
    let ref_path = dir.join("ref_fast_counter.sv");
    std::fs::write(&ref_path, &full[start..end]).unwrap();
    let independent: Vec<(u8, u8)> = run_sv(&ref_path, "ref_fast_counter", "wr_clk", &["count_o", "flag_o"], "")
        .into_iter()
        .map(|r| (r[0] as u8, r[1] as u8))
        .collect();

    let sv = copper_codegen::transpile_source(
        FAST_COUNTER_SRC,
        Some("fast_counter"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("transpile");
    let p = dir.join("fast_counter.sv");
    std::fs::write(&p, &sv).unwrap();
    let transpiled: Vec<(u8, u8)> = run_sv(&p, "fast_counter", "clk", &["count_out", "flag_out"], "")
        .into_iter()
        .map(|r| (r[0] as u8, r[1] as u8))
        .collect();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        transpiled, independent,
        "codegen must keep matching the independent reference"
    );
    assert_ne!(
        sim, independent,
        "the simulator now MATCHES independent hardware — the gap is FIXED; \
         delete this test and re-bless tests/two_domain_hierarchy_cdc.rs"
    );

    // Concretely: the flag asserts a cycle early in the simulator.
    let sim_flag = sim.iter().position(|&(_, f)| f == 1).unwrap();
    let ref_flag = independent.iter().position(|&(_, f)| f == 1).unwrap();
    assert_eq!(sim_flag + 1, ref_flag, "sim = {sim:?}\nref = {independent:?}");
}

// ── Divergence 2, and the compensation ────────────────────────────────────────

#[hardware(sequential)]
async fn slow_consumer(clk: Clock<ClkSlow>, flag_in: In<Logic, ClkSlow>, out: Out<Logic, ClkSlow>) {
    loop {
        out.write(flag_in.read());
        clk.tick().await;
    }
}

/// The corrected counter: sticky update moved after the tick, so it reads the
/// pre-edge `count` exactly as `if (cnt[3]) latch <= 1;` does.
#[hardware(sequential)]
async fn fast_counter_corrected(
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

/// The `two_domain_counter` chain at 2:1, returning the cycle each stage asserts:
/// `(flag_raw, sync_q, consumer)`.
fn chain_assert_cycles(corrected: bool) -> (usize, usize, usize) {
    let mut cf = Clock::<ClkFast>::new();
    let mut cs = Clock::<ClkSlow>::new();
    let mut exec = HardwareExecutor::new();

    let (cp, _co) = wire::<Bits<8>, ClkFast>(Bits::zero());
    let (fp, fi) = wire::<Logic, ClkFast>(Logic::Zero);
    let (qp, qi) = wire::<Logic, ClkSlow>(Logic::Zero);
    let (op, oo) = wire::<Logic, ClkSlow>(Logic::Zero);
    let (flag_obs, syncq_obs) = (fi.clone(), qi.clone());

    let (dc, df, dq, dop) =
        (cp.dirty_handle(), fp.dirty_handle(), qp.dirty_handle(), op.dirty_handle());
    let sync_reads = vec![fi.wire_id()];
    let cons_reads = vec![qi.wire_id()];

    if corrected {
        exec.spawn_wired(fast_counter_corrected(cf.clone(), cp, fp), vec![dc, df], vec![]);
    } else {
        exec.spawn_wired(fast_counter(cf.clone(), cp, fp), vec![dc, df], vec![]);
    }
    exec.spawn_wired(sync_2ff(cs.clone(), fi, qp), vec![dq], sync_reads);
    exec.spawn_wired(slow_consumer(cs.clone(), qi, op), vec![dop], cons_reads);

    let (mut f, mut q, mut o) = (None, None, None);
    for i in 0..12 {
        exec.tick_clock(&mut cf);
        exec.tick_clock(&mut cf);
        exec.tick_clock(&mut cs);
        if f.is_none() && flag_obs.read() == Logic::One {
            f = Some(i);
        }
        if q.is_none() && syncq_obs.read() == Logic::One {
            q = Some(i);
        }
        if o.is_none() && oo.read() == Logic::One {
            o = Some(i);
        }
    }
    (f.unwrap(), q.unwrap(), o.unwrap())
}

#[test]
fn d2_is_fixed_and_d1_still_demonstrates_the_hazard() {
    // HISTORY. `tests/two_domain_hierarchy_cdc.rs` used to pass for the WRONG reason:
    // the counter asserted a cycle early (D1) and the passthrough consumer a cycle
    // late (D2), and the two cancelled at the boundary. This test pinned that
    // cancellation. Both are now resolved — D1 is a compile error (this file's
    // `fast_counter` opts out precisely so it can still demonstrate it), and D2 is
    // fixed in `classify_reads`. What is asserted here is the post-fix state.

    // D2 IS FIXED: a combinational passthrough now TRACKS its producer instead of
    // lagging it. Adjudicated against independent hand-written Verilog — a clocked
    // producer feeding a passthrough gives `mid == out` in hardware.
    let (_f, q, o) = chain_assert_cycles(false);
    assert_eq!(
        o, q,
        "the combinational passthrough must track the synchronizer output, not lag it \
         — if this regresses, D2 is back"
    );

    // D1 IS STILL DEMONSTRABLE: `fast_counter` here carries
    // `allow_pretick_alignment`, so the divergent shape is still expressible and its
    // effect is still visible — the flag asserts a cycle before the corrected form.
    let (fc, qc, oc) = chain_assert_cycles(true);
    let (f, _, _) = chain_assert_cycles(false);
    assert_eq!(f + 1, fc, "the opted-out fast_counter should still assert a cycle early");
    assert_eq!(oc, qc, "the corrected chain must also have a tracking passthrough");
}

// ── D1 in a MIDDLE segment: a plain `Out` driven in two phases ────────────────
//
// The guardrail plan recorded the middle-segment gap as theoretical (Q5: "no
// instance in the corpus. If one turns up it should be measured before the rule is
// widened"). One turned up — a one-cycle output pulse, found while writing the
// first equivalence test for the UART receiver — and this is the measurement.
//
// The shape is a plain `Out` written on BOTH sides of a `clk.tick().await`. It is
// already refused on the linear multi-tick path ("output port `dv` is driven in
// more than one phase … hold it in a register"), but control extraction rewrites a
// body whose ticks live inside branches or loops into a SINGLE-tick `match pc` FSM,
// so that check counts one tick and passes while the `pc` states are the phases it
// was meant to count.
//
// Widening the D1 rule instead was measured and rejected: extending it to every
// post-tick segment flags 36 of 120 corpus modules, ~30 of them with passing
// equivalence tests (`det_010`, `mac_pipeline`, `dual_port_ram`, `bsg_dff_en`,
// every memory fixture). Writing a plain `Out` after a tick is the ordinary
// multi-phase pattern and is correct; writing it in *two* phases is not.

const PULSE_PLAIN_SRC: &str = r#"
#[hardware(sequential, allow_pretick_alignment)]
async fn pulse_plain(clk: Clock<C>, dv: Out<Logic, C>) {
    loop {
        for _ in 0..3 { clk.tick().await; }
        dv.write(Logic::One);
        clk.tick().await;
        dv.write(Logic::Zero);
    }
}
"#;
#[hardware(sequential, allow_pretick_alignment)]
async fn pulse_plain(clk: Clock<C>, dv: Out<Logic, C>) {
    loop {
        for _ in 0..3 { clk.tick().await; }
        dv.write(Logic::One);
        clk.tick().await;
        dv.write(Logic::Zero);
    }
}

const PULSE_REG_SRC: &str = r#"
#[hardware(sequential)]
async fn pulse_registered(clk: Clock<C>, dv: RegOut<Logic, C>) {
    loop {
        for _ in 0..3 { clk.tick().await; }
        dv.write(Logic::One);
        clk.tick().await;
        dv.write(Logic::Zero);
    }
}
"#;
#[hardware(sequential)]
async fn pulse_registered(clk: Clock<C>, dv: RegOut<Logic, C>) {
    loop {
        for _ in 0..3 { clk.tick().await; }
        dv.write(Logic::One);
        clk.tick().await;
        dv.write(Logic::Zero);
    }
}

fn sim_pulse_plain() -> Vec<u8> {
    let mut clk = Clock::<C>::new();
    let mut exec = HardwareExecutor::new();
    let (o, obs) = wire::<Logic, C>(Logic::Zero);
    let dh = o.dirty_handle();
    exec.spawn_wired(pulse_plain(clk.clone(), o), vec![dh], vec![]);
    (0..CYCLES)
        .map(|_| {
            exec.tick_clock(&mut clk);
            u8::from(obs.read() == Logic::One)
        })
        .collect()
}

fn sim_pulse_registered() -> Vec<u8> {
    let mut clk = Clock::<C>::new();
    let mut exec = HardwareExecutor::new();
    let (o, obs) = copper_core::port::registered_wire::<Logic, C>(&clk, Logic::Zero);
    let dh = o.dirty_handle();
    exec.spawn_wired(pulse_registered(clk.clone(), o), vec![dh], vec![]);
    (0..CYCLES)
        .map(|_| {
            exec.tick_clock(&mut clk);
            u8::from(obs.read() == Logic::One)
        })
        .collect()
}

/// The measurement the rule rests on: a plain `Out` pulse driven in two phases
/// diverges from its own transpiled SystemVerilog by exactly one cycle.
#[test]
fn a_plain_out_driven_in_two_phases_diverges() {
    let sim = sim_pulse_plain();
    // Period 4 (three delay ticks plus the pulse tick); the simulator observes the
    // write in the post-edge settle of the cycle it was issued in.
    assert_eq!(
        sim,
        vec![0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0],
        "simulator behaviour changed"
    );

    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(PULSE_PLAIN_SRC, "pulse_plain", "clk", "dv", "");
    assert_ne!(
        sim, sv,
        "sim and SV now AGREE for a plain `Out` driven in two phases — the hazard is \
         FIXED. Remove `multi_phase_out_write`'s error, promote this to a real \
         equivalence test, and revert the `RegOut` migrations in examples/uart/system.rs \
         and examples/cpu/rv32i_cpu.rs."
    );
    // The same period, one cycle later — not a different design, a shifted one.
    assert_eq!(sv, vec![0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0], "transpiled SV behaviour changed");
}

/// The discriminator, isolated: identical design, `RegOut` instead of `Out`, and
/// the divergence disappears. This is what makes the diagnostic's advice correct.
#[test]
fn the_registered_output_form_agrees() {
    let sim = sim_pulse_registered();
    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(PULSE_REG_SRC, "pulse_registered", "clk", "dv", "");
    assert_eq!(
        sim, sv,
        "the `RegOut` form must stay sim ≡ SV — it is the remedy the guard points at"
    );
}

/// …and the rule itself flags exactly the plain form.
#[test]
fn the_rule_flags_the_plain_form_and_not_the_registered_one() {
    let plain: syn::ItemFn = syn::parse_str(PULSE_PLAIN_SRC).expect("parses");
    let reg: syn::ItemFn = syn::parse_str(PULSE_REG_SRC).expect("parses");
    assert_eq!(copper_analysis::multi_phase_out_write(&plain), vec!["dv".to_string()]);
    assert!(
        copper_analysis::multi_phase_out_write(&reg).is_empty(),
        "`RegOut` must be exempt — it is excluded by construction, like multi_write_collapse"
    );
}

// ── D1 in the TRAILING segment — a measured gap, NOT yet guarded ──────────────
//
// `unprotected_pretick_out_write` examines head → first tick. The statements
// AFTER the loop's last tick run in the same cycle as the head segment (falling
// off the end and re-entering costs no clock), so they are exposed to the same
// phase question — and D1's canonical shape, moved there, diverges:
//
//     loop { for _ in 0..2 { tick } n = n + 1; o.write(n); }
//
// The rule does not flag it. Two widenings were measured and REJECTED:
//
//   * merging the trailing segment into the head region flags **25** further
//     corpus modules, all passing — including `sync_2ff`, `dual_port_ram`, and
//     `fast_counter_corrected`, which is the module D1's OWN REMEDY produces
//     ("move the register update after the `clk.tick().await`"). The hazard needs
//     the output write and the register assignment in the SAME segment; split
//     across the two, it is the fix, not the bug.
//   * applying the two clauses to the trailing segment as a separate region cuts
//     that to **10**, all memory modules — e.g. `rom_from_fn`, whose trailing
//     segment is `if ready { q = data() } data.write(q)`: structurally identical
//     to the DUT below, and it AGREES.
//
// So the discriminator between this DUT and `rom_from_fn` is unidentified, and a
// leading `In` read is not it — the same DUT plus one was measured and still
// diverges. Until a condition has a flipping witness (requirement R3 of
// design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md), widening the rule would reject
// correct designs, which is requirement R1.

const TRAILING_D1_SRC: &str = r#"
#[hardware(sequential)]
async fn trailing_update(clk: Clock<C>, o: Out<Bits<8>, C>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        for _ in 0..2 { clk.tick().await; }
        n = n + Bits::from_lit::<1>();
        o.write(n);
    }
}
"#;
#[hardware(sequential)]
async fn trailing_update(clk: Clock<C>, o: Out<Bits<8>, C>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        for _ in 0..2 { clk.tick().await; }
        n = n + Bits::from_lit::<1>();
        o.write(n);
    }
}

#[test]
fn d1_in_the_trailing_segment_is_an_unguarded_gap() {
    let mut clk = Clock::<C>::new();
    let mut exec = HardwareExecutor::new();
    let (o, obs) = wire::<Bits<8>, C>(Bits::zero());
    let dh = o.dirty_handle();
    exec.spawn_wired(trailing_update(clk.clone(), o), vec![dh], vec![]);
    let sim: Vec<u8> = (0..CYCLES)
        .map(|_| {
            exec.tick_clock(&mut clk);
            obs.read().as_u128() as u8
        })
        .collect();

    // The rule does NOT flag it — that is the gap, asserted so it flips loudly if
    // the rule is ever widened to cover it.
    let f: syn::ItemFn = syn::parse_str(TRAILING_D1_SRC).expect("parses");
    assert!(
        copper_analysis::unprotected_pretick_out_write(&f).is_empty(),
        "the trailing-segment gap is now GUARDED — good. Delete this test, add the \
         DUT to EXPECTED_FLAGGED in pretick_alignment_corpus.rs, and record the \
         discriminator that made it separable from `rom_from_fn`."
    );

    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(TRAILING_D1_SRC, "trailing_update", "clk", "o", "");
    assert_ne!(
        sim, sv,
        "sim and SV now AGREE for a trailing-segment register update — the gap is \
         FIXED. Promote this to a real equivalence test."
    );
}

// ── Divergence 3 — a write in a state-machine arm, and D1's constant exemption ──
//
// FOUND 2026-08-25 by tests/corpus_equivalence.rs, on random stimulus, in
// `branch_merge_explicit` — a fixture that had been in the tree for weeks with only
// a STRUCTURAL check on it. The sharpest form of the finding: that module and its
// async twin `branch_merge` transpile to BYTE-IDENTICAL SystemVerilog, the twin
// agrees with that SV for 200 random cycles, and the explicit one leads it by a
// cycle. The simulator therefore disagrees with itself depending on how the same
// hardware is spelled.
//
// The DUTs below are that shape, minimised. `o` is written only in the `pc == 1`
// arm, which belongs to the NEXT cycle — but the simulator runs the next
// iteration's pre-tick segment during the post-edge settle of this tick, so the
// write is observable a cycle before the hardware performs it.
//
// WHY THE D1 GUARD DOES NOT FIRE: `unprotected_pretick_out_write` exempts a write
// of a CONSTANT, on the grounds that a constant is idempotent across the phase
// shift. That premise holds only if the write happens every cycle. Here which arm
// runs is chosen by a register (`pc`), so the constant is not the whole story — in
// `pc_arm_write` the other path leaves the port HOLDING, and in `pc_arm_toggle` it
// writes a DIFFERENT constant. Either way, *when* the write lands is observable.
//
// Narrowing the exemption is a rule widening whose corpus cost has to be measured
// first (see §5.4 for two widenings that were measured and rejected), so these pin
// today's behaviour and flip loudly when it changes.

const PC_ARM_SRC: &str = r#"
#[hardware(sequential)]
async fn pc_arm_write(clk: Clock<C>, sel: In<Logic, C>, o: Out<Logic, C>) {
    let mut pc: u8 = 0;
    loop {
        match pc {
            0u8 => { if sel.read() == Logic::One { pc = 1; } }
            1u8 => { o.write(Logic::One); pc = 0; }
            _ => {}
        }
        clk.tick().await;
    }
}
"#;

#[hardware(sequential)]
async fn pc_arm_write(clk: Clock<C>, sel: In<Logic, C>, o: Out<Logic, C>) {
    let mut pc: u8 = 0;
    loop {
        match pc {
            0u8 => { if sel.read() == Logic::One { pc = 1; } }
            1u8 => { o.write(Logic::One); pc = 0; }
            _ => {}
        }
        clk.tick().await;
    }
}

/// The same machine with the other arm driving the port low, so the lead is visible
/// on every cycle instead of only the first — `pc_arm_write`'s output latches high
/// and can only show it once.
const PC_ARM_TOGGLE_SRC: &str = r#"
#[hardware(sequential)]
async fn pc_arm_toggle(clk: Clock<C>, sel: In<Logic, C>, o: Out<Logic, C>) {
    let mut pc: u8 = 0;
    loop {
        match pc {
            0u8 => { o.write(Logic::Zero); if sel.read() == Logic::One { pc = 1; } }
            1u8 => { o.write(Logic::One); pc = 0; }
            _ => {}
        }
        clk.tick().await;
    }
}
"#;

#[hardware(sequential)]
async fn pc_arm_toggle(clk: Clock<C>, sel: In<Logic, C>, o: Out<Logic, C>) {
    let mut pc: u8 = 0;
    loop {
        match pc {
            0u8 => { o.write(Logic::Zero); if sel.read() == Logic::One { pc = 1; } }
            1u8 => { o.write(Logic::One); pc = 0; }
            _ => {}
        }
        clk.tick().await;
    }
}

fn sim_pc_arm(toggle: bool) -> Vec<u8> {
    let mut clk = Clock::<C>::new();
    let mut exec = HardwareExecutor::new();
    let (sel_drv, sel_in) = wire::<Logic, C>(Logic::One);
    let (o, obs) = wire::<Logic, C>(Logic::Zero);
    let dh = o.dirty_handle();
    let reads = vec![sel_in.wire_id()];
    if toggle {
        exec.spawn_wired(pc_arm_toggle(clk.clone(), sel_in, o), vec![dh], reads);
    } else {
        exec.spawn_wired(pc_arm_write(clk.clone(), sel_in, o), vec![dh], reads);
    }
    sel_drv.write(Logic::One);
    (0..CYCLES)
        .map(|_| {
            exec.tick_clock(&mut clk);
            u8::from(obs.read() == Logic::One)
        })
        .collect()
}

/// The corpus instance's shape: `o` is written only in the `pc == 1` arm, so the
/// other path leaves it holding. The simulator shows the write a cycle before the
/// hardware performs it. It shows up once because the output latches high — which
/// is exactly why a hand-written vector set never caught it, and why 200 cycles of
/// random stimulus did.
#[test]
fn a_write_in_a_state_arm_leads_the_hardware_by_one_cycle() {
    let sim = sim_pc_arm(false);
    assert_eq!(
        sim,
        vec![1u8; CYCLES],
        "simulator behaviour changed; if it now starts at 0 the divergence is FIXED"
    );

    // The D1 guard exempts this: the value written is a constant. That exemption is
    // what this witness says is unsound for a conditionally-written `Out`.
    let f: syn::ItemFn = syn::parse_str(PC_ARM_SRC).expect("parses");
    assert!(
        copper_analysis::unprotected_pretick_out_write(&f).is_empty(),
        "the constant-write exemption is now NARROWER — good. Delete this assertion, \
         add the DUT to EXPECTED_FLAGGED in pretick_alignment_corpus.rs, and record \
         the corpus cost of the narrowing in PRETICK_ALIGNMENT_GUARDRAIL.md §5.5."
    );

    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(PC_ARM_SRC, "pc_arm_write", "clk", "o", "t->sel = 1;");
    let mut expected_sv = vec![1u8; CYCLES];
    expected_sv[0] = 0;
    assert_eq!(sv, expected_sv, "transpiled SV behaviour changed");
    assert_ne!(
        sim, sv,
        "sim and SV now AGREE — this divergence is FIXED. Promote it to a real \
         equivalence test and un-ignore \
         corpus_equivalence.rs::branch_merge_explicit_differential_case, which is \
         the same shape in the corpus."
    );
}

/// The same machine with the other arm driving the port low. The two traces are
/// each other shifted by exactly one cycle, for all 13 — so this is a phase shift,
/// not an initialisation artifact, and it is worth the second DUT to say so.
#[test]
fn the_state_arm_lead_is_systematic_not_a_first_cycle_artifact() {
    let sim = sim_pc_arm(true);
    let expected_sim: Vec<u8> = (0..CYCLES).map(|i| u8::from(i % 2 == 0)).collect();
    assert_eq!(sim, expected_sim, "simulator behaviour changed");

    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(PC_ARM_TOGGLE_SRC, "pc_arm_toggle", "clk", "o", "t->sel = 1;");
    let expected_sv: Vec<u8> = (0..CYCLES).map(|i| u8::from(i % 2 == 1)).collect();
    assert_eq!(sv, expected_sv, "transpiled SV behaviour changed");
    assert_eq!(
        sim[1..],
        sv[..CYCLES - 1],
        "the divergence is no longer a clean one-cycle shift — it changed shape, \
         which means something other than the phase alignment moved"
    );
}
