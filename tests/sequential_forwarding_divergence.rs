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
#[hardware(sequential)]
async fn add_then_write(clk: Clock<C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        r = r + Bits::from_lit::<1>();
        o.write(r);
        clk.tick().await;
    }
}
"#;

#[hardware(sequential)]
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
    // Unique PER INVOCATION, not per top module: two tests in this binary
    // Verilate the same top in parallel (`trailing_update` is built by both the
    // d1-trailing gap test and the m2 model-lowering test), and a shared
    // directory lets them clobber each other's build — the false-PASS/false-FAIL
    // mechanism the repo's obj_dir convention exists to prevent (see CLAUDE.md).
    static NONCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let work =
        std::env::temp_dir().join(format!("copper_fwd_{top}_{}_{n}", std::process::id()));
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
fn pre_tick_update_forwarding_agrees_end_to_end() {
    // RE-BLESSED 2026-08-26, cycle-dataflow phase B (PAIRED_IMPLEMENTATION_SCOPE.md):
    // divergence 1 is DISSOLVED. Codegen now emits the FORWARDED continuous assign
    // (`assign o = (r + 8'd1)`) for an opening-prefix drive — the meaning the
    // simulator always had — so the shape is a real equivalence check now. The
    // trace pins stay exact so a regression on either side is loud.
    let sim = sim_add_then_write();
    let expected: Vec<u8> = (2..).take(CYCLES).collect();
    assert_eq!(sim, expected, "simulator behaviour changed");

    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(ADD_SRC, "add_then_write", "clk", "o", "");
    assert_eq!(
        sim, sv,
        "sim and SV must AGREE on the forwarded reading — phase B's emission \
         (opening-prefix drives use edge_value) has regressed"
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
#[hardware(sequential)]
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
fn independent_hardware_anchors_the_corrected_spelling() {
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

    // RE-POINTED 2026-08-26, cycle-dataflow phase B, with a recorded rationale
    // (PAIRED_IMPLEMENTATION_SCOPE.md phase B; R7: anchors are re-verified, never
    // silently re-blessed). What the adjudication established is that the English
    // description — "a counter with a sticky flag" — maps to the REGISTERED form,
    // which under the model is the write-then-update SPELLING
    // (`fast_counter_corrected`). The reference therefore anchors THAT spelling.
    // The witness spelling (update-then-write) legitimately means the forwarded
    // trace, and as of phase B both implementations agree on it — one cycle ahead
    // of the reference, which is the two spellings being two different programs,
    // not a divergence.
    let corrected_sv = copper_codegen::transpile_source(
        FAST_COUNTER_CORRECTED_SRC,
        Some("fast_counter_corrected"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("transpile corrected");
    let dir2 = std::env::temp_dir().join(format!("copper_fwd_corr_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir2);
    std::fs::create_dir_all(&dir2).unwrap();
    let pc = dir2.join("fast_counter_corrected.sv");
    std::fs::write(&pc, &corrected_sv).unwrap();
    let corrected: Vec<(u8, u8)> =
        run_sv(&pc, "fast_counter_corrected", "clk", &["count_out", "flag_out"], "")
            .into_iter()
            .map(|r| (r[0] as u8, r[1] as u8))
            .collect();
    let _ = std::fs::remove_dir_all(&dir2);
    assert_eq!(
        corrected, independent,
        "the independent reference anchors the CORRECTED spelling — codegen for it \
         has drifted from the outside implementation"
    );

    // The witness spelling now agrees END TO END on the forwarded reading…
    assert_eq!(
        sim, transpiled,
        "the witness spelling must agree sim ≡ SV under forwarded emission (phase B)"
    );
    // …and leads the reference by exactly one cycle: two spellings, two programs.
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

const FAST_COUNTER_CORRECTED_SRC: &str = r#"
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
"#;

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
#[hardware(sequential, allow_pretick_alignment)]
async fn trailing_update(clk: Clock<C>, o: Out<Bits<8>, C>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        for _ in 0..2 { clk.tick().await; }
        n = n + Bits::from_lit::<1>();
        o.write(n);
    }
}
"#;
#[hardware(sequential, allow_pretick_alignment)]
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

    // GUARDED since 2026-08-25 by `unprotected_trailing_out_write`. The head-segment
    // rule still does not see it — it examines head → first tick — which is why the
    // trailing region needed a rule of its own rather than a wider one.
    //
    // The discriminator that made it separable from `rom_from_fn` (§5.4's open
    // question) is the number of clock edges the body crosses per iteration, found by
    // flipping exactly that: with the identical trailing body, a SINGLE-tick loop
    // agrees and a multi-tick one diverges. In a single-tick loop the trailing
    // statements share the head's phase, so there is nothing to be misaligned against.
    let f: syn::ItemFn = syn::parse_str(TRAILING_D1_SRC).expect("parses");
    assert!(
        copper_analysis::unprotected_pretick_out_write(&f).is_empty(),
        "the HEAD rule now covers the trailing segment too — check that is deliberate; \
         the two were kept separate because widening the head region cost 25 false \
         positives (§5.4)"
    );
    assert_eq!(
        copper_analysis::unprotected_trailing_out_write(&f),
        vec!["o".to_string()],
        "the trailing rule stopped flagging the shape it exists for — the traces below \
         still diverge, so accepting it again would ship the divergence"
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
#[hardware(sequential, allow_pretick_alignment)]
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

#[hardware(sequential, allow_pretick_alignment)]
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
#[hardware(sequential, allow_pretick_alignment)]
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

#[hardware(sequential, allow_pretick_alignment)]
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

    // The D1 guard now COVERS this shape (2026-08-25): the constant-write exemption
    // was narrowed to UNCONDITIONAL writes, because a constant is only idempotent
    // across the phase shift if it is written on every path — where the alternative
    // is the port's held value, when the write lands is observable. The divergence
    // below is unchanged; what changed is that the language rejects the shape rather
    // than emitting it. The DUT keeps `allow_pretick_alignment` because it exists to
    // demonstrate the hazard, and the flag silences the error, not the detection.
    let f: syn::ItemFn = syn::parse_str(PC_ARM_SRC).expect("parses");
    assert_eq!(
        copper_analysis::unprotected_pretick_out_write(&f),
        vec!["o".to_string()],
        "the rule stopped flagging the shape it was narrowed to catch — this DUT is \
         in EXPECTED_FLAGGED in pretick_alignment_corpus.rs, and the traces below \
         still diverge, so silently accepting it again would ship the divergence"
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

// ── m1 / V8 — a write between a leading read and the register update ──────────
//
// `design_docs/DERIVATION_TABLE.md` F2, measured (m1 in its §5). The
// cycle-dataflow model derives a divergent shape D1 does not guard: in a
// closing-anchored segment the barrier parks the task AT THE READ SITE, so a
// plain-`Out` write placed after the read runs in the pre-edge settle — and if it
// also precedes the update of the register it reads, it writes the PREVIOUS
// generation's value, which the emitted `assign o = r` (Q) never shows at any
// observation instant. D1 exempts the segment precisely because the read
// comb-reaches the update, so nothing flags it.
//
// This is the CPU sweep's `program_counter` divergence #1 (TODO cause Q) reduced
// to its minimal pair. Position of the write is the only variable across the
// three DUTs:
//
//   V8a  read; write; update   → predicted DIVERGE (SV leads by one)
//   V8b  read; update; write   → predicted agree (lfsr's shape: forwarded value
//                                 ≡ committing value ≡ Q at the next observation)
//   V8c  write; read+update    → predicted agree (the write executes BEFORE the
//                                 barrier point, at the opening, reading
//                                 committed state)

const V8A_SRC: &str = r#"
#[hardware(sequential, allow_pretick_alignment)]
async fn v8a_read_write_update(clk: Clock<C>, step: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        let s = step.read();
        o.write(r);
        r = r + s;
        clk.tick().await;
    }
}
"#;

#[hardware(sequential, allow_pretick_alignment)]
async fn v8a_read_write_update(clk: Clock<C>, step: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        let s = step.read();
        o.write(r);
        r = r + s;
        clk.tick().await;
    }
}

const V8B_SRC: &str = r#"
#[hardware(sequential)]
async fn v8b_read_update_write(clk: Clock<C>, step: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        let s = step.read();
        r = r + s;
        o.write(r);
        clk.tick().await;
    }
}
"#;

#[hardware(sequential)]
async fn v8b_read_update_write(clk: Clock<C>, step: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        let s = step.read();
        r = r + s;
        o.write(r);
        clk.tick().await;
    }
}

const V8C_SRC: &str = r#"
#[hardware(sequential)]
async fn v8c_write_read_update(clk: Clock<C>, step: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        o.write(r);
        r = r + step.read();
        clk.tick().await;
    }
}
"#;

#[hardware(sequential)]
async fn v8c_write_read_update(clk: Clock<C>, step: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        o.write(r);
        r = r + step.read();
        clk.tick().await;
    }
}

fn sim_v8(which: u8) -> Vec<u8> {
    let mut clk = Clock::<C>::new();
    let mut exec = HardwareExecutor::new();
    let (step_drv, step_in) = wire::<Bits<8>, C>(Bits::zero());
    let (p, obs) = wire::<Bits<8>, C>(Bits::zero());
    let dh = p.dirty_handle();
    let reads = vec![step_in.wire_id()];
    match which {
        0 => exec.spawn_wired(v8a_read_write_update(clk.clone(), step_in, p), vec![dh], reads),
        1 => exec.spawn_wired(v8b_read_update_write(clk.clone(), step_in, p), vec![dh], reads),
        _ => exec.spawn_wired(v8c_write_read_update(clk.clone(), step_in, p), vec![dh], reads),
    };
    step_drv.write(Bits::from_lit::<1>());
    (0..CYCLES)
        .map(|_| {
            exec.tick_clock(&mut clk);
            obs.read().as_u128() as u8
        })
        .collect()
}

#[test]
fn v8a_write_between_leading_read_and_update_diverges_known_gap() {
    let sim = sim_v8(0);
    let expected_sim: Vec<u8> = (0..).take(CYCLES).collect();
    assert_eq!(
        sim, expected_sim,
        "simulator behaviour changed; if it now starts at 1 the gap is FIXED"
    );

    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(V8A_SRC, "v8a_read_write_update", "clk", "o", "t->step = 1;");
    let expected_sv: Vec<u8> = (1..).take(CYCLES).collect();
    assert_eq!(sv, expected_sv, "transpiled SV behaviour changed");
    assert_ne!(
        sim, sv,
        "sim and SV now AGREE — F2's derived shape is FIXED. Update \
         DERIVATION_TABLE.md F2 and the write-position rule it proposes."
    );
}

#[test]
fn v8b_moving_the_write_after_the_update_removes_the_divergence() {
    let sim = sim_v8(1);
    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(V8B_SRC, "v8b_read_update_write", "clk", "o", "t->step = 1;");
    assert_eq!(
        sim, sv,
        "the write-after-update form must stay sim ≡ SV — this is lfsr's shape \
         (legal shape 2 in DERIVATION_TABLE.md §1)"
    );
    assert_eq!(sim, (1..).take(CYCLES).collect::<Vec<u8>>());
}

#[test]
fn v8c_moving_the_write_before_the_read_removes_the_divergence() {
    let sim = sim_v8(2);
    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(V8C_SRC, "v8c_write_read_update", "clk", "o", "t->step = 1;");
    assert_eq!(
        sim, sv,
        "the write-before-the-read form must stay sim ≡ SV — the write executes \
         before the barrier point, at the opening, reading committed state"
    );
    assert_eq!(sim, (1..).take(CYCLES).collect::<Vec<u8>>());
}

// V8d — V8a with the update routed through a same-cycle temp (`let next = …`).
// This is the shape of the `single_loop_local_ok.rs` UI fixture, which the new
// rule flagged on landing: the fixture existed to exercise the single-loop and
// param-shadowing guardrails and was COMPILE-ONLY — nothing had ever measured its
// behaviour. The rename changes nothing: the write still captures pre-update
// `count` at the pre-edge while `assign out = count` shows the committed value.

const V8D_SRC: &str = r#"
#[hardware(sequential, allow_pretick_alignment)]
async fn v8d_temp_renamed_update(clk: Clock<C>, step: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut count: Bits<8> = Bits::zero();
    loop {
        let next = count + step.read();
        o.write(count);
        count = next;
        clk.tick().await;
    }
}
"#;

#[hardware(sequential, allow_pretick_alignment)]
async fn v8d_temp_renamed_update(clk: Clock<C>, step: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut count: Bits<8> = Bits::zero();
    loop {
        let next = count + step.read();
        o.write(count);
        count = next;
        clk.tick().await;
    }
}

#[test]
fn v8d_temp_renamed_update_diverges_like_v8a_known_gap() {
    let mut clk = Clock::<C>::new();
    let mut exec = HardwareExecutor::new();
    let (step_drv, step_in) = wire::<Bits<8>, C>(Bits::zero());
    let (p, obs) = wire::<Bits<8>, C>(Bits::zero());
    let dh = p.dirty_handle();
    let reads = vec![step_in.wire_id()];
    exec.spawn_wired(v8d_temp_renamed_update(clk.clone(), step_in, p), vec![dh], reads);
    step_drv.write(Bits::from_lit::<1>());
    let sim: Vec<u8> = (0..CYCLES)
        .map(|_| {
            exec.tick_clock(&mut clk);
            obs.read().as_u128() as u8
        })
        .collect();
    let expected_sim: Vec<u8> = (0..).take(CYCLES).collect();
    assert_eq!(sim, expected_sim, "simulator behaviour changed");

    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(V8D_SRC, "v8d_temp_renamed_update", "clk", "o", "t->step = 1;");
    let expected_sv: Vec<u8> = (1..).take(CYCLES).collect();
    assert_eq!(sv, expected_sv, "transpiled SV behaviour changed");
    assert_ne!(sim, sv, "sim and SV now AGREE — see the V8a test's note");
}

// ── m2 — the model's lowering for the trailing segment, hand-written ──────────
//
// `design_docs/DERIVATION_TABLE.md` §5 m2, the §5.3 technique (hand-write the
// candidate lowering, run it under Verilator, compare against the simulator).
//
// The cycle-dataflow model places `trailing_update`'s divergence in the
// **dissolved** bucket: the trailing statements belong to the cycle the LAST tick
// opens, their register update commits AT that edge (the trailing-statements rule
// in SYNCHRONOUS_SEMANTICS.md), and the port write publishes the forwarded value —
// which equals the register committed at that same edge. Hand-lowered, that is an
// FSM whose trailing increment lands at the edge closing the final wait state,
// with `assign o = n` reading the committed register:
//
//   always_ff @(posedge clk) begin
//     state <= ~state;
//     if (state == 1) n <= n + 1;   // the edge that OPENS the trailing cycle
//   end
//   assign o = n;
//
// Prediction: this matches the simulator's existing trace cycle-for-cycle —
// i.e. the sim was already implementing the model, and today's codegen (which
// diverges by one, `d1_in_the_trailing_segment_is_an_unguarded_gap`) is the side
// the paired fix changes.

const TRAILING_MODEL_SV: &str = r#"
module trailing_update_model(input logic clk, output logic [7:0] o);
  logic [7:0] n = 8'd0;
  logic state = 1'b0;
  always_ff @(posedge clk) begin
    state <= ~state;
    if (state == 1'b1) n <= n + 8'd1;
  end
  assign o = n;
endmodule
"#;

#[test]
fn m2_model_forwarded_lowering_matches_the_simulator_for_trailing_update() {
    // The simulator's trace, now pinned exactly: the write lands at the
    // observation of every even edge, so obs k shows floor(k/2).
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
    let expected: Vec<u8> = (1..=CYCLES).map(|k| (k / 2) as u8).collect();
    assert_eq!(sim, expected, "simulator behaviour changed");

    if !verilator_available() {
        return;
    }

    let dir = std::env::temp_dir()
        .join(format!("copper_fwd_m2_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("trailing_update_model.sv");
    std::fs::write(&p, TRAILING_MODEL_SV).unwrap();
    let model: Vec<u8> = run_sv(&p, "trailing_update_model", "clk", &["o"], "")
        .into_iter()
        .map(|r| r[0] as u8)
        .collect();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        sim, model,
        "the hand-lowered MODEL emission no longer matches the simulator — the m2 \
         derivation (DERIVATION_TABLE.md, trailing_update row) is wrong or the \
         executor's trailing-segment behaviour changed"
    );

    // And today's codegen still disagrees with both — the divergence the paired
    // fix will close from the codegen side. (The sim≠SV half is also asserted by
    // d1_in_the_trailing_segment_is_an_unguarded_gap; repeated here so this test
    // reads as the complete m2 statement: sim ≡ model ≠ today's SV.)
    let sv = transpile_and_run(TRAILING_D1_SRC, "trailing_update", "clk", "o", "");
    assert_ne!(
        model, sv,
        "today's transpiled SV now matches the model lowering — the trailing gap is \
         FIXED in codegen. Promote trailing_update to a real equivalence test and \
         update DERIVATION_TABLE.md's trailing_update row from 'predict' to done."
    );
}

// ── The V-battery re-measured under forwarded emission (phase D evidence) ─────
//
// Narrowing D1 (`unprotected_pretick_out_write`) after phase B requires knowing
// which of its measured-divergent shapes the forwarded emission DISSOLVED. The
// guardrail's §3.1 verdicts predate both the trailing-forwarding fix
// (2026-08-25) and phase B, so they are re-measured here and pinned. V1 is
// covered by `pre_tick_update_forwarding_agrees_end_to_end`; these are the
// discriminating variants for the narrowing's clauses.

const V5_SRC: &str = r#"
#[hardware(sequential)]
async fn v5_trailing_read(clk: Clock<C>, i: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        r = r + Bits::from_lit::<1>();
        o.write(r);
        let _late = i.read();
        clk.tick().await;
    }
}
"#;

#[hardware(sequential)]
async fn v5_trailing_read(clk: Clock<C>, i: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        r = r + Bits::from_lit::<1>();
        o.write(r);
        let _late = i.read();
        clk.tick().await;
    }
}

const V7_SRC: &str = r#"
#[hardware(sequential)]
async fn v7_escape_across_tick(clk: Clock<C>, i: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    let mut s: Bits<8> = Bits::zero();
    loop {
        r = r + Bits::from_lit::<1>();
        o.write(s);
        clk.tick().await;
        s = r;
    }
}
"#;

#[hardware(sequential)]
async fn v7_escape_across_tick(clk: Clock<C>, i: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut r: Bits<8> = Bits::zero();
    let mut s: Bits<8> = Bits::zero();
    loop {
        r = r + Bits::from_lit::<1>();
        o.write(s);
        clk.tick().await;
        s = r;
    }
}

const W4_SRC: &str = r#"
#[hardware(sequential, allow_pretick_alignment)]
async fn w4_mixed_alignment(clk: Clock<C>, i: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut phase: u8 = 0;
    let mut r: Bits<8> = Bits::zero();
    loop {
        if phase == 0 { r = i.read(); phase = 1; }
        else { r = r + Bits::from_lit::<1>(); phase = 0; }
        o.write(r);
        clk.tick().await;
    }
}
"#;

#[hardware(sequential, allow_pretick_alignment)]
async fn w4_mixed_alignment(clk: Clock<C>, i: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
    let mut phase: u8 = 0;
    let mut r: Bits<8> = Bits::zero();
    loop {
        if phase == 0 { r = i.read(); phase = 1; }
        else { r = r + Bits::from_lit::<1>(); phase = 0; }
        o.write(r);
        clk.tick().await;
    }
}

fn sim_probe(which: u8) -> Vec<u8> {
    let mut clk = Clock::<C>::new();
    let mut exec = HardwareExecutor::new();
    let (i_drv, i_in) = wire::<Bits<8>, C>(Bits::zero());
    let (p, obs) = wire::<Bits<8>, C>(Bits::zero());
    let dh = p.dirty_handle();
    let reads = vec![i_in.wire_id()];
    match which {
        0 => exec.spawn_wired(v5_trailing_read(clk.clone(), i_in, p), vec![dh], reads),
        1 => exec.spawn_wired(v7_escape_across_tick(clk.clone(), i_in, p), vec![dh], reads),
        _ => exec.spawn_wired(w4_mixed_alignment(clk.clone(), i_in, p), vec![dh], reads),
    };
    i_drv.write(Bits::from_lit::<3>());
    (0..CYCLES)
        .map(|_| {
            exec.tick_clock(&mut clk);
            obs.read().as_u128() as u8
        })
        .collect()
}

/// Measured 2026-08-26 under forwarded emission (phase B), pinning the phase-D
/// narrowing's discriminator: V5 and V7 AGREE (dissolved — V7's 2026-08-21
/// verdict was stale even before phase B: the 2026-08-25 shared
/// trailing-forwarding map already emits `s <= r + 1`), W4 DIVERGES (the
/// read-preceded / path-dependent-boundary class the narrowed rule retains —
/// sim holds `i + 1`, the SV alternates).
#[test]
fn d_narrowing_battery_verdicts() {
    if !verilator_available() {
        return;
    }
    let v5_sim = sim_probe(0);
    let v5_sv = transpile_and_run(V5_SRC, "v5_trailing_read", "clk", "o", "t->i = 3;");
    assert_eq!(v5_sim, v5_sv, "V5 (trailing read) must stay dissolved: the write is an opening-prefix drive");
    assert_eq!(v5_sim, (2..).take(CYCLES).collect::<Vec<u8>>());

    let v7_sim = sim_probe(1);
    let v7_sv = transpile_and_run(V7_SRC, "v7_escape_across_tick", "clk", "o", "t->i = 3;");
    assert_eq!(v7_sim, v7_sv, "V7 (escape across the tick) must stay dissolved: the shared trailing-forwarding map covers `s = r`");
    assert_eq!(v7_sim, (1..).take(CYCLES).collect::<Vec<u8>>());

    let w4_sim = sim_probe(2);
    let w4_sv = transpile_and_run(W4_SRC, "w4_mixed_alignment", "clk", "o", "t->i = 3;");
    assert_eq!(w4_sim, vec![4u8; CYCLES], "simulator behaviour changed for the mixed-alignment witness");
    assert_ne!(
        w4_sim, w4_sv,
        "sim and SV now AGREE for MIXED alignment — the path-dependent-boundary \
         class is fixed; re-bless the narrowed D1 rule, EXPECTED_FLAGGED, and the \
         ui/fail/pretick_alignment case, which all pin this divergence"
    );
}

// ── The linear-trailing probe (phase-C decision evidence, 2026-08-27) ─────────
//
// `unprotected_trailing_out_write` keys on "crosses more than one clock edge per
// iteration", which covers BOTH lowering paths — but §5.4's divergence was only
// ever measured on the EXTRACTED path (the folded counted `for`). The linear
// multi-tick path commits trailing register updates at the last phase's edge
// (the 2026-08-25 shared-map work), so the model predicts the linear spelling
// of the identical shape AGREES — measured so (2026-08-27). The rule therefore
// over-flags the linear class: its refusal there is a lowering limitation, not
// a semantics rule. The EXTRACTED-with-top-level-last-tick class is NOT
// over-flagged — measured DIVERGING (2026-08-27), see `branch_trailing`.

const LINEAR_TRAILING_SRC: &str = r#"
#[hardware(sequential)]
async fn linear_trailing(clk: Clock<C>, o: Out<Bits<8>, C>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        clk.tick().await;
        clk.tick().await;
        n = n + Bits::from_lit::<1>();
        o.write(n);
    }
}
"#;

#[hardware(sequential)]
async fn linear_trailing(clk: Clock<C>, o: Out<Bits<8>, C>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        clk.tick().await;
        clk.tick().await;
        n = n + Bits::from_lit::<1>();
        o.write(n);
    }
}

