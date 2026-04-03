use copper_core::Logic;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use crate::verification::{SimulationTrace, CycleData, verify_with_verilator_traced};

/// Standardized hardware test harness.
///
/// Collects a simulation trace cycle-by-cycle, then on `finish()`:
/// - Compares against an expected trace (if provided via `finish_with_expected`)
/// - Exports a standard cycle-level VCD waveform (via `with_waveform`)
/// - Exports a phased VCD with pre/post clock edges (via `with_phased_waveform`)
/// - Exports a Verilator VCD with all internal module signals (via `with_verilator_waveform`)
/// - Cross-validates against Verilator (if configured via `with_verilog`)
///
/// # Cycle-level recording (simple)
/// ```rust,no_run
/// let mut test = HardwareTest::new("inverter")
///     .with_verilog("verilog/inverter.v")
///     .with_waveform("waveforms/inverter.vcd");
///
/// for (i, &input) in inputs.iter().enumerate() {
///     let output = inverter(Bit(input));
///     test.record_cycle(i + 1, &[("bit", &[input])], &[("out", &[output.0])]);
/// }
/// test.finish_with_expected(&expected).assert_passed();
/// ```
///
/// # Phased recording (pre/post clock edge)
/// ```rust,no_run
/// let mut test = HardwareTest::new("jk_ff")
///     .with_verilog("verilog/jk_ff.v")
///     .with_phased_waveform("waveforms/jk_ff_phased.vcd")
///     .with_verilator_waveform("waveforms/jk_ff_verilator.vcd");
///
/// for (cycle, (jv, kv)) in pattern.iter().enumerate() {
///     *j.lock().unwrap() = *jv;
///     *k.lock().unwrap() = *kv;
///
///     exec.poll_tasks();
///     let q_pre = *out.lock().unwrap();
///
///     exec.advance(&mut clk);
///     exec.poll_tasks();
///     let q_post = *out.lock().unwrap();
///
///     // Record inputs + pre/post outputs. Also populates actual_trace for Verilator.
///     test.record_cycle_phased(
///         cycle,
///         &[("j", &[jv.0]), ("k", &[kv.0])],  // inputs
///         &[("q", &[q_pre.0])],                  // pre-clock-edge outputs
///         &[("q", &[q_post.0])],                 // post-clock-edge outputs
///     );
/// }
/// test.finish().assert_passed();
/// ```
pub struct HardwareTest {
    name: String,
    verilog_path: Option<String>,
    waveform_path: Option<String>,
    phased_waveform_path: Option<String>,
    verilator_waveform_path: Option<String>,
    actual_trace: SimulationTrace,
    phase_data: Vec<PhasedCycleData>,
}

/// One simulation cycle with explicit pre-clock and post-clock signal snapshots.
struct PhasedCycleData {
    cycle: usize,
    /// All signals (inputs + outputs) sampled before the clock edge.
    pre_signals: Vec<(String, Vec<Logic>)>,
    /// All signals (inputs + outputs) sampled after the clock edge.
    post_signals: Vec<(String, Vec<Logic>)>,
}

impl HardwareTest {
    /// Create a new test harness. `name` is used as the display name and as the
    /// Verilator module name when verifying against Verilog.
    pub fn new(name: &str) -> Self {
        HardwareTest {
            name: name.to_string(),
            verilog_path: None,
            waveform_path: None,
            phased_waveform_path: None,
            verilator_waveform_path: None,
            actual_trace: SimulationTrace::new(),
            phase_data: Vec::new(),
        }
    }

    /// Set the Verilog file path for Verilator cross-verification.
    pub fn with_verilog(mut self, path: &str) -> Self {
        self.verilog_path = Some(path.to_string());
        self
    }

    /// Export a cycle-level VCD (one timestep per simulation cycle).
    /// Compatible with both `record_cycle` and `record_cycle_phased` workflows;
    /// in the phased case the post-clock values are used for this file.
    pub fn with_waveform(mut self, path: &str) -> Self {
        self.waveform_path = Some(path.to_string());
        self
    }

