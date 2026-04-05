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
            let port = sanitize_port_name(name);
            tb.push_str(&format!("    top->{} = {};\n", port, value));
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
        // Cycles whose expected output contains X are skipped: Verilator is a 2-value
        // simulator and cannot propagate 4-state unknowns the same way as a 4-state sim.
        for (name, expected) in &cycle_data.outputs {
            let port = sanitize_port_name(name);
            if has_x(expected) {
                tb.push_str(&format!(
                    "    std::cout << \"SKIP: Cycle {cyc} {name} (X-state)\" << std::endl;\n",
                    cyc  = cycle_data.cycle,
                    name = name,
                ));
            } else {
                let expected_val = logic_vec_to_int(expected);
                tb.push_str(&format!(
                    "    if (top->{port} != {exp}) {{\n\
                     \x20\x20\x20\x20    std::cout << \"FAIL: Cycle {cyc} {name} expected {exp} got \" << (int)top->{port} << std::endl;\n\
                     \x20\x20\x20\x20    failures++;\n\
                     \x20\x20\x20\x20}} else {{\n\
                     \x20\x20\x20\x20    std::cout << \"PASS: Cycle {cyc} {name}\" << std::endl;\n\
                     \x20\x20\x20\x20}}\n",
                    port = port,
                    name = name,
                    exp  = expected_val,
                    cyc  = cycle_data.cycle,
                ));
            }
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

/// Apply the same keyword sanitization used by the Verilog code generator.
///
/// Verilog reserved words cannot be used as identifiers, so `ir_builder`
/// appends `_sig` to any port/signal name that is a keyword.  The testbench
/// must use the sanitized name to match the Verilator-compiled module struct.
fn sanitize_port_name(name: &str) -> String {
    const VERILOG_KEYWORDS: &[&str] = &[
        "input", "output", "inout", "wire", "reg", "module", "endmodule", "always",
        "assign", "begin", "end", "if", "else", "case", "endcase", "default", "bit",
    ];
    if VERILOG_KEYWORDS.contains(&name) {
        format!("{}_sig", name)
    } else {
        name.to_string()
    }
}

/// Return true if any bit in the vector is X (unknown).
fn has_x(values: &[Logic]) -> bool {
    values.iter().any(|v| matches!(v, Logic::X))
}

/// Convert Logic vector to integer for C++ testbench.
/// X bits are treated as 0 to match Verilator's 2-value simulation.
fn logic_vec_to_int(values: &[Logic]) -> u64 {
    let mut result = 0u64;
    for (i, val) in values.iter().enumerate() {
        match val {
            Logic::One  => result |= 1 << i,
            Logic::Zero | Logic::X => {}
        }
    }
    result
}

/// Run Verilator simulation and compare with expected trace.
/// The original public API — no waveform output.
pub fn verify_with_verilator(
    verilog_file: &str,
    module_name: &str,
    trace: &SimulationTrace,
) -> Result<bool, String> {
    verify_with_verilator_traced(verilog_file, module_name, trace, None)
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
    let tb_file   = format!("tb_{}.cpp", module_name);
    fs::write(&tb_file, &testbench)
        .map_err(|e| format!("Failed to write testbench: {}", e))?;

    println!("Generated testbench: {}", tb_file);

    // Check Verilator is installed
    if Command::new("verilator").arg("--version").output().is_err() {
        return Err(
            "Verilator not found. Install with: brew install verilator (macOS) \
             or apt-get install verilator (Linux)".to_string()
        );
    }

    // Build compile arguments
    let mut verilator_args: Vec<&str> = vec![
        "--cc", "--exe", "--build",
        "--top-module", module_name,
        "-Wall", "-Wno-DECLFILENAME",
        "-CFLAGS", "-std=c++14",
    ];
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
    let sim_exe = format!("./obj_dir/V{}", module_name);
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
