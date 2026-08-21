use copper_core::Logic;
use std::collections::HashMap;
use std::fs;
use std::process::Command;
use std::path::Path;

/// Represents a trace of signal values over time
#[derive(Debug, Clone)]
pub struct SimulationTrace {
    pub cycles: Vec<CycleData>,
}

/// Represents the data for a single cycle in the simulation, including the cycle number, input signal values, and output signal values. 
/// This is used to compare against Verilator's output for verification.
#[derive(Debug, Clone)]
pub struct CycleData {
    pub cycle: usize,
    pub inputs: Vec<(String, Vec<Logic>)>,
    pub outputs: Vec<(String, Vec<Logic>)>,
}

impl SimulationTrace {
    /// Create a new empty simulation trace.
    pub fn new() -> Self {
        SimulationTrace { cycles: Vec::new() }
    }

    /// Create a trace from a pre-built list of cycles. Useful for constructing expected traces.
    pub fn from_cycles(cycles: Vec<CycleData>) -> Self {
        SimulationTrace { cycles }
    }

    /// Add a cycle's data to the trace.
    pub fn add_cycle(&mut self, cycle: usize, inputs: Vec<(String, Vec<Logic>)>, outputs: Vec<(String, Vec<Logic>)>) {
        self.cycles.push(CycleData { cycle, inputs, outputs });
    }

    /// Export the trace as a VCD (Value Change Dump) file viewable in GTKWave or similar tools.
    ///
    /// Signal widths are inferred from the first cycle. One VCD time unit = one simulation cycle.
    pub fn export_vcd(&self, path: &str, module_name: &str) -> Result<(), String> {
        if self.cycles.is_empty() {
            return Err("Cannot export VCD: trace has no cycles".to_string());
        }

        // Collect signal names and widths from first cycle, inputs then outputs
        let first = &self.cycles[0];
        let mut signals: Vec<(String, usize, char)> = Vec::new(); // (name, width, symbol)
        let mut symbol = '!' as u8;

        for (name, vals) in first.inputs.iter().chain(first.outputs.iter()) {
            signals.push((name.clone(), vals.len(), symbol as char));
            symbol += 1;
        }

        let symbol_map: HashMap<String, char> = signals
            .iter()
            .map(|(name, _, sym)| (name.clone(), *sym))
            .collect();

        let mut vcd = String::new();

        // Header
        vcd.push_str("$timescale 1ps $end\n");
        vcd.push_str(&format!("$scope module {} $end\n", module_name));
        for (name, width, sym) in &signals {
            vcd.push_str(&format!("$var wire {} {} {} $end\n", width, sym, name));
        }
        vcd.push_str("$upscope $end\n");
        vcd.push_str("$enddefinitions $end\n");

        // Initial values at time 0 from the first cycle
        vcd.push_str("#0\n");
        vcd.push_str("$dumpvars\n");
        for (name, _, sym) in &signals {
            let vals = first
                .inputs
                .iter()
                .chain(first.outputs.iter())
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_slice())
                .unwrap_or(&[]);
            vcd.push_str(&format!("{} {}\n", logic_vals_to_vcd(vals), sym));
        }
        vcd.push_str("$end\n");

        // One timestep per cycle
        for cycle_data in &self.cycles {
            vcd.push_str(&format!("#{}\n", cycle_data.cycle));
            for (name, vals) in cycle_data.inputs.iter().chain(cycle_data.outputs.iter()) {
                if let Some(&sym) = symbol_map.get(name) {
                    vcd.push_str(&format!("{} {}\n", logic_vals_to_vcd(vals), sym));
                }
            }
        }

        // Create parent directory if needed
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create directory '{}': {}", parent.display(), e))?;
            }
        }

        fs::write(path, vcd)
            .map_err(|e| format!("Failed to write VCD '{}': {}", path, e))?;

        Ok(())
    }
}

