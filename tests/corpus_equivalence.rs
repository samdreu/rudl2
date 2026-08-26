//! Corpus differential sweep, **phase 1** — the modules that transpile but had no
//! check that what they transpile *to* agrees with the simulator.
//!
//! See `design_docs/CORPUS_DIFFERENTIAL_SWEEP.md`. The short version: simulator vs
//! Verilated-emitted-SystemVerilog is already an oracle — two independent
//! implementations of one source — so a differential case needs no reference model,
//! and that is what makes it cheap enough to have one for *every* module. This file
//! is the hand-written proof of that before `build.rs` generates them (phase 2);
//! the wiring below is exactly what the generator has to emit.
//!
//! Stimulus is seeded random, 200 cycles per module, every `In` port re-randomised
//! every cycle. Hand vectors walk the paths their author thought of; these walk the
//! ones nobody did. Where a module has a hand-vector test elsewhere, this does not
//! replace it — the two ask different questions.
//!
//! **What a failure means.** `verilator: FAIL` is a transpiler bug (the simulator is
//! the semantic source of truth). The trace comparison cannot fail here: with no
//! reference model, the simulator's own outputs are the expected trace.
//!
//! **What this cannot see:** a divergence needing a specific long input sequence
//! (random stimulus is weak on deep FSM states — the hand vectors and golden traces
//! stay), and anything where both sides are wrong the same way, which is what the
//! independent Verilog in `examples/basejump/` is for.
//!
//! The example files are `include!`d rather than copied, so the simulated and
//! transpiled designs are the same bytes. Each one goes in its own module because
//! they declare their own clock-domain types (`MainClk`, `ClkFast`, …), and their
//! `fn main` demos come along for the ride unused — hence the module-level `allow`.

mod common;

/// Cycles per module. Long enough for a `2`- or `3`-state FSM to be walked many
/// times over, short enough that ~11 Verilator builds stay under a minute.
const CYCLES: usize = 200;

