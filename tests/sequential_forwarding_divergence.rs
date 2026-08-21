//! **KNOWN GAP, pinned.** Two silent sim ≠ synthesized-SV divergences, both from
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
//! # Divergence 2 — a combinational passthrough of a post-edge-produced signal
//!
//! `loop { out.write(inp.read()); clk.tick().await; }` transpiles to
//! `assign out = inp;` — zero cycles. In the simulator its leading read is
//! `Deferred`, so it samples at the *pre*-edge while its producer (a synchronizer)
//! updates at the *post*-edge, and the passthrough lags one cycle. Standalone the
//! two agree (a testbench drives the input before the edge, which coincides with
//! the pre-edge sample); the divergence only appears when the producer is another
//! clocked task.
//!
//! # Why nothing caught either one
//!
//! `tests/two_domain_hierarchy_cdc.rs` — the independent-hardware anchor for the
//! whole dual-clock design — is green because these two divergences **cancel**:
//! divergence 1 makes the flag assert a cycle early, divergence 2 makes the
//! consumer a cycle late, and the observable boundary lands on the same cycle as
//! the reference. `compensation_is_what_makes_the_hierarchy_anchor_pass` pins that
//! explicitly, so the anchor cannot be mistaken for evidence that the chain is
//! right. Correct either divergence alone and that test must be re-blessed.

mod common;
use common::{verilator_available, verilator_command};
use copper::sync_2ff;
use copper_core::port::{wire, In, Out};
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
fn compensation_is_what_makes_the_hierarchy_anchor_pass_known_gap() {
    // `tests/two_domain_hierarchy_cdc.rs` asserts the flag arrives at slow cycle 5,
    // matching its independent SV reference — and it does. But not because the chain
    // is right: the counter asserts a cycle EARLY (divergence 1) and the passthrough
    // consumer a cycle LATE (divergence 2), and the two cancel at the boundary.
    let (f, q, o) = chain_assert_cycles(false);
    assert_eq!((f, q, o), (3, 4, 5), "as-is chain timing changed");

    // Divergence 2 in isolation: the synchronizer's output reaches the consumer a
    // cycle later in the sim, where `assign out = flag_in` in the SV is immediate.
    assert_eq!(o, q + 1, "the combinational passthrough should lag by one in the sim");

    // Correct ONLY the counter and the cancellation is gone — the boundary moves to
    // 6, which no longer matches the reference's 5.
    let (fc, qc, oc) = chain_assert_cycles(true);
    assert_eq!((fc, qc, oc), (4, 5, 6), "corrected-counter chain timing changed");
    assert_ne!(
        oc, o,
        "correcting the counter no longer changes the boundary — the compensation \
         story has changed and two_domain_hierarchy_cdc.rs needs re-examining"
    );
}
