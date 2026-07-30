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

use copper_core::Logic;
use copper_sim::{make_cycle, CycleData, HardwareTest, SimulationTrace};

/// Convenience: `bool` → `Logic`.
pub fn logic(b: bool) -> Logic {
    if b {
        Logic::One
    } else {
        Logic::Zero
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
        }
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
