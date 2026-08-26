//! Shared harness for transpiler equivalence tests.
//!
//! Every equivalence test does the same three things:
//!   1. transpile a DUT fixture to SystemVerilog,
//!   2. run the DUT in the Copper simulator, recording actual outputs per cycle
//!      alongside a reference model's expected outputs,
//!   3. Verilate the *generated* `.sv` against the simulator's own trace.
//!
//! Only steps 1 and 3 are boilerplate; the per-DUT parts (port wiring, stimulus,
//! reference model) stay in each test because DUT signatures genuinely differ.
//! This module factors out the boilerplate and the assertion.
//!
//! Each DUT fixture in `tests/fixtures/` is used two ways — `include!`d for
//! simulation and `include_str!`d for transpilation — so the simulated and
//! transpiled designs are byte-identical by construction.
//!
//! Note the division of labour when a test fails:
//!   * `verilator: FAIL` → the transpiler disagrees with the simulator (a real
//!     transpiler bug — the simulator is the semantic source of truth).
//!   * `trace: FAIL` with `verilator: PASS` → the transpiler is fine and the
//!     test's own reference model is wrong.

#![allow(dead_code)] // each test binary uses a different subset

use std::path::PathBuf;

use copper_core::types::Bits;
use copper_core::Logic;
use copper_sim::{make_cycle, CycleData, HardwareTest, SimulationTrace};

/// Whether Verilator can be used, for tests that drive it directly rather than
/// through `EquivalenceTest`.
///
/// Returns `false` **only** when the binary is genuinely absent (the one skippable
/// case). If Verilator is installed but fails to run — classically a stale
/// `VERILATOR_ROOT`, a known hazard on this project's machines (see CLAUDE.md) —
/// this **panics** instead of returning `false`.
///
/// That distinction is the whole point. A probe that treats "broken" as "absent"
/// makes a misconfigured environment indistinguishable from an unconfigured one, and
/// the Verilator arms of a test — often the only thing anchoring it to real hardware
/// — quietly do not run while the suite reports green.
pub fn verilator_available() -> bool {
    match copper_sim::verilator_status() {
        Ok(()) => true,
        Err(e) if e.starts_with(copper_sim::VERILATOR_NOT_INSTALLED) => {
            eprintln!("skipping: {e}");
            false
        }
        Err(e) => panic!("Verilator is present but unusable, so this test cannot be skipped:\n{e}"),
    }
}

/// A `verilator` invocation with `VERILATOR_ROOT` cleared — always use this rather
/// than `Command::new("verilator")`, so a stale value cannot break the build with a
/// confusing error after `verilator_available` has already said yes.
pub fn verilator_command() -> std::process::Command {
    let mut cmd = std::process::Command::new("verilator");
    cmd.env_remove("VERILATOR_ROOT");
    cmd
}

/// Convenience: `bool` → `Logic`.
pub fn logic(b: bool) -> Logic {
    if b {
        Logic::One
    } else {
        Logic::Zero
    }
}

/// A tiny deterministic PRNG (SplitMix64) for reproducible randomized/property
/// tests. Not cryptographic — just well-spread and seedable, so a failing case is
/// reproducible from its seed. Mirrors the generator `copper-sim` uses for
/// `PollOrder::Seeded`, kept here so tests need no external `rand` dependency.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A random `u8`.
    pub fn u8(&mut self) -> u8 {
        self.next_u64() as u8
    }

    /// A uniform value in `0..n` (n > 0). Modulo bias is irrelevant for these tests.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    /// A random single bit as `Logic`.
    pub fn logic(&mut self) -> Logic {
        logic(self.next_u64() & 1 == 1)
    }
}

/// A port payload that seeded random stimulus can be generated for, and that can be
/// handed to the trace recorder as bits.
///
/// The corpus is near-uniform in this respect — 175 `Bits<N>` ports and 123 `Logic`
/// ports, and no third payload shape outside the const-generic and `Vec` modules the
/// transpiler does not accept — which is what makes a mechanical sweep possible.
/// See `design_docs/CORPUS_DIFFERENTIAL_SWEEP.md`.
pub trait RandStim: Copy {
    /// A uniformly random value of this type.
    fn rand(rng: &mut Rng) -> Self;
    /// The value as a bit slice, in the layout `record_cycle` expects.
    fn as_bits(&self) -> Vec<Logic>;
}