/// Format a `Logic` slice as a VCD binary value string (e.g. `b0`, `b1`, `b10x1`).
/// The slice is stored LSB-first; VCD format requires MSB-first.
fn logic_vals_to_vcd(vals: &[Logic]) -> String {
    let bits: String = vals
        .iter()
        .rev() // convert LSB-first storage to MSB-first VCD
        .map(|v| match v {
            Logic::Zero => '0',
            Logic::One => '1',
            Logic::X => 'x',
        })
        .collect();
    format!("b{}", bits)
}

/// Generate a C++ testbench for Verilator.
///
/// When `vcd_output_path` is `Some(path)`, the testbench includes `--trace`
/// instrumentation that writes a VCD of all internal module signals to `path`.
pub fn generate_testbench(
    module_name: &str,
    trace: &SimulationTrace,
    has_clock: bool,
    vcd_output_path: Option<&str>,
) -> String {
    let tracing = vcd_output_path.is_some();
    let mut tb = String::new();

    // Includes
    tb.push_str(&format!("#include \"V{}.h\"\n", module_name));
    tb.push_str("#include \"verilated.h\"\n");
    if tracing {
        tb.push_str("#include \"verilated_vcd_c.h\"\n");
    }
    tb.push_str("#include <iostream>\n\n");

    // main()
    tb.push_str("int main(int argc, char** argv) {\n");
    tb.push_str("    Verilated::commandArgs(argc, argv);\n");
    if tracing {
        tb.push_str("    Verilated::traceEverOn(true);\n");
    }
    tb.push_str(&format!("    V{} *top = new V{}();\n", module_name, module_name));

    // Trace setup
    if let Some(vcd_path) = vcd_output_path {
        // Escape backslashes for Windows paths in C string literal
        let escaped = vcd_path.replace('\\', "\\\\");
        tb.push_str("    VerilatedVcdC* tfp = new VerilatedVcdC();\n");
        tb.push_str("    top->trace(tfp, 99);\n"); // 99 = trace all hierarchy levels
        tb.push_str(&format!("    tfp->open(\"{}\");\n", escaped));
        tb.push_str("    uint64_t vcd_time = 0;\n");
    }

    tb.push_str("    int failures = 0;\n\n");

    // Test cycles
    for cycle_data in &trace.cycles {
        tb.push_str(&format!("    // Cycle {}\n", cycle_data.cycle));

        for (name, values) in &cycle_data.inputs {
            let value = logic_vec_to_int(values);
            tb.push_str(&format!("    top->{} = {};\n", name, value));
        }

        if has_clock {
            tb.push_str("    top->clk = 0;\n");
            tb.push_str("    top->eval();\n");
            if tracing {
                tb.push_str("    tfp->dump(vcd_time++);\n");
            }
            tb.push_str("    top->clk = 1;\n");
            tb.push_str("    top->eval();\n");
            if tracing {
                tb.push_str("    tfp->dump(vcd_time++);\n");
            }
        } else {
            tb.push_str("    top->eval();\n");
            if tracing {
                tb.push_str("    tfp->dump(vcd_time++);\n");
            }
        }

        // Output checks — log failures but keep running so the full waveform is captured.
        for (name, expected) in &cycle_data.outputs {
            let expected_val = logic_vec_to_int(expected);
            tb.push_str(&format!(
                "    if (top->{name} != {exp}) {{\n\
                 \x20\x20\x20\x20    std::cout << \"FAIL: Cycle {cyc} {name} expected {exp} got \" << (int)top->{name} << std::endl;\n\
                 \x20\x20\x20\x20    failures++;\n\
                 \x20\x20\x20\x20}} else {{\n\
                 \x20\x20\x20\x20    std::cout << \"PASS: Cycle {cyc} {name}\" << std::endl;\n\
                 \x20\x20\x20\x20}}\n",
                name = name,
                exp  = expected_val,
                cyc  = cycle_data.cycle,
            ));
        }

        tb.push_str("\n");
    }

    // Trace teardown
    if tracing {
        tb.push_str("    tfp->close();\n");
        tb.push_str("    delete tfp;\n");
    }

    tb.push_str("    delete top;\n");
    tb.push_str("    if (failures == 0) {\n");
    tb.push_str("        std::cout << \"All tests passed!\" << std::endl;\n");
    tb.push_str("        return 0;\n");
    tb.push_str("    } else {\n");
    tb.push_str("        std::cout << failures << \" test(s) failed.\" << std::endl;\n");
    tb.push_str("        return 1;\n");
    tb.push_str("    }\n");
    tb.push_str("}\n");

    tb
}