#[test]
fn linear_trailing_probe() {
    // NARROWED 2026-08-27: the rule now exempts the linear class (its gate is
    // `has_nested_tick`, the source-level mirror of extraction's trigger), so
    // this spelling compiles WITHOUT an opt-out and the rule must return
    // nothing for it — this probe is the exemption's standing witness.
    let f: syn::ItemFn = syn::parse_str(LINEAR_TRAILING_SRC).expect("parses");
    assert!(
        copper_analysis::unprotected_trailing_out_write(&f).is_empty(),
        "the trailing rule flags the LINEAR spelling again — it was measured \
         AGREEING (the linear path commits trailing updates at the right edge), \
         so that is a false positive unless the linear lowering itself changed"
    );

    let mut clk = Clock::<C>::new();
    let mut exec = HardwareExecutor::new();
    let (o, obs) = wire::<Bits<8>, C>(Bits::zero());
    let dh = o.dirty_handle();
    exec.spawn_wired(linear_trailing(clk.clone(), o), vec![dh], vec![]);
    let sim: Vec<u8> = (0..CYCLES)
        .map(|_| {
            exec.tick_clock(&mut clk);
            obs.read().as_u128() as u8
        })
        .collect();
    let expected: Vec<u8> = (1..=CYCLES).map(|k| (k / 2) as u8).collect();
    assert_eq!(sim, expected, "simulator behaviour changed");

    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(LINEAR_TRAILING_SRC, "linear_trailing", "clk", "o", "");
    eprintln!("linear_trailing: sim = {sim:?}\n                 sv  = {sv:?}  -> {}", if sim == sv { "AGREE" } else { "DIVERGE" });
    assert_eq!(
        sim, sv,
        "…but the LINEAR lowering handles the shape correctly, so the flag is a \
         false positive on this path — an extraction lowering limitation, not a \
         semantics rule (the phase-C reframing)"
    );
}