impl RandStim for Logic {
    fn rand(rng: &mut Rng) -> Self {
        rng.logic()
    }
    fn as_bits(&self) -> Vec<Logic> {
        vec![*self]
    }
}

impl<const N: usize> RandStim for Bits<N> {
    /// Built bit by bit rather than from an integer: `from_usize` panics on a value
    /// too wide for `N`, and `from_u64` will not compile below `N = 64`. Per-bit is
    /// width-exact for every `N` the corpus uses and every one it might.
    fn rand(rng: &mut Rng) -> Self {
        let mut b = Bits::<N>::zero();
        for i in 0..N {
            b.as_array_mut()[i] = rng.logic();
        }
        b
    }
    fn as_bits(&self) -> Vec<Logic> {
        self.as_array().to_vec()
    }
}

/// Transpile a DUT fixture and write the SystemVerilog to a temp file.
///
/// `module` may be `None` when the fixture declares exactly one hardware module.
pub fn transpile_fixture(module_name: &str, src: &str, module: Option<&str>) -> PathBuf {
    let sv = copper_codegen::transpile_source(src, module, &copper_codegen::EmitConfig::default())
        .unwrap_or_else(|e| panic!("transpiling '{module_name}' failed: {e}"));
    let path = std::env::temp_dir().join(format!("copper_{module_name}.sv"));
    std::fs::write(&path, sv).expect("write generated sv");
    path
}

/// An equivalence check in progress: accumulates the simulator's actual trace
/// and the reference model's expected trace, then compares both and Verilates
/// the generated SystemVerilog against the actual trace.
pub struct EquivalenceTest {
    test: HardwareTest,
    expected: Vec<CycleData>,
    cycle: usize,
    /// The DUT source + selected module, retained so `finish` can run the G2
    /// structural register-inference check (`None` for `sim_only`, which has no src).
    src: Option<String>,
    module: Option<String>,
    /// A G2 reg-match request set by `with_reference_registers`: the path to an
    /// *independent hand-written* reference SV and the match mode.
    reg_check: Option<(String, copper_analysis::RegMatch)>,
    /// Set by [`EquivalenceTest::differential_only`]: this run has no reference
    /// model, and `record_differential` feeds the simulator's own outputs in as the
    /// expected trace. Kept as a flag rather than left implicit so the two recording
    /// modes cannot be mixed — see that constructor for why.
    differential: bool,
}

impl EquivalenceTest {
    /// Transpile `src` and begin an equivalence check for `module_name`.
    /// `module_name` must match the DUT's function name (Verilator's top module).
    pub fn new(module_name: &str, src: &str) -> Self {
        Self::for_module(module_name, src, None)
    }

    /// Like `new`, but selects one module from a multi-module fixture.
    pub fn for_module(module_name: &str, src: &str, module: Option<&str>) -> Self {
        let sv_path = transpile_fixture(module_name, src, module);
        EquivalenceTest {
            test: HardwareTest::new(module_name).with_verilog(
                sv_path.to_str().expect("temp path is valid UTF-8"),
            ),
            expected: Vec::new(),
            cycle: 0,
            src: Some(src.to_string()),
            module: module.map(str::to_string),
            reg_check: None,
            differential: false,
        }
    }

    /// Also structurally reg-match the DUT's **inferred** register set (from the
    /// shared control/liveness analysis) against an *independent hand-written*
    /// reference SV's sequential flip-flops (G2 of the impl plan). `mode` is
    /// `NameExact` for a faithful translation that mirrors the design's names,
    /// `StorageEquivalent` (count) for a genuinely independent reference. Asserted
    /// in `finish`; the reference must NOT be the transpiler's own output (circular).
    pub fn with_reference_registers(
        mut self,
        reference_sv_path: &str,
        mode: copper_analysis::RegMatch,
    ) -> Self {
        self.reg_check = Some((reference_sv_path.to_string(), mode));
        self
    }