/// Convert Logic vector to integer for C++ testbench
fn logic_vec_to_int(values: &[Logic]) -> u64 {
    let mut result = 0u64;
    for (i, val) in values.iter().enumerate() {
        match val {
            Logic::One  => result |= 1 << i,
            Logic::Zero => {}
            Logic::X    => result |= 1 << i, // treat X as 1 for testbench purposes
        }
    }
    result
}

/// Unambiguous marker prefixing the *one* error that means "Verilator is not
/// installed on this machine" — the only condition a caller may legitimately treat
/// as a skip.
///
/// It exists because the alternative — matching prose like `contains("not found")`
/// — silently swallowed **real** build failures. Verilator's C++ stage emits
/// `fatal error: 'Vfoo.h' file not found` for a broken testbench, which matched that
/// substring, so a genuinely failing equivalence check reported PASS. Callers must
/// match this prefix and treat every other `Err` as a failure.
pub const VERILATOR_NOT_INSTALLED: &str = "VERILATOR_NOT_INSTALLED:";

/// Whether an error from [`verify_with_verilator`] means "Verilator is not installed"
/// — the *only* condition under which a caller may skip instead of fail.
///
/// Deliberately an exact-prefix match on [`VERILATOR_NOT_INSTALLED`], never a
/// substring search of the message. See the regression tests at the bottom of this
/// file for the real Verilator output that made the substring version unsafe.
pub fn is_missing_verilator(err: &str) -> bool {
    err.starts_with(VERILATOR_NOT_INSTALLED)
}

/// Whether the `verilator` binary is present and runnable.
///
/// Distinguishes three states that must not be conflated:
///   * `Ok(())` — present and `--version` succeeds.
///   * `Err(msg)` starting with [`VERILATOR_NOT_INSTALLED`] — the binary could not be
///     spawned at all. The only skippable case.
///   * `Err(msg)` without that prefix — the binary *is* installed but fails to run
///     (classically a stale `VERILATOR_ROOT`). That is a broken environment, not an
///     absent tool, and must surface loudly rather than masquerade as "not installed".
///
/// `VERILATOR_ROOT` is cleared for the probe exactly as it is for the build below,
/// so the probe and the thing it is probing for see the same environment.
pub fn verilator_status() -> Result<(), String> {
    match Command::new("verilator")
        .arg("--version")
        .env_remove("VERILATOR_ROOT")
        .output()
    {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!(
            "Verilator is installed but failed to run (`verilator --version` exited {}). \
             This is a broken environment, not a missing tool — it must not be skipped.\n{}{}",
            o.status,
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr),
        )),
        Err(e) => Err(format!(
            "{VERILATOR_NOT_INSTALLED} Verilator not found ({e}). \
             Install with: brew install verilator (macOS) or apt-get install verilator (Linux)"
        )),
    }
}

/// Run Verilator simulation and compare with expected trace.
/// The original public API — no waveform output.
pub fn verify_with_verilator(
    verilog_file: &str,
    module_name: &str,
    trace: &SimulationTrace,
) -> Result<bool, String> {
    verify_with_verilator_traced(verilog_file, module_name, trace, None, &[])
}