// The same trailing body behind BRANCH-NESTED ticks — extraction fires with a
// top-level LAST tick. Measuring this was blocked until 2026-08-27 by an
// emission-legality bug: the cause-N `edge_condition` substitution puts a
// compound expression under the branch condition's `Index`, and the emitter
// rendered `pc <= (((n + 8'd1)[0] == 1'b1) ? …)` — a bit-select on a
// parenthesized expression, which SV forbids. Fixed in `emit.rs` (`select_legal`
// gates the `[..]` syntax; compound bases emit the width-cast form
// `1'((n + 8'd1))`), which unblocked the measurement:
//
//   VERDICT (2026-08-27): DIVERGE — the SV trace is the sim trace delayed by
//   one cycle. The extracted route commits the trailing update one edge late
//   even when the LAST tick is top-level, so the placement error is a property
//   of the extraction route as a whole, not just its rotation placement. The
//   trailing rule's flag on this class is a TRUE positive; only the LINEAR
//   class (above) is over-flagged, so the phase-C/D narrowing exempts the
//   linear lowering route alone.

const BRANCH_TRAILING_SRC: &str = r#"
#[hardware(sequential, allow_pretick_alignment)]
async fn branch_trailing(clk: Clock<C>, o: Out<Bits<8>, C>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        if n[0] == Logic::One {
            clk.tick().await;
        } else {
            clk.tick().await;
        }
        clk.tick().await;
        n = n + Bits::from_lit::<1>();
        o.write(n);
    }
}
"#;

