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

impl<T: RandStim, const N: usize> RandStim for [T; N] {
    fn rand(rng: &mut Rng) -> Self {
        std::array::from_fn(|_| T::rand(rng))
    }
    /// Element-major, each element in its own bit order — the layout the hand-written
    /// array-port tests already record (`mux_equivalence.rs` flattens
    /// `data.iter().flat_map(|b| b.as_array())`), which is the array-port ABI in
    /// `design_docs/ARRAY_PORT_ABI.md`.
    fn as_bits(&self) -> Vec<Logic> {
        self.iter().flat_map(|e| e.as_bits()).collect()
    }
}

/// Transpile a DUT fixture and write the SystemVerilog to a temp file.
///
/// `module` may be `None` when the fixture declares exactly one hardware module.
///
/// The file is keyed on `(module, pid, counter)`, never on the module name alone —
/// the same rule as the Verilator work dirs. Thirteen module names occur twice in
/// the swept corpus (an `examples/` copy and a `tests/fixtures/` copy), both run in
/// the same test binary in parallel threads, and two of those pairs (`mac_pipeline`,
/// `ripple_carry_adder`) emit DIFFERENT SystemVerilog. A name-keyed path let one
/// case overwrite the other's SV between `for_module` and `finish`, so a test could
/// Verilate its twin's design against its own stimulus — a false-PASS mechanism as
/// well as a false-failure one.
pub fn transpile_fixture(module_name: &str, src: &str, module: Option<&str>) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    let sv = copper_codegen::transpile_source(src, module, &copper_codegen::EmitConfig::default())
        .unwrap_or_else(|e| panic!("transpiling '{module_name}' failed: {e}"));
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "copper_{module_name}_{}_{n}.sv",
        std::process::id()
    ));
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
    /// The module name, mirrored because `HardwareTest` keeps its own private and
    /// the reference leg needs to build a second one with the same Verilator top.
    name: String,
    /// A BEHAVIOURAL reference leg set by `with_hand_written_reference`: an
    /// independent Verilog implementation of the same behaviour, run over the same
    /// stimulus. `reg_check` above is the STRUCTURAL counterpart — it compares
    /// register sets, not traces — and the two are deliberately separate: a design
    /// can have the right flip-flops and the wrong behaviour, or the reverse.
    reference_sv: Option<String>,
    /// Mirrored because `with_params` folds them into `self.test` and the reference
    /// leg has to be Verilated at the same widths.
    params: Vec<(String, i64)>,
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
            name: module_name.to_string(),
            reference_sv: None,
            params: Vec::new(),
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
        self.params = params.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        self
    }

    /// Also run the recorded stimulus against an **independent hand-written Verilog
    /// implementation of the same behaviour** and require it to agree.
    ///
    /// # What this adds over the differential check
    ///
    /// The sweep's ordinary check is simulator vs the SystemVerilog the transpiler
    /// emitted — two implementations of one source, which is an oracle for *"the two
    /// agree"* and nothing more. Both can be wrong together, and a shared
    /// misconception between the executor and the lowering is exactly the failure it
    /// cannot see. A third implementation nobody derived from the other two closes
    /// that: the reference is what a hardware engineer would have written for this
    /// behaviour, so agreement means the semantics are right, not merely consistent.
    ///
    /// # What it does NOT add
    ///
    /// A reference written by whoever wrote the Copper module shares that person's
    /// model of the design. It catches lowering and transcription errors; it does
    /// not catch a misconception both files share. Only a genuinely third-party
    /// source (BaseJump STL and the like) buys that, which is why each reference
    /// file states its provenance in its header and why third-party is preferred
    /// wherever one exists.
    ///
    /// The reference's SystemVerilog module must be named the same as the Copper
    /// module — it is Verilated as the top, exactly as the transpiler's output is.
    pub fn with_hand_written_reference(mut self, reference_sv_path: &str) -> Self {
        self.reference_sv = Some(reference_sv_path.to_string());
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
            name: module_name.to_string(),
            reference_sv: None,
            params: Vec::new(),
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
        // BOTH LEGS RUN BEFORE EITHER IS ALLOWED TO FAIL. Asserting the transpiled
        // leg first would hide the reference result whenever both are wrong, which is
        // exactly the case a new fixture-plus-reference pair is most likely to be in —
        // two round trips to learn what one run already knew.
        //
        // Keep a copy: the transpiled leg consumes the trace, and the reference leg
        // must be driven with the SAME stimulus and held to the SAME expectations.
        let replay = self.expected.clone();
        let expected = SimulationTrace::from_cycles(self.expected);
        let transpiled = self.test.finish_with_expected(&expected);
        transpiled.print_summary();

        let reference = self.reference_sv.as_ref().map(|ref_path| {
            println!("=== {} vs INDEPENDENT reference {ref_path} ===", self.name);
            let mut ref_test = HardwareTest::new(&self.name).with_verilog(ref_path);
            if !self.params.is_empty() {
                let ps: Vec<(&str, i64)> =
                    self.params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
                ref_test = ref_test.with_params(&ps);
            }
            for c in &replay {
                let ins: Vec<(&str, &[Logic])> =
                    c.inputs.iter().map(|(n, v)| (n.as_str(), &v[..])).collect();
                let outs: Vec<(&str, &[Logic])> =
                    c.outputs.iter().map(|(n, v)| (n.as_str(), &v[..])).collect();
                ref_test.record_cycle(c.cycle, &ins, &outs);
            }
            let expected_ref = SimulationTrace::from_cycles(replay);
            let r = ref_test.finish_with_expected(&expected_ref);
            r.print_summary();
            r
        });

        let ref_failed = reference.as_ref().is_some_and(|r| !r.passed());
        if transpiled.passed() && !ref_failed {
            return;
        }

        // WHICH legs failed is the diagnosis, so say it rather than making the reader
        // infer it from two summaries. The transpiled leg alone means the transpiler
        // disagrees with the simulator. The REFERENCE leg alone is the finding this
        // whole mechanism exists for: the simulator and the transpiler agree with each
        // other and are both wrong, which no amount of differential testing can see.
        let mut report = String::new();
        let verdict = match (transpiled.passed(), ref_failed) {
            (false, false) => "the TRANSPILED SystemVerilog disagrees with the simulator",
            (true, true) => {
                "the simulator and the transpiled SystemVerilog AGREE WITH EACH OTHER and \
                 both disagree with the independent reference — a shared misconception, \
                 which is the case the differential check alone cannot see"
            }
            (false, true) => "both legs failed: the transpiler disagrees with the simulator, \
                              AND both disagree with the independent reference",
            (true, false) => unreachable!("early-returned above"),
        };
        report.push_str(&format!("equivalence failed for '{}': {verdict}\n", self.name));
        if !transpiled.passed() {
            report.push_str("\n-- simulator vs transpiled SystemVerilog --\n");
            report.push_str(&transpiled.errors.join("\n"));
        }
        if let Some(r) = reference.filter(|r| !r.passed()) {
            report.push_str("\n-- simulator vs independent reference --\n");
            report.push_str(&r.errors.join("\n"));
        }
        panic!("{report}");
    }
}