/// Run Verilator simulation, compare with expected trace, and optionally dump
/// an internal-signal VCD to `vcd_output_path`.
///
/// When `vcd_output_path` is `Some(path)`, Verilator is compiled with `--trace`
/// and the simulation writes a full VCD (including all internal signals, registers,
/// and hierarchy) to `path`. Open it in GTKWave to inspect internal state.
pub(crate) fn verify_with_verilator_traced(
    verilog_file: &str,
    module_name: &str,
    trace: &SimulationTrace,
    vcd_output_path: Option<&str>,
    params: &[(String, i64)],
) -> Result<bool, String> {
    let has_clock = verilog_has_clock_port(verilog_file)?;
    let tracing   = vcd_output_path.is_some();

    // Ensure the VCD output directory exists before running the simulation
    if let Some(vcd_path) = vcd_output_path {
        if let Some(parent) = Path::new(vcd_path).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create VCD directory: {}", e))?;
            }
        }
    }

    // Generate and write testbench
    let testbench = generate_testbench(module_name, trace, has_clock, vcd_output_path);
    let param_suffix: String = params
        .iter()
        .map(|(name, value)| format!("_{name}{value}"))
        .collect();
    let tb_file   = format!("tb_{module_name}{param_suffix}.cpp");
    fs::write(&tb_file, &testbench)
        .map_err(|e| format!("Failed to write testbench: {}", e))?;

    println!("Generated testbench: {}", tb_file);

    // Check Verilator is installed AND runnable. `verilator_status` separates
    // "absent" (skippable, marked with VERILATOR_NOT_INSTALLED) from "present but
    // broken" (must fail) — the old check used `.is_err()`, which only catches a
    // spawn failure, and reported prose the caller had to substring-match.
    verilator_status()?;

    // Module parameter overrides (`-GN=8`), so the Verilated design matches the
    // widths the simulator ran with. Built here so the strings outlive the args.
    let param_args: Vec<String> = params
        .iter()
        .map(|(name, value)| format!("-G{name}={value}"))
        .collect();

    // Verilated output directory, keyed on module name AND parameters.
    //
    // Two things shared a single `obj_dir` before this, and both let a test run
    // against the WRONG Verilated model:
    //
    //  * Verilator defaults to `obj_dir` in the CWD, which *every* equivalence test
    //    shared. Tests inside one binary run in parallel threads, so two Verilating at
    //    once clobbered each other.
    //  * Worse, the same module built at *different parameters* also collided —
    //    `wide_alu` is checked at `N=32` and then `N=64`. Same name, same directory,
    //    and `--build`'s make step decides by mtime, so the second build could be
    //    skipped as "up to date" and the N=64 stimulus then ran against the N=32
    //    model. That is the observed symptom: a Bits<64> DUT reading back 32-bit
    //    values, intermittently (mtime has 1-second granularity), and always clean
    //    under `--test-threads=1`.
    //
    // Nondeterministic *failure* is how this surfaced, but the same collision can
    // equally produce a false PASS — which is why the directory is now unique per
    // (module, parameters) rather than per module.
    let obj_dir = format!("obj_dir_{module_name}{param_suffix}");

    // Build compile arguments
    let mut verilator_args: Vec<&str> = vec![
        "--cc", "--exe", "--build",
        "--top-module", module_name,
        "--Mdir", &obj_dir,
        "-Wall", "-Wno-DECLFILENAME",
        "-CFLAGS", "-std=c++14",
    ];
    for p in &param_args {
        verilator_args.push(p);
    }
    if tracing {
        verilator_args.push("--trace");
    }
    verilator_args.push(verilog_file);
    verilator_args.push(&tb_file);

    println!("Running Verilator{}...", if tracing { " (with trace)" } else { "" });
    let verilator_output = Command::new("verilator")
        .args(&verilator_args)
        .env_remove("VERILATOR_ROOT")
        .output()
        .map_err(|e| format!("Failed to run Verilator: {}", e))?;

    if !verilator_output.status.success() {
        return Err(format!(
            "Verilator compilation failed:\n{}",
            String::from_utf8_lossy(&verilator_output.stderr)
        ));
    }

    println!("Verilator compilation successful");

    // Run simulation
    let sim_exe = format!("./{obj_dir}/V{module_name}");
    if !Path::new(&sim_exe).exists() {
        return Err(format!("Simulation executable not found: {}", sim_exe));
    }

    println!("Running Verilator simulation...");
    let sim_output = Command::new(&sim_exe)
        .output()
        .map_err(|e| format!("Failed to run simulation: {}", e))?;

    let stdout = String::from_utf8_lossy(&sim_output.stdout);
    println!("Verilator output:\n{}", stdout);

    if !sim_output.status.success() {
        return Err(format!("Simulation failed:\n{}", stdout));
    }

    if stdout.contains("All tests passed!") {
        Ok(true)
    } else {
        Err("Verilator simulation did not pass all tests".to_string())
    }
}