#[hardware(sequential, allow_pretick_alignment)]
async fn branch_trailing(clk: Clock<C>, o: Out<Bits<8>, C>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        if n[0] == Logic::One {
            clk.tick().await;
        } else {
            clk.tick().await;
        }
        clk.tick().await;
        n = n + Bits::from_lit::<1>();
        o.write(n);
    }
}

#[test]
fn branch_trailing_probe() {
    let mut clk = Clock::<C>::new();
    let mut exec = HardwareExecutor::new();
    let (o, obs) = wire::<Bits<8>, C>(Bits::zero());
    let dh = o.dirty_handle();
    exec.spawn_wired(branch_trailing(clk.clone(), o), vec![dh], vec![]);
    let sim: Vec<u8> = (0..CYCLES)
        .map(|_| {
            exec.tick_clock(&mut clk);
            obs.read().as_u128() as u8
        })
        .collect();
    let expected: Vec<u8> = (1..=CYCLES).map(|k| (k / 2) as u8).collect();
    assert_eq!(sim, expected, "simulator behaviour changed");

    // The emission bug this probe used to pin (`(n + 8'd1)[0]`, illegal SV) is
    // fixed — selects over compound bases now emit the width-cast form — so
    // the text must stay select-legal.
    let sv_text = copper_codegen::transpile_source(
        BRANCH_TRAILING_SRC,
        Some("branch_trailing"),
        &copper_codegen::EmitConfig::default(),
    )
    .expect("transpiles");
    assert!(
        !sv_text.contains(")["),
        "a select over a parenthesized expression is back in the emitted SV — \
         the emit.rs select_legal fallback regressed"
    );

    if !verilator_available() {
        return;
    }
    let sv = transpile_and_run(BRANCH_TRAILING_SRC, "branch_trailing", "clk", "o", "");
    eprintln!("branch_trailing: sim = {sim:?}\n                 sv  = {sv:?}  -> {}", if sim == sv { "AGREE" } else { "DIVERGE" });
    let one_cycle_late: Vec<u8> = (0..CYCLES).map(|k| (k / 2) as u8).collect();
    assert_eq!(
        sv, one_cycle_late,
        "the extracted-route divergence is no longer the one-edge-late trace — \
         if sim == sv the trailing lowering is fixed for the top-level-last-tick \
         class: re-bless this probe, EXPECTED_TRAILING, and the phase-C scope \
         notes in design_docs/PAIRED_IMPLEMENTATION_SCOPE.md together"
    );
    assert_ne!(
        sim, sv,
        "sim and SV now AGREE for extracted trailing after a TOP-LEVEL last \
         tick — phase C landed for this class; re-bless this probe, \
         EXPECTED_TRAILING, and the phase-C scope notes together"
    );
}