    /// Export a phased VCD that shows two timesteps per cycle:
    /// - Even time (clk=0): signals sampled before the clock edge (combinational settle)
    /// - Odd  time (clk=1): signals sampled after  the clock edge (registered update)
    ///
    /// This is populated by `record_cycle_phased`. Open in GTKWave to see
    /// the async/await emit-then-tick vs tick-then-emit differences.
    pub fn with_phased_waveform(mut self, path: &str) -> Self {
        self.phased_waveform_path = Some(path.to_string());
        self
    }

    /// Export a Verilator VCD that includes all internal module signals (registers, wires,
    /// hierarchy). Requires Verilator to be installed. The file is written by the Verilator
    /// simulation itself using `--trace`.
    pub fn with_verilator_waveform(mut self, path: &str) -> Self {
        self.verilator_waveform_path = Some(path.to_string());
        self
    }

    /// Record one simulation cycle into the actual trace.
    /// Use this for simple combinational or sequential modules.
    pub fn record_cycle(
        &mut self,
        cycle: usize,
        inputs: &[(&str, &[Logic])],
        outputs: &[(&str, &[Logic])],
    ) {
        let inputs_owned = to_owned_signals(inputs);
        let outputs_owned = to_owned_signals(outputs);
        self.actual_trace.add_cycle(cycle, inputs_owned, outputs_owned);
    }

    /// Record one simulation cycle with explicit pre-clock and post-clock output snapshots.
    ///
    /// - `inputs`       — driven input signals (same value across both phases)
    /// - `pre_outputs`  — output signals sampled *before* the clock edge (combinational)
    /// - `post_outputs` — output signals sampled *after*  the clock edge (registered)
    ///
    /// This automatically:
    /// 1. Stores pre/post data for the phased VCD (`with_phased_waveform`)
    /// 2. Adds `(inputs, post_outputs)` to the regular trace for Verilator verification
    pub fn record_cycle_phased(
        &mut self,
        cycle: usize,
        inputs: &[(&str, &[Logic])],
        pre_outputs: &[(&str, &[Logic])],
        post_outputs: &[(&str, &[Logic])],
    ) {
        // Build pre/post signal lists by combining inputs with each output set
        let pre_signals: Vec<(String, Vec<Logic>)> = inputs
            .iter()
            .chain(pre_outputs.iter())
            .map(|(n, v)| (n.to_string(), v.to_vec()))
            .collect();
        let post_signals: Vec<(String, Vec<Logic>)> = inputs
            .iter()
            .chain(post_outputs.iter())
            .map(|(n, v)| (n.to_string(), v.to_vec()))
            .collect();

        self.phase_data.push(PhasedCycleData { cycle, pre_signals, post_signals });

        // Also populate the regular trace (using post-clock values) for Verilator
        self.actual_trace.add_cycle(
            cycle,
            to_owned_signals(inputs),
            to_owned_signals(post_outputs),
        );
    }

    /// Finalize: export waveforms and run Verilator (if configured). No trace assertion.
    pub fn finish(self) -> TestResult {
        self.finish_internal(None)
    }

    /// Finalize: assert the actual trace matches `expected`, then export waveforms
    /// and run Verilator (if configured).
    pub fn finish_with_expected(self, expected: &SimulationTrace) -> TestResult {
        self.finish_internal(Some(expected))
    }