/// The `(clk, In<Logic>, Out<Logic>)` shape — five of the eleven modules here, all
/// of them one-line passthroughs or synchronizer stages. The domains are separate
/// parameters because a synchronizer's input is deliberately in a *different* domain
/// from its clock.
macro_rules! logic_passthrough_differential {
    ($(#[$attr:meta])* $fname:ident, $dut:ident, $src:ident, $clk_dom:ty,
     ($in_name:literal, $in_dom:ty), ($out_name:literal, $out_dom:ty), $seed:expr) => {
        $(#[$attr])*
        #[test]
        fn $fname() {
            let mut eq = EquivalenceTest::differential_only(
                stringify!($dut),
                $src,
                Some(stringify!($dut)),
            );
            let mut rng = Rng::new($seed);
            let mut clk = Clock::<$clk_dom>::new();
            let mut exec = HardwareExecutor::new();
            let (d_drv, d_in) = wire::<Logic, $in_dom>(Logic::Zero);
            let (q_out, q_obs) = wire::<Logic, $out_dom>(Logic::Zero);
            let dh = q_out.dirty_handle();
            let reads = vec![d_in.wire_id()];
            exec.spawn_wired($dut(clk.clone(), d_in, q_out), vec![dh], reads);

            for _ in 0..CYCLES {
                let d = Logic::rand(&mut rng);
                d_drv.write(d);
                exec.tick_clock(&mut clk);
                let q = q_obs.read();
                // The port NAMES matter: the generated testbench addresses the
                // Verilated model by them, so a wrong one is a compile error in C++
                // rather than a mismatch — which is how this macro's first draft,
                // with `d`/`q` hard-coded, failed on three of the five DUTs.
                eq.record_differential(&[($in_name, &[d][..])], &[($out_name, &[q][..])]);
            }
            eq.finish();
        }
    };
}

// ── examples/cdc/flag_crossing.rs ────────────────────────────────────────────

#[allow(unused_imports, dead_code)]
mod flag_crossing {
    // The `include!` comes FIRST: these files open with `//!` module docs, and an
    // inner doc comment may not follow an item — a `use` above it would not parse.
    include!("../examples/cdc/flag_crossing.rs");
    const SRC: &str = include_str!("../examples/cdc/flag_crossing.rs");

    use crate::common::{EquivalenceTest, RandStim, Rng};
    use crate::CYCLES;

    // FOUND BY THIS SWEEP, 2026-08-25, and FIXED the same day: `event` is a
    // SystemVerilog keyword that was missing from `vlir_lower`'s legalizer list, so
    // this module emitted SystemVerilog Verilator could not parse ("syntax error,
    // unexpected event"). The example itself passed throughout — it is checked
    // against HAND-WRITTEN Verilog, and nothing had ever Verilated what it
    // transpiles to, which is the gap this whole file exists to close.
    //
    // The output is recorded under its LEGALIZED name: a reserved name gets `_sig`
    // appended, and the generated testbench addresses the Verilated model by the
    // emitted name, not the Rust one. Phase 2's generator needs that mapping — see
    // design_docs/CORPUS_DIFFERENTIAL_SWEEP.md.
    logic_passthrough_differential!(
        event_source_differential, event_source, SRC, ClkFast,
        ("trigger", ClkFast), ("event_sig", ClkFast), 0x5EED_0001
    );
    logic_passthrough_differential!(
        event_sink_differential, event_sink, SRC, ClkSlow,
        ("event_synced", ClkSlow), ("seen", ClkSlow), 0x5EED_0002
    );
}

// ── examples/cdc/two_domain_hierarchy.rs ─────────────────────────────────────

#[allow(unused_imports, dead_code)]
mod two_domain_hierarchy {
    // The `include!` comes FIRST: these files open with `//!` module docs, and an
    // inner doc comment may not follow an item — a `use` above it would not parse.
    include!("../examples/cdc/two_domain_hierarchy.rs");
    const SRC: &str = include_str!("../examples/cdc/two_domain_hierarchy.rs");

    use crate::common::{EquivalenceTest, RandStim, Rng};
    use crate::CYCLES;

    // `flag_sync`'s input is in the FAST domain and its clock is the slow one: that
    // crossing is the module's whole purpose, and is legal because it is declared a
    // `#[hardware(synchronizer)]`.
    logic_passthrough_differential!(
        flag_sync_differential, flag_sync, SRC, ClkSlow,
        ("d", ClkFast), ("q", ClkSlow), 0x5EED_0003
    );
    logic_passthrough_differential!(
        slow_consumer_differential, slow_consumer, SRC, ClkSlow,
        ("flag_in", ClkSlow), ("out", ClkSlow), 0x5EED_0004
    );

    /// No inputs at all — a free-running counter with a sticky flag. The stimulus is
    /// the clock, and the check is that both sides count and latch on the same edges.
    #[test]
    fn fast_counter_differential() {
        let mut eq = EquivalenceTest::differential_only("fast_counter", SRC, Some("fast_counter"));
        let mut clk = Clock::<ClkFast>::new();
        let mut exec = HardwareExecutor::new();
        let (c_out, c_obs) = wire::<Bits<8>, ClkFast>(Bits::zero());
        let (f_out, f_obs) = wire::<Logic, ClkFast>(Logic::Zero);
        let handles = vec![c_out.dirty_handle(), f_out.dirty_handle()];
        exec.spawn_wired(fast_counter(clk.clone(), c_out, f_out), handles, vec![]);

        for _ in 0..CYCLES {
            exec.tick_clock(&mut clk);
            let c = c_obs.read();
            let f = f_obs.read();
            eq.record_differential(
                &[],
                &[("count_out", &c.as_bits()[..]), ("flag_out", &[f][..])],
            );
        }
        eq.finish();
    }
}

// ── examples/sequential/pattern_detector_2.rs ────────────────────────────────

#[allow(unused_imports, dead_code)]
mod pattern_detector_2 {
    use crate::common::{EquivalenceTest, RandStim, Rng};
    use crate::CYCLES;
    use copper_core::port::registered_wire;

    include!("../examples/sequential/pattern_detector_2.rs");
    const SRC: &str = include_str!("../examples/sequential/pattern_detector_2.rs");

    /// The `.await`-per-state coding of the "010" detector — the one whose ticks live
    /// inside branches, so it is control-extracted before it lowers. Its twin
    /// `det_010` is checked against a hand-written golden elsewhere; this checks the
    /// coding that exercises the extraction path, on stimulus nobody chose.
    ///
    /// `rstn` is re-randomised every cycle rather than held: a reset asserted at an
    /// arbitrary point in the state walk is precisely the case a vector set is least
    /// likely to contain.
    #[test]
    fn det_010_awaits_differential() {
        let mut eq =
            EquivalenceTest::differential_only("det_010_awaits", SRC, Some("det_010_awaits"));
        let mut rng = Rng::new(0x5EED_0005);
        let mut clk = Clock::<MainClk>::new();
        let mut exec = HardwareExecutor::new();
        let (rstn_drv, rstn_in) = wire::<Logic, MainClk>(Logic::One);
        let (bit_drv, bit_in) = wire::<Logic, MainClk>(Logic::Zero);
        let (o_out, o_obs) = registered_wire::<Logic, MainClk>(&clk, Logic::Zero);
        let dh = o_out.dirty_handle();
        let reads = vec![rstn_in.wire_id(), bit_in.wire_id()];
        exec.spawn_wired(
            det_010_awaits(clk.clone(), rstn_in, bit_in, o_out),
            vec![dh],
            reads,
        );

        for _ in 0..CYCLES {
            let rstn = Logic::rand(&mut rng);
            let bit = Logic::rand(&mut rng);
            rstn_drv.write(rstn);
            bit_drv.write(bit);
            exec.tick_clock(&mut clk);
            let o = o_obs.read();
            eq.record_differential(
                &[("rstn", &[rstn][..]), ("in_i", &[bit][..])],
                &[("out_o", &[o][..])],
            );
        }
        eq.finish();
    }
}

// ── tests/fixtures/control_extraction_dut.rs ─────────────────────────────────

/// The branch- and match-nested-tick pairs. Each async module has a hand-written
/// explicit `match pc` twin, and `tests/control_extraction_structural.rs` asserts
/// the two transpile to identical SystemVerilog. That is a strong structural claim
/// and says nothing about whether either one *behaves* like the simulator, which is
/// what these add — for both halves, so the pair stays apples-to-apples.
#[allow(unused_imports, dead_code)]
mod control_extraction {
    use crate::common::{EquivalenceTest, RandStim, Rng};
    use crate::CYCLES;
    use copper_core::port::{registered_wire, wire, In, Out, RegOut};
    use copper_core::types::Bits;
    use copper_core::{Clock, ClockDomain, Logic};
    use copper_macros::hardware;
    use copper_sim::HardwareExecutor;

    struct MainClk;
    impl ClockDomain for MainClk {}

    include!("fixtures/control_extraction_dut.rs");
    const SRC: &str = include_str!("fixtures/control_extraction_dut.rs");

    /// One `In<Logic>` selecting a branch, three `Out<Logic>` marking which segments
    /// ran. `$dut` is the async coding or its explicit twin.
    macro_rules! branch_merge_differential {
        ($(#[$attr:meta])* $fname:ident, $dut:ident, $seed:expr) => {
            $(#[$attr])*
            #[test]
            fn $fname() {
                let mut eq = EquivalenceTest::differential_only(
                    stringify!($dut),
                    SRC,
                    Some(stringify!($dut)),
                );
                let mut rng = Rng::new($seed);
                let mut clk = Clock::<MainClk>::new();
                let mut exec = HardwareExecutor::new();
                let (sel_drv, sel_in) = wire::<Logic, MainClk>(Logic::Zero);
                let (h_out, h_obs) = wire::<Logic, MainClk>(Logic::Zero);
                let (m_out, m_obs) = wire::<Logic, MainClk>(Logic::Zero);
                let (t_out, t_obs) = wire::<Logic, MainClk>(Logic::Zero);
                let handles = vec![
                    h_out.dirty_handle(),
                    m_out.dirty_handle(),
                    t_out.dirty_handle(),
                ];
                let reads = vec![sel_in.wire_id()];
                exec.spawn_wired(
                    $dut(clk.clone(), sel_in, h_out, m_out, t_out),
                    handles,
                    reads,
                );

                for _ in 0..CYCLES {
                    let sel = Logic::rand(&mut rng);
                    sel_drv.write(sel);
                    exec.tick_clock(&mut clk);
                    let (h, m, t) = (h_obs.read(), m_obs.read(), t_obs.read());
                    eq.record_differential(
                        &[("sel", &[sel][..])],
                        &[
                            ("head_o", &[h][..]),
                            ("mid_o", &[m][..]),
                            ("tail_o", &[t][..]),
                        ],
                    );
                }
                eq.finish();
            }
        };
    }

    branch_merge_differential!(branch_merge_differential_case, branch_merge, 0x5EED_0006);
    // FOUND BY THIS SWEEP, 2026-08-25 — a MEASURED sim ≠ synth divergence, and the
    // sharpest statement of it available: this module and `branch_merge` above
    // transpile to BYTE-IDENTICAL SystemVerilog (asserted by
    // control_extraction_structural.rs, re-confirmed by hand), the async one agrees
    // with that SV for 200 random cycles, and this one leads it by a cycle.
    //
    // Mechanism, measured both sides at seed 0x5EED_0007, cycle 0 (sel = 1):
    // the `pc = 1` arm writes `tail_o` and belongs to the NEXT cycle, but the
    // simulator runs the next iteration's pre-tick segment during the post-edge
    // settle of this tick, so `tail_o` reads 1 a cycle before the hardware sets it.
    // sim: head/mid/tail = 1/0/1 · SV: 1/0/0. It shows up exactly once because
    // these outputs only ever latch high.
    //
    // This is the pre-tick alignment family (D1), in a shape the guardrail exempts:
    // a CONSTANT write, on the grounds that a constant is idempotent across the
    // phase shift. That holds only if the write happens every cycle. Here it is
    // conditional — the other path leaves the port HOLDING — so *when* it lands is
    // observable. `unprotected_pretick_out_write` returns [] for both twins.
    //
    // Minimised, pinned and written up: sequential_forwarding_divergence.rs
    // (`pc_arm_write` / `pc_arm_toggle`, whose traces are each other shifted by
    // exactly one cycle) and PRETICK_ALIGNMENT_GUARDRAIL.md §5.5.
    branch_merge_differential!(
        #[ignore = "MEASURED DIVERGENCE, pinned as sequential_forwarding_divergence.rs::a_write_in_a_state_arm_leads_the_hardware_by_one_cycle and written up in PRETICK_ALIGNMENT_GUARDRAIL.md 5.5 — the simulator leads the identical emitted SV by one cycle. Un-ignore when the constant-write exemption is narrowed"]
        branch_merge_explicit_differential_case,
        branch_merge_explicit,
        0x5EED_0007
    );

    /// Two `In<Bits<8>>` and a `RegOut<Bits<8>>`, alternating between them under a
    /// two-state `match`.
    macro_rules! match_tick_differential {
        ($fname:ident, $dut:ident, $seed:expr) => {
            #[test]
            fn $fname() {
                let mut eq = EquivalenceTest::differential_only(
                    stringify!($dut),
                    SRC,
                    Some(stringify!($dut)),
                );
                let mut rng = Rng::new($seed);
                let mut clk = Clock::<MainClk>::new();
                let mut exec = HardwareExecutor::new();
                let (a_drv, a_in) = wire::<Bits<8>, MainClk>(Bits::zero());
                let (b_drv, b_in) = wire::<Bits<8>, MainClk>(Bits::zero());
                let (o_out, o_obs) = registered_wire::<Bits<8>, MainClk>(&clk, Bits::zero());
                let dh = o_out.dirty_handle();
                let reads = vec![a_in.wire_id(), b_in.wire_id()];
                exec.spawn_wired($dut(clk.clone(), a_in, b_in, o_out), vec![dh], reads);

                for _ in 0..CYCLES {
                    let a = Bits::<8>::rand(&mut rng);
                    let b = Bits::<8>::rand(&mut rng);
                    a_drv.write(a);
                    b_drv.write(b);
                    exec.tick_clock(&mut clk);
                    let o = o_obs.read();
                    eq.record_differential(
                        &[("a", &a.as_bits()[..]), ("b", &b.as_bits()[..])],
                        &[("out", &o.as_bits()[..])],
                    );
                }
                eq.finish();
            }
        };
    }

    match_tick_differential!(match_tick_differential_case, match_tick, 0x5EED_0008);
    match_tick_differential!(
        match_tick_explicit_differential_case,
        match_tick_explicit,
        0x5EED_0009
    );
}

// ── tests/fixtures/sync_2ff_dut.rs ───────────────────────────────────────────

/// The concrete specialization of the standard-library synchronizer. It is anchored
/// in the simulator (`tests/cdc_synchronizer_anchor.rs` proves it behaves identically
/// to the library generic), and against hand-written CDC Verilog — but nothing until
/// now compared it to its own emitted SystemVerilog.
#[allow(unused_imports, dead_code)]
mod sync_2ff {
    use crate::common::{EquivalenceTest, RandStim, Rng};
    use crate::CYCLES;
    use copper_core::port::{wire, In, Out};
    use copper_core::{Clock, ClockDomain, Logic};
    use copper_macros::hardware;
    use copper_sim::HardwareExecutor;

    struct SrcClk;
    impl ClockDomain for SrcClk {}
    struct DstClk;
    impl ClockDomain for DstClk {}

    include!("fixtures/sync_2ff_dut.rs");
    const SRC: &str = include_str!("fixtures/sync_2ff_dut.rs");

    logic_passthrough_differential!(
        sync_2ff_concrete_differential, sync_2ff_concrete, SRC, DstClk,
        ("d", SrcClk), ("q", DstClk), 0x5EED_000A
    );
}
