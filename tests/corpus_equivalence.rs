//! Corpus differential sweep, **phase 1** — the modules that transpile but had no
//! check that what they transpile *to* agrees with the simulator.
//!
//! See `design_docs/CORPUS_DIFFERENTIAL_SWEEP.md`. The short version: simulator vs
//! Verilated-emitted-SystemVerilog is already an oracle — two independent
//! implementations of one source — so a differential case needs no reference model,
//! and that is what makes it cheap enough to have one for *every* module. This file
//! is the hand-written original of that wiring; `build.rs` now generates the same
//! shape for every module in `tests/fixtures/` (phase 2, `tests/corpus_generated.rs`),
//! and the fixture cases that started life here moved there — including the
//! `control_extraction_dut` pair whose divergence this file found. What is left is
//! the `examples/` modules, which the generator does not reach yet (phase 3).
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
//!
//! The fixture files, by contrast, declare no domains and no imports at all — which
//! is what let the generator supply them mechanically.  Unlike an example, a fixture
//! also carries no `//!` header: `include!` cannot produce inner doc comments.

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