    fn finish_internal(self, expected: Option<&SimulationTrace>) -> TestResult {
        let mut result = TestResult {
            name: self.name.clone(),
            trace_match: None,
            verilator_ok: None,
            waveform_path: None,
            phased_waveform_path: None,
            verilator_waveform_path: None,
            errors: Vec::new(),
        };

        // 1. Compare against expected trace
        if let Some(expected) = expected {
            match compare_traces(&self.actual_trace, expected) {
                Ok(()) => result.trace_match = Some(true),
                Err(e) => {
                    result.trace_match = Some(false);
                    result.errors.push(e);
                }
            }
        }

        // 2. Export cycle-level VCD
        if let Some(ref path) = self.waveform_path {
            match self.actual_trace.export_vcd(path, &self.name) {
                Ok(()) => result.waveform_path = Some(path.clone()),
                Err(e) => result.errors.push(format!("VCD export failed: {}", e)),
            }
        }

        // 3. Export phased VCD (pre/post clock edge)
        if let Some(ref path) = self.phased_waveform_path {
            if !self.phase_data.is_empty() {
                match export_phased_vcd(path, &self.name, &self.phase_data) {
                    Ok(()) => result.phased_waveform_path = Some(path.clone()),
                    Err(e) => result.errors.push(format!("Phased VCD export failed: {}", e)),
                }
            } else {
                result.errors.push(
                    "with_phased_waveform configured but no phased data recorded \
                     (use record_cycle_phased instead of record_cycle)".to_string()
                );
            }
        }

        // 4. Run Verilator cross-validation (with optional trace output)
        if let Some(ref verilog_path) = self.verilog_path {
            let vcd_out = self.verilator_waveform_path.as_deref();
            match verify_with_verilator_traced(verilog_path, &self.name, &self.actual_trace, vcd_out) {
                Ok(true) => {
                    result.verilator_ok = Some(true);
                    result.verilator_waveform_path = self.verilator_waveform_path.clone();
                }
                Ok(false) => {
                    result.verilator_ok = Some(false);
                    result.errors.push("Verilator simulation did not pass all tests".to_string());
                }
                Err(e) if e.contains("not found") || e.contains("not installed") => {
                    println!("  [verilator] not available: {}", e);
                }
                Err(e) => {
                    result.verilator_ok = Some(false);
                    result.errors.push(format!("Verilator error: {}", e));
                }
            }
        } else if self.verilator_waveform_path.is_some() {
            result.errors.push(
                "with_verilator_waveform configured but no with_verilog path set".to_string()
            );
        }

        result
    }
}

// ─── Phased VCD export ───────────────────────────────────────────────────────

