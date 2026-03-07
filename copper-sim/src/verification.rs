use copper_core::{Logic, Module};
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

    /// Add a cycle's data to the trace. This includes the cycle number, input signal values, and output signal values.
    pub fn add_cycle(&mut self, cycle: usize, inputs: Vec<(String, Vec<Logic>)>, outputs: Vec<(String, Vec<Logic>)>) {
        self.cycles.push(CycleData { cycle, inputs, outputs });
    }
}

/// Generate a C++ testbench for Verilator
pub fn generate_testbench(module_name: &str, trace: &SimulationTrace) -> String {
    let mut tb = String::new();
    
    tb.push_str(&format!(r#"
#include "V{}.h"
#include "verilated.h"
#include <iostream>

int main(int argc, char** argv) {{
    Verilated::commandArgs(argc, argv);
    V{} *top = new V{}();
    
"#, module_name, module_name, module_name));

    // Generate test cycles
    for cycle_data in &trace.cycles {
        tb.push_str(&format!("    // Cycle {}\n", cycle_data.cycle));
        
    for (name, values) in &cycle_data.inputs {
        let value = logic_vec_to_int(values);
        tb.push_str(&format!("    top->{} = {};\n", name, value));
    }

    tb.push_str("    top->clk = 0;\n");
    tb.push_str("    top->eval();\n");
    tb.push_str("    top->clk = 1;\n");
    tb.push_str("    top->eval();\n");
        
        // Check outputs
        for (name, expected) in &cycle_data.outputs {
            let expected_val = logic_vec_to_int(expected);
            tb.push_str(&format!(r#"    
    if (top->{} != {}) {{
        std::cout << "FAIL: Cycle {} {} expected {} got " << (int)top->{} << std::endl;
        delete top;
        return 1;
    }}
"#, name, expected_val, cycle_data.cycle, name, expected_val, name));
        }
        
        tb.push_str(&format!("    std::cout << \"PASS: Cycle {}\" << std::endl;\n", cycle_data.cycle));
        tb.push_str("\n");
    }
    
    tb.push_str(r#"
    delete top;
    std::cout << "All tests passed!" << std::endl;
    return 0;
}
"#);
    
    tb
}

/// Convert Logic vector to integer for C++ testbench
fn logic_vec_to_int(values: &[Logic]) -> u64 {
    let mut result = 0u64;
    for (i, val) in values.iter().enumerate() {
        match val {
            Logic::One => result |= 1 << i,
            Logic::Zero => {},
            Logic::X => result |= 1 << i, // Treat X as 1 for now
        }
    }
    result
}

/// Run Verilator simulation and compare with expected trace
pub fn verify_with_verilator(
    verilog_file: &str,
    module_name: &str,
    trace: &SimulationTrace,
) -> Result<bool, String> {
    // Generate testbench
    let testbench = generate_testbench(module_name, trace);
    let tb_file = format!("tb_{}.cpp", module_name);
    fs::write(&tb_file, testbench)
        .map_err(|e| format!("Failed to write testbench: {}", e))?;
    
    println!("Generated testbench: {}", tb_file);
    
    // Check if verilator is available
    let verilator_check = Command::new("verilator")
        .arg("--version")
        .output();
    
    if verilator_check.is_err() {
        return Err("Verilator not found. Install with: brew install verilator (macOS) or apt-get install verilator (Linux)".to_string());
    }
    
    // Run Verilator
    println!("Running Verilator...");
    let verilator_output = Command::new("verilator")
        .args(&[
            "--cc",
            "--exe",
            "--build",
            "-Wall",
            "-CFLAGS", "-std=c++14",
            verilog_file,
            &tb_file,
        ])
        .env_remove("VERILATOR_ROOT")  // Remove VERILATOR_ROOT to avoid conflicts
        .output()
        .map_err(|e| format!("Failed to run Verilator: {}", e))?;
    
    if !verilator_output.status.success() {
        return Err(format!(
            "Verilator compilation failed:\n{}",
            String::from_utf8_lossy(&verilator_output.stderr)
        ));
    }
    
    println!("Verilator compilation successful");
    
    // Run the simulation
    let sim_exe = format!("./obj_dir/V{}", module_name);
    if !Path::new(&sim_exe).exists() {
        return Err(format!("Simulation executable not found: {}", sim_exe));
    }
    
    println!("Running Verilator simulation... Executable: {}", sim_exe);
    let sim_output = Command::new(&sim_exe)
        .output()
        .map_err(|e| format!("Failed to run simulation: {}", e))?;
    
    let stdout = String::from_utf8_lossy(&sim_output.stdout);
    println!("Verilator output:\n{}", stdout);
    
    if !sim_output.status.success() {
        return Err(format!("Simulation failed:\n{}", stdout));
    }
    
    // Check if all tests passed
    if stdout.contains("All tests passed!") {
        Ok(true)
    } else {
        Err("Verilator simulation did not pass all tests".to_string())
    }
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