    /// Override the transpiled module's SystemVerilog parameters for the Verilator
    /// cross-check (e.g. `&[("N", 8), ("N_1", 7)]`) — the same widths the
    /// simulator ran the parametric DUT at.
    pub fn with_params(mut self, params: &[(&str, i64)]) -> Self {
        self.test = self.test.with_params(params);
        self
    }

    /// Like `new`, but checks the simulator against the reference model only —
    /// no transpilation, no Verilator cross-check. Use this for DUTs where the
    /// transpiler is known not to agree with the simulator yet (see
    /// TRANSPILATION_TODO.md, "`pre_edge_barrier` invisible to phase
    /// extraction"), so the frontend/simulation semantics can still be
    /// verified independently of that gap.
    pub fn sim_only(module_name: &str) -> Self {
        EquivalenceTest {
            test: HardwareTest::new(module_name),
            expected: Vec::new(),
            cycle: 0,
            src: None,
            module: None,
            reg_check: None,
            differential: false,
        }
    }

    /// Begin a **differential-only** check: simulator vs the transpiled
    /// SystemVerilog under Verilator, with **no reference model**.
    ///
    /// The two are independent implementations of the same source, so comparing
    /// them is already an oracle — a divergence means one of them is wrong. What it
    /// cannot see is the case where both are wrong the same way, which is what a
    /// reference model (and, better, the independent Verilog in
    /// `examples/basejump/`) is for. Use this where no model exists; do not use it
    /// to retire one that does.
    ///
    /// Recording is a separate method ([`record_differential`](Self::record_differential))
    /// and the two are mutually exclusive by assertion. Feeding the simulator's own
    /// outputs back as the "expected" trace is exactly right here and a silent
    /// disaster anywhere else — a model that has quietly become a copy of the thing
    /// it is meant to check always passes. It must not be reachable by accident.
    pub fn differential_only(module_name: &str, src: &str, module: Option<&str>) -> Self {
        let mut eq = Self::for_module(module_name, src, module);
        eq.differential = true;
        eq
    }

    /// Record one cycle of a [`differential_only`](Self::differential_only) run: the
    /// inputs driven and the outputs the simulator produced. The simulator's own
    /// outputs become the expected trace, so the trace comparison is trivially
    /// satisfied and the check that carries the weight is the Verilator one.
    pub fn record_differential(&mut self, inputs: &[(&str, &[Logic])], actual: &[(&str, &[Logic])]) {
        assert!(
            self.differential,
            "record_differential on a modelled EquivalenceTest — it would replace the reference \
             model with a copy of the simulator, and the test would pass by construction"
        );
        self.record(inputs, actual, actual);
    }

    /// Record one cycle: the inputs driven, the outputs the simulator actually
    /// produced, and the outputs the reference model predicts. Cycle numbering
    /// is automatic.
    pub fn record(
        &mut self,
        inputs: &[(&str, &[Logic])],
        actual: &[(&str, &[Logic])],
        expected: &[(&str, &[Logic])],
    ) {
        assert!(
            !self.differential || std::ptr::eq(actual, expected),
            "record with a reference model on a differential_only run — pick one: a model, or \
             the simulator standing in for one"
        );
        let i = self.cycle;
        self.test.record_cycle(i, inputs, actual);
        self.expected.push(make_cycle(i, inputs, expected));
        self.cycle += 1;
    }

    /// Assert the simulator matched the reference model, that the generated
    /// SystemVerilog matches the simulator under Verilator, and — if
    /// `with_reference_registers` was set — that the inferred register set
    /// structurally matches the independent reference SV (G2).
    pub fn finish(self) {
        if let Some((ref_sv_path, mode)) = &self.reg_check {
            let src = self
                .src
                .as_deref()
                .expect("with_reference_registers requires a DUT source (not sim_only)");
            let sv = std::fs::read_to_string(ref_sv_path)
                .unwrap_or_else(|e| panic!("read reference SV {ref_sv_path}: {e}"));
            copper_analysis::assert_source_registers_match_reference_sv(
                src,
                self.module.as_deref(),
                &sv,
                *mode,
            );
        }
        let expected = SimulationTrace::from_cycles(self.expected);
        self.test.finish_with_expected(&expected).assert_passed();
    }
}