/// Export a phased VCD with two timesteps per simulation cycle.
///
/// VCD time convention:
///   `2 * cycle`     → clk = 0, signals at pre-clock-edge (combinational settle)
///   `2 * cycle + 1` → clk = 1, signals at post-clock-edge (registered update)
fn export_phased_vcd(
    path: &str,
    module_name: &str,
    phase_data: &[PhasedCycleData],
) -> Result<(), String> {
    if phase_data.is_empty() {
        return Err("Cannot export phased VCD: no phased cycle data recorded".to_string());
    }

    // Collect all unique signal names (from any pre or post snapshot)
    let mut signal_names: Vec<String> = Vec::new();
    for pd in phase_data {
        for (name, _) in pd.pre_signals.iter().chain(pd.post_signals.iter()) {
            if !signal_names.contains(name) {
                signal_names.push(name.clone());
            }
        }
    }

    // Infer widths from the first phase data that has each signal
    let mut widths: HashMap<String, usize> = HashMap::new();
    for pd in phase_data {
        for (name, vals) in pd.pre_signals.iter().chain(pd.post_signals.iter()) {
            widths.entry(name.clone()).or_insert(vals.len());
        }
    }

    // Assign VCD symbols: '!' onwards, clk gets the first available after signals
    let mut symbol_counter = '!' as u8;
    // Reserve first symbol for 'clk'
    let clk_sym = symbol_counter as char;
    symbol_counter += 1;

    let signal_symbols: Vec<(String, char)> = signal_names
        .iter()
        .map(|name| {
            let sym = symbol_counter as char;
            symbol_counter += 1;
            (name.clone(), sym)
        })
        .collect();

    let mut vcd = String::new();

    // Header
    vcd.push_str("$timescale 1ps $end\n");
    vcd.push_str(&format!("$scope module {} $end\n", module_name));

    // Declare clk
    vcd.push_str(&format!("$var wire 1 {} clk $end\n", clk_sym));

    // Declare all signals
    for (name, sym) in &signal_symbols {
        let width = widths.get(name).copied().unwrap_or(1);
        vcd.push_str(&format!("$var wire {} {} {} $end\n", width, sym, name));
    }

    vcd.push_str("$upscope $end\n");
    vcd.push_str("$enddefinitions $end\n");

    // Initial values at time 0 from the first cycle's pre-signals
    vcd.push_str("#0\n");
    vcd.push_str("$dumpvars\n");
    vcd.push_str(&format!("b0 {}\n", clk_sym)); // clk starts low

    let first = &phase_data[0];
    for (name, sym) in &signal_symbols {
        let vals = lookup_signal(&first.pre_signals, name).map(Vec::as_slice);
        let width = widths.get(name).copied().unwrap_or(1);
        vcd.push_str(&format!("{} {}\n", logic_vals_to_vcd(vals, width), sym));
    }
    vcd.push_str("$end\n");

    // One pre/post pair per cycle.
    // Start at time 2 (not 0) so cycle 0's pre-edge doesn't collide with the
    // $dumpvars block above, which is also at #0.
    for pd in phase_data {
        let t_pre  = (pd.cycle as u64) * 2 + 2;
        let t_post = t_pre + 1;

        // Pre-clock edge: clk = 0
        vcd.push_str(&format!("#{}\n", t_pre));
        vcd.push_str(&format!("b0 {}\n", clk_sym));
        for (name, sym) in &signal_symbols {
            let vals = lookup_signal(&pd.pre_signals, name).map(Vec::as_slice);
            let width = widths.get(name).copied().unwrap_or(1);
            vcd.push_str(&format!("{} {}\n", logic_vals_to_vcd(vals, width), sym));
        }

        // Post-clock edge: clk = 1
        vcd.push_str(&format!("#{}\n", t_post));
        vcd.push_str(&format!("b1 {}\n", clk_sym));
        for (name, sym) in &signal_symbols {
            let vals = lookup_signal(&pd.post_signals, name).map(Vec::as_slice);
            let width = widths.get(name).copied().unwrap_or(1);
            vcd.push_str(&format!("{} {}\n", logic_vals_to_vcd(vals, width), sym));
        }
    }

    write_vcd_file(path, vcd)
}

// ─── Trace comparison ────────────────────────────────────────────────────────

fn compare_traces(actual: &SimulationTrace, expected: &SimulationTrace) -> Result<(), String> {
    if actual.cycles.len() != expected.cycles.len() {
        return Err(format!(
            "Trace length mismatch: actual {} cycles, expected {} cycles",
            actual.cycles.len(),
            expected.cycles.len()
        ));
    }

    for (actual_cycle, expected_cycle) in actual.cycles.iter().zip(expected.cycles.iter()) {
        if actual_cycle.cycle != expected_cycle.cycle {
            return Err(format!(
                "Cycle number mismatch: actual {}, expected {}",
                actual_cycle.cycle, expected_cycle.cycle
            ));
        }

        for (exp_name, exp_vals) in &expected_cycle.outputs {
            let actual_vals = actual_cycle
                .outputs
                .iter()
                .find(|(n, _)| n == exp_name)
                .map(|(_, v)| v);

            match actual_vals {
                None => {
                    return Err(format!(
                        "Cycle {}: missing output signal '{}'",
                        actual_cycle.cycle, exp_name
                    ))
                }
                Some(vals) if vals != exp_vals => {
                    return Err(format!(
                        "Cycle {}: output '{}' mismatch — got {:?}, expected {:?}",
                        actual_cycle.cycle, exp_name, vals, exp_vals
                    ))
                }
                _ => {}
            }
        }
    }

    Ok(())
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Build a `CycleData` from slice references. Useful for constructing expected traces.
///
/// # Example
/// ```rust,no_run
/// let expected = SimulationTrace::from_cycles(vec![
///     make_cycle(1, &[("bit", &[Logic::Zero])], &[("out", &[Logic::One])]),
///     make_cycle(2, &[("bit", &[Logic::One])],  &[("out", &[Logic::Zero])]),
/// ]);
/// ```
pub fn make_cycle(
    cycle: usize,
    inputs: &[(&str, &[Logic])],
    outputs: &[(&str, &[Logic])],
) -> CycleData {
    CycleData {
        cycle,
        inputs: to_owned_signals(inputs),
        outputs: to_owned_signals(outputs),
    }
}

fn to_owned_signals(signals: &[(&str, &[Logic])]) -> Vec<(String, Vec<Logic>)> {
    signals
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_vec()))
        .collect()
}

