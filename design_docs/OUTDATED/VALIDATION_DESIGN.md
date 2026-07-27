# Copper Transpilation Validation Design — Phase F

## Purpose

This document defines Phase F of the Copper transpilation pipeline: Validation. Phase F takes emitted Verilog (Phase E output) and a Copper simulation trace and verifies that the generated RTL is both syntactically valid and behaviorally equivalent to the Copper simulation.

Phase F is the quality gate. A transpilation output that does not pass Phase F is a transpiler bug.

---

## What Phase F Is and Is Not

**Phase F is:**
- A verification harness, not part of the compiler itself
- Composed of multiple independent checks at different levels of rigor
- The authoritative source of pass/fail for transpilation correctness
- The mechanism for catching semantic mismatches between Copper simulation and emitted RTL

**Phase F is not:**
- A correctness proof — it is simulation-based testing
- Exhaustive over all possible inputs
- Required to run on every build (it is an optional but highly recommended gate)

---

## Three-Level Verification Model

Phase F runs three levels of checks, each building on the previous:

```
Level 1: Syntax / Lint
    └─ Is the emitted Verilog syntactically valid for the target toolchain?

Level 2: Simulation Equivalence
    └─ Does the Verilog simulate to the same outputs as the Copper trace?

Level 3: Formal / Structural Checks (future)
    └─ Do timing and assignment rules hold structurally in the emitted RTL?
```

Milestone 1 targets Level 1 and Level 2. Level 3 is deferred.

---

## Level 1: Syntax / Lint

### What it checks

- The emitted Verilog parses without errors under the target toolchain
- No undefined variables, undeclared ports, width mismatches
- No mixed blocking/non-blocking on the same signal (tool-reported)
- No implicit net declarations (profile-dependent)

### How it runs

For the Verilator profile:
```bash
verilator --lint-only --sv <module>.sv
```

For the Yosys profile:
```bash
yosys -p "read_verilog -sv <module>.sv; check"
```

For the generic profile, use Verilator lint-only as the default check.

### Pass criteria

Exit code 0 with no errors. Warnings are collected and reported but do not fail the lint check unless they are in the configured "fatal warnings" list.

Fatal warning categories (always fail):
- `WIDTHEXPAND` — implicit width extension
- `WIDTHTRUNC` — implicit width truncation
- `BLKSEQ` — blocking assignment in sequential block
- `BLKNBA` — non-blocking assignment in combinational block
- `UNOPTFLAT` — combinational loop detected

---

## Level 2: Simulation Equivalence

### Overview

A C++ testbench is generated from the Copper simulation trace. The testbench applies the same inputs cycle-by-cycle and asserts that the Verilog outputs match the expected outputs from the trace.

This is the primary behavioral check. It directly tests the contract from `VERILOG_OUTPUT_STANDARDS.md` §2: Copper simulator behavior is the semantic source of truth.

### The `SimulationTrace` type

Copper simulations record a `SimulationTrace` using `HardwareTest::record_cycle`:

```rust
pub struct SimulationTrace {
    pub cycles: Vec<SimulationCycle>,
}

pub struct SimulationCycle {
    pub cycle_idx: usize,
    pub inputs: Vec<(String, Vec<Logic>)>,    // (port_name, bit_values)
    pub outputs: Vec<(String, Vec<Logic>)>,   // (port_name, expected_bit_values)
}
```

Each cycle records:
- The input values applied at that cycle
- The expected output values observed from the Copper simulation

### Testbench Generation

Phase F generates a C++ testbench from the trace:

```cpp
#include "V<module_name>.h"
#include "verilated.h"
#include <iostream>

int main(int argc, char** argv) {
    Verilated::commandArgs(argc, argv);
    V<module_name> *top = new V<module_name>();

    // Cycle 0
    top->clk = 0; top->eval();
    top-><input_port> = <value>;
    top->clk = 1; top->eval();    // rising edge
    if (top-><output_port> != <expected>) {
        std::cerr << "FAIL cycle 0: <output_port> expected <expected> got " << top-><output_port> << std::endl;
        return 1;
    }
    std::cout << "PASS cycle 0" << std::endl;

    // ... more cycles ...

    delete top;
    std::cout << "ALL TESTS PASSED" << std::endl;
    return 0;
}
```

### Clock Handling in Testbench

For sequential modules, the testbench drives the clock explicitly:

```
1. Set inputs
2. Drive clk = 0, eval (pre-edge settle)
3. Drive clk = 1, eval (rising edge + post-edge settle)
4. Read outputs
5. Assert
```

This matches the Copper executor's `tick_clock` sequence:
```
poll_tasks (pre-edge)  → clk.advance() (rising edge) → poll_tasks (post-edge)
```

For combinational modules, there is no clock and the testbench simply:
```
1. Set inputs
2. eval()
3. Read and assert outputs
```

### Logic::X Handling

Copper simulation has a three-valued `Logic::X` (unknown). Verilator is a two-value simulator. For cycles where the expected output contains `Logic::X`, the equivalence check is skipped and a `SKIP` is printed instead of `PASS` or `FAIL`. This is correct behavior: X means "don't care" or "unknown", not "expected to be 0".

```cpp
// If expected value was X (unknown in Copper sim), skip check
// SKIP cycle 3: output contains X
```

### Port Name Mapping

