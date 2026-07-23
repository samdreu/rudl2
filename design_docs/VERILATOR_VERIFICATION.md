# Verilator Verification Setup

## Overview

The copper HDL now includes automatic verification against Verilator to ensure that the Rust simulation matches the Verilog output.

**Type convention:** Verification examples use primitive integers and `Logic` vectors because they describe the host-side test harness. In Copper hardware modules, prefer `Logic` and `Bits<N>` for actual signals and datapaths.

## How It Works

1. **Simulation Trace**: As you run your Rust simulation, you record the inputs and outputs at each cycle into a `SimulationTrace`
2. **Testbench Generation**: The verification module generates a C++ testbench that applies the same inputs and checks the outputs
3. **Verilator Compilation**: The Verilog is compiled with Verilator along with the testbench
4. **Verification**: The testbench runs and compares outputs cycle-by-cycle

## Installation

### macOS
```bash
brew install verilator
```

### Linux (Ubuntu/Debian)
```bash
sudo apt-get install verilator
```

### Verify Installation
```bash
verilator --version
```

## Usage Example

See `examples/inverter.rs` for a complete example:

```rust
use copper_sim::SimulationTrace;

fn main() {
    let mut inv = Inverter::<1>::new();
    let mut trace = SimulationTrace::new();
    
    // Run simulation and record trace
    inv.set_input([Logic::Zero]);
    inv.design();
    trace.add_cycle(
        0,
        vec![("input".to_string(), vec![Logic::Zero])],
        vec![("output".to_string(), vec![Logic::One])],
    );
    
    // Generate Verilog
    let verilog = copper_codegen::to_verilog(&inv);
    fs::write("inverter.v", &verilog).expect("Failed to write Verilog file");
    
    // Verify with Verilator
    match copper_sim::verify_with_verilator("inverter.v", "inverter", &trace) {
        Ok(true) => println!("✓ Verification passed!"),
        Ok(false) => println!("✗ Verification failed"),
        Err(e) => println!("⚠ Error: {}", e),
    }
}
```

## What Gets Generated

### Verilog File (inverter.v)
```verilog
module inverter (
  input  wire input,
  output wire output
);
  assign output = (((input) == 1'b0) ? 1'b1 : ((input) == 1'b1) ? 1'b0 : ...);
endmodule
```

### C++ Testbench (tb_inverter.cpp)
```cpp
#include "Vinverter.h"
#include "verilated.h"

int main(int argc, char** argv) {
    Vinverter *top = new Vinverter();
    
    // Cycle 0: Input=0, Expected Output=1
    top->input = 0;
    top->eval();
    if (top->output != 1) {
        std::cout << "FAIL" << std::endl;
        return 1;
    }
    
    // ... more cycles ...
    
    std::cout << "All tests passed!" << std::endl;
    return 0;
}
```

## Running Verification

```bash
# Run the example (which includes verification)
cargo run --example inverter

# Expected output:
# Cycle 0: Input: Zero, Output: [One]
# Cycle 1: Input: One, Output: [Zero]
# ...
# Generated Verilog written to inverter.v
# Generated testbench: tb_inverter.cpp
# Running Verilator...
# Verilator compilation successful
# Running Verilator simulation...
# PASS: Cycle 0
# PASS: Cycle 1
# PASS: Cycle 2
# All tests passed!
# ✓ Verilator verification passed!
```

## Files Generated

After running with verification:
- `inverter.v` - Generated Verilog module
- `tb_inverter.cpp` - C++ testbench
- `obj_dir/` - Verilator build directory (contains compiled simulation)
- `obj_dir/Vinverter` - Executable simulation

## Troubleshooting

### Verilator not found
Make sure Verilator is installed and in your PATH:
```bash
which verilator
verilator --version
```

### Compilation errors
Check the Verilog syntax:
```bash
verilator --lint-only inverter.v
```

### Simulation mismatches
The verification will show which cycle failed:
```
FAIL: Cycle 2 output expected 1 got 0
```

This indicates the Rust simulation and Verilog simulation disagree at cycle 2.

## Advanced: Multi-bit Signals

For signals wider than 1 bit:
```rust
trace.add_cycle(
    0,
    vec![("data".to_string(), vec![Logic::One, Logic::Zero, Logic::One, Logic::One])], // 4-bit: 0b1101 = 13
    vec![("result".to_string(), vec![Logic::Zero, Logic::One, Logic::One, Logic::Zero])], // 4-bit: 0b0110 = 6
);
```

The verification module automatically converts bit vectors to integers for the C++ testbench.