/// Look up a signal by name in a flat signal list; returns None if not present.
fn lookup_signal<'a>(signals: &'a [(String, Vec<Logic>)], name: &str) -> Option<&'a Vec<Logic>> {
    signals.iter().find(|(n, _)| n == name).map(|(_, v)| v)
}

/// Format a `Logic` slice as a VCD binary value string.
/// `vals` is LSB-first; VCD format requires MSB-first.
/// If `vals` is None, outputs `bx` for `width` bits.
fn logic_vals_to_vcd(vals: Option<&[Logic]>, width: usize) -> String {
    match vals {
        None => format!("b{}", "x".repeat(width)),
        Some(v) => {
            let bits: String = v
                .iter()
                .rev() // LSB-first → MSB-first
                .map(|l| match l {
                    Logic::Zero => '0',
                    Logic::One  => '1',
                    Logic::X    => 'x',
                })
                .collect();
            format!("b{}", bits)
        }
    }
}

fn write_vcd_file(path: &str, vcd: String) -> Result<(), String> {
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory '{}': {}", parent.display(), e))?;
        }
    }
    fs::write(path, vcd).map_err(|e| format!("Failed to write VCD '{}': {}", path, e))
}

// ─── TestResult ──────────────────────────────────────────────────────────────

/// Result of a finalized `HardwareTest`.
pub struct TestResult {
    /// Test name.
    pub name: String,
    /// `Some(true)` if trace matched expected, `Some(false)` if mismatch,
    /// `None` if no expected trace was provided.
    pub trace_match: Option<bool>,
    /// `Some(true)` if Verilator passed, `Some(false)` if it failed,
    /// `None` if Verilator was not configured or not installed.
    pub verilator_ok: Option<bool>,
    /// Path to the exported cycle-level VCD, if export succeeded.
    pub waveform_path: Option<String>,
    /// Path to the exported phased VCD, if export succeeded.
    pub phased_waveform_path: Option<String>,
    /// Path to the Verilator-generated VCD, if it was produced.
    pub verilator_waveform_path: Option<String>,
    /// Collected error messages from any failed checks.
    pub errors: Vec<String>,
}

impl TestResult {
    /// Returns `true` if all configured checks passed (no hard failures).
    pub fn passed(&self) -> bool {
        self.trace_match != Some(false)
            && self.verilator_ok != Some(false)
            && self.errors.is_empty()
    }

    /// Print a human-readable summary of the test result.
    pub fn print_summary(&self) {
        println!("=== {} ===", self.name);
        if let Some(ok) = self.trace_match {
            println!("  trace:            {}", if ok { "PASS" } else { "FAIL" });
        }
        if let Some(ok) = self.verilator_ok {
            println!("  verilator:        {}", if ok { "PASS" } else { "FAIL" });
        }
        if let Some(ref path) = self.waveform_path {
            println!("  waveform:         {}", path);
        }
        if let Some(ref path) = self.phased_waveform_path {
            println!("  phased waveform:  {}", path);
        }
        if let Some(ref path) = self.verilator_waveform_path {
            println!("  verilator waves:  {}", path);
        }
        for err in &self.errors {
            println!("  ERROR: {}", err);
        }
    }

    /// Print summary and panic if any check failed.
    pub fn assert_passed(self) {
        self.print_summary();
        if !self.passed() {
            panic!(
                "Hardware test '{}' failed:\n{}",
                self.name,
                self.errors.join("\n")
            );
        }
    }
}