Port names in the testbench are derived from the legalized names in VLIR. The testbench generator uses the same legalization table Phase D built, so `bit` → `bit_sig`, `reg` → `reg_sig`, etc. are applied consistently.

```rust
fn legalize_for_testbench(name: &str) -> String {
    // same keyword list as Phase D Pass 1
}
```

### Integer Conversion

Copper traces store values as `Vec<Logic>` (bit vectors, LSB-first). The testbench needs integer values:

```rust
fn logic_vec_to_int(bits: &[Logic]) -> u128 {
    bits.iter().enumerate().fold(0u128, |acc, (i, b)| {
        match b {
            Logic::One  => acc | (1u128 << i),
            Logic::Zero => acc,
            Logic::X    => acc,  // X treated as 0 in 2-value sim
        }
    })
}
```

For output checking, if any bit in the expected output is `Logic::X`, the entire cycle's output check is skipped.

### Compilation and Execution

```bash
# Compile Verilog with Verilator
verilator --cc --exe --sv <module>.sv tb_<module>.cpp -o V<module>

# Build
make -C obj_dir -f V<module>.mk

# Run
./obj_dir/V<module>
```

### Pass Criteria

- Exit code 0
- No `FAIL` lines in output
- `ALL TESTS PASSED` in output

`SKIP` lines (X-valued cycles) do not affect pass/fail.

---

## Validation API in `copper-sim`

The `HardwareTest` type in `copper-sim` orchestrates Phase F:

```rust
pub struct HardwareTest {
    pub name: String,
    verilog_path: Option<PathBuf>,
    waveform_path: Option<PathBuf>,
    trace: SimulationTrace,
}

impl HardwareTest {
    pub fn new(name: &str) -> Self;
    pub fn with_verilog(mut self, path: &str) -> Self;
    pub fn with_waveform(mut self, path: &str) -> Self;

    // Record one cycle of simulation
    pub fn record_cycle(
        &mut self,
        cycle: usize,
        inputs: &[(&str, &[Logic])],
        outputs: &[(&str, &[Logic])],
    );

    // Finish and run validation if verilog_path is set
    pub fn finish(&self) -> TestResult;

    // Finish and compare against an explicit expected trace
    pub fn finish_with_expected(&self, expected: &SimulationTrace) -> TestResult;
}

pub struct TestResult {
    pub passed: bool,
    pub cycle_results: Vec<CycleResult>,
    pub lint_output: Option<String>,
}

impl TestResult {
    pub fn assert_passed(&self);  // panics with details if !passed
}
```

### `finish` vs `finish_with_expected`

- `finish`: compares the recorded Copper simulation trace against the Verilog simulation. The recorded trace is the ground truth.
- `finish_with_expected`: compares the Verilog simulation against an explicitly constructed expected trace. Used when the test wants to assert specific values rather than just "Rust and Verilog agree".

---

## VCD Waveform Output

When `with_waveform` is set, Phase F generates a VCD file alongside the testbench run. The VCD file records all signal transitions for every cycle in the trace and can be viewed in GTKWave or similar.

VCD format:
```
$timescale 1ns $end
$var wire 8 # count $end
$var wire 1 # clk $end
...
#0
0#    // clk = 0
...
```

The VCD output is independent of pass/fail — it is always generated when configured, even if the simulation fails.

---

## Regression Test Targets

The initial regression set for Milestone 1 (from `TRANSPILATION_PLAN.md`):

| Module | Type | Key behaviors tested |
|---|---|---|
| `inverter` | Combinational | X-value passthrough, basic inversion |
| `mux` | Combinational | Select logic, all input combinations |
| `counter` | Sequential, single-tick | Register update, emit-before-tick pattern |
| `jk_ff` | Sequential, single-tick | Tuple match, toggle/set/reset/hold cases |
| `registered_pipeline` | Sequential, single-tick | Multi-register, 2-stage latency |
| `mux_4to1` | Combinational | Wide match, 4-input select |

Future targets (Milestone 2+):
- `mealy_fsm` — state machine with combinational output
- `uart_fsm` — multi-state FSM
- `fifo` — memory-backed module
- Multi-tick module when implemented

---

## Phase F Validation Report Format

When validation runs, it prints a structured report:

```
=== Validation: counter ===
Lint:    PASS (0 errors, 0 warnings)
Sim:     PASS (8/8 cycles passed, 0 skipped)
Overall: PASS

=== Validation: jk_ff ===
Lint:    PASS (0 errors, 0 warnings)
Sim:     FAIL
  FAIL cycle 3: out expected 1 got 0
  PASS cycle 4
  ...
Overall: FAIL
```

---

## Phase F Contract

**Inputs:**
- Emitted Verilog text from Phase E
- `SimulationTrace` from the Copper simulation harness
- `ToolchainProfile` for lint configuration

**Outputs:**
- `TestResult` with pass/fail per cycle, lint output, and overall status

**Invariants:**
1. A `PASS` from Phase F means: for every recorded simulation cycle, the Verilog simulation produced the same output as the Copper simulation (excluding X-valued cycles)
2. A `FAIL` from Phase F is always a transpiler bug (per Decision 2 in `TRANSPILATION_PLAN.md`) unless the trace itself is wrong
3. `Logic::X` cycles are always skipped, never failed
4. Lint errors always fail Phase F regardless of simulation result
5. The testbench clock model matches `HardwareExecutor::tick_clock` exactly: inputs set → clk low eval → clk high eval → read outputs