fn verilog_has_clock_port(verilog_file: &str) -> Result<bool, String> {
    let content = fs::read_to_string(verilog_file)
        .map_err(|e| format!("Failed to read Verilog file '{}': {}", verilog_file, e))?;

    let lowered = content.to_lowercase();
    Ok(lowered.contains("input") && lowered.contains("clk"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_logic_vec_to_int() {
        assert_eq!(logic_vec_to_int(&[Logic::Zero]), 0);
        assert_eq!(logic_vec_to_int(&[Logic::One]), 1);
        assert_eq!(logic_vec_to_int(&[Logic::Zero, Logic::One]), 2);
        assert_eq!(logic_vec_to_int(&[Logic::One, Logic::One]), 3);
    }
}


#[cfg(test)]
mod verilator_classification_tests {
    use super::*;

    /// The regression that matters. These are **real** messages Verilator produces
    /// on a genuine build failure, captured from the tool. The classifier used to be
    /// `err.contains("not found") || err.contains("not installed")`, which matched
    /// the first one — so a broken C++ testbench was reported as "Verilator not
    /// available", the equivalence check was skipped, and the test PASSED.
    #[test]
    fn real_build_failures_are_never_classified_as_missing() {
        let build_failures = [
            // clang, when the generated testbench includes the wrong header.
            "Verilator compilation failed:\n../tb_good.cpp:1:10: fatal error: \
             'Vwrong_name.h' file not found",
            // A module the transpiler failed to emit.
            "Verilator compilation failed:\n%Error-MODMISSING: bad.sv:2:5: \
             Cannot find file containing module: 'missing_child'",
            "Verilator compilation failed:\n%Error: bad2.sv:1:10: \
             Cannot find include file: 'nope.svh'",
            "Verilator compilation failed:\n%Error: bad3.sv:2:16: \
             Can't find definition of variable: 'undefined_sig'",
            // A stale VERILATOR_ROOT: installed, but unusable. Not a skip either.
            "Verilator is installed but failed to run (`verilator --version` exited \
             exit status: 1).\n%Error: verilator: VERILATOR_ROOT is set to inconsistent path.",
        ];
        for err in build_failures {
            assert!(
                !is_missing_verilator(err),
                "a real build failure would be silently skipped:\n{err}"
            );
        }
    }

    /// The one message that *is* a legitimate skip carries the marker.
    #[test]
    fn the_absent_binary_message_is_classified_as_missing() {
        let err = format!(
            "{VERILATOR_NOT_INSTALLED} Verilator not found (No such file or directory). \
             Install with: brew install verilator"
        );
        assert!(is_missing_verilator(&err));
    }

    /// `verilator_status` must not report a *working* install as missing. Vacuous
    /// where Verilator is absent, which is the honest thing for it to be.
    #[test]
    fn a_working_install_is_not_reported_missing() {
        if let Err(e) = verilator_status() {
            assert!(
                is_missing_verilator(&e),
                "verilator_status failed for a reason other than absence: {e}"
            );
        }
    }
}
