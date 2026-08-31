# Copper HDL

A fundamentally safer hardware description language embedded in Rust that eliminates entire classes of bugs through ownership and type system guarantees.

## Design Goals

Three principles guide every design decision in Copper:

1. **The simulation and the hardware are the same program.** Not "the simulator approximates the hardware" — the *same source* compiles to a cycle-accurate Rust simulation and to synthesizable Verilog. If the simulation is wrong, the hardware is wrong, and vice versa. This is a correctness guarantee, not a convenience feature.

2. **Illegal hardware states should be inexpressible, not just detectable.** A type error is better than a runtime assertion, and a runtime assertion is better than a silent bug. Copper's type system and ownership model make entire classes of hardware bugs physically impossible to write, not merely flagged at simulation time.

3. **Abstraction shouldn't cost you hardware quality.** High-level constructs (async/await FSMs, phantom-type clock domains, const-generic bit widths) must compile to the same hardware as hand-written Verilog. Zero-cost means zero gap between what you write and what you get.

**Type convention:** Use `Logic` for single-bit hardware signals, `Bits<N>` for width-bearing hardware values, `bool` only for simple two-state control where `X` does not matter, and Rust primitives (`u32`, `i32`, `usize`, etc.) for host-side or testbench-only data. If a value crosses into hardware, prefer `Logic` or `Bits<N>`.

**Anti-goals:** Copper is not trying to be "Verilog with better syntax" — that would just be a more comfortable way to write the same bugs. And "runs fast" is not a primary goal: correctness and safety come first.

## What Makes Copper Different?

Traditional HDLs like Verilog and VHDL were designed in the 1980s before modern type theory and programming language advances. Copper leverages Rust's unique features to prevent common hardware design mistakes at compile time:

- **Typed Clock Domains (CDC safety)**: clock domains are phantom types on every signal and clock, so an unsynchronized cross-domain connection is a *compile error*. Regular modules are single-domain by construction; every domain crossing must go through an explicit synchronizer (`sync_2ff`, a `#[hardware(synchronizer)]` module), so a crossing can never be hidden. This localizes and makes every crossing explicit — it does not verify a synchronizer's internal timing.
- **Async/Await State Machines**: Write FSMs naturally with async/await—no manual state enumeration
- **Function-Typed Modules**: Ports inferred from function signatures—no explicit port declarations
- **Type-Driven Hardware**: `Logic`/`Bits<N>` for hardware values, phantom types for clock domains, const generics for bit widths
- **Unified Simulation/Synthesis**: Same code runs in cycle-accurate Rust simulator and compiles to Verilog

## Quick Start

### Prerequisites

- Rust (latest stable)
- Verilator (for Verilog verification)

### Build and Run Examples

```bash
# Build all examples
cargo build --examples

# Run a simple counter example
cargo run --example simple_counter

# Run the Verilog pitfall showcase
cargo run --example verilog_pitfalls
```

### Run Tests

```bash
# Run full test suite
cargo test --all
```

## Verilog Pitfall Showcases

Want to see concrete examples of bugs Copper prevents? We have three comprehensive showcases:

### 1. Basic Pitfalls ([examples/verilog_pitfalls.rs](examples/verilog_pitfalls.rs))
- **Latch Inference** - Incomplete case statements that create unwanted latches
- **Implicit Net Declaration** - Typos that silently create new wires
- **Multiple Driver Races** - Multiple always blocks writing to the same register

### 2. Simulation Hazards ([examples/simulation_hazards.rs](examples/simulation_hazards.rs))
With detailed cycle-by-cycle execution traces:
- **Blocking/Non-Blocking Races** - Mixed assignment types creating simulator-dependent behavior
- **Multiple Assignments** - Race conditions from multiple procedural blocks
- **Read-During-Write** - Scheduler-dependent signal values
- **Missing `default_nettype none** - Typos creating implicit wires

### 3. Security Vulnerabilities ([examples/security_showcase.rs](examples/security_showcase.rs))
Hardware security bugs that Copper prevents:
- **Timing Side-Channels** - Incomplete coverage leaking secret information
- **Uninitialized Registers** - Security-critical state with undefined initial values
- **Illegal FSM States** - Unhandled states vulnerable to fault injection
- **Information Leakage** - Outputs retaining secret data from previous operations
- **Privilege Escalation** - Hidden global state bypassing security checks

Run any showcase:
```bash
cargo run --example verilog_pitfalls
cargo run --example simulation_hazards
cargo run --example security_showcase
```

See the corresponding buggy Verilog modules in `verilog/bug_*.v` for comparison.

## Examples

Copper includes various examples demonstrating different features:

### Basic Examples
- [inverter.rs](examples/inverter.rs) - Simple combinational logic
- [simple_counter.rs](examples/simple_counter.rs) - Basic sequential logic with async
- [async_counter.rs](examples/async_counter.rs) - Counter with async/await state machine
- [mux.rs](examples/mux.rs) - Multiplexer with pattern matching

### State Machine Examples
- [mealy.rs](examples/mealy.rs) - Mealy FSM pattern detector
- [uart_fsm.rs](examples/uart_fsm.rs) - UART receiver state machine

### Pipeline Examples
- [pipeline.rs](examples/pipeline.rs) - Multi-stage pipeline
- [pipeline_stall.rs](examples/pipeline_stall.rs) - Pipeline with valid/stall signals
- [hierarchical_pipeline.rs](examples/hierarchical_pipeline.rs) - Hierarchical module composition

### Complex Examples
- [alu.rs](examples/alu.rs) - Arithmetic Logic Unit with function-typed outputs
- [ram_rom.rs](examples/ram_rom.rs) - Memory models
- [independent_counters.rs](examples/independent_counters.rs) - Multiple concurrent modules

## Architecture

Copper is organized into several crates:

- **copper-core**: Core type system (`Logic`, `Bits<N>`, `Clock<Domain>`)
- **copper-sim**: Cycle-accurate simulation runtime and executor
- **copper-macros**: Procedural macros (`#[hardware]`, etc.)
- **copper-codegen**: Verilog code generation backend

## Project Status

Copper is in active development for academic publication (targeting PLDI 2027). Current focus:

- ✅ Core type system with 4-state logic
- ✅ Async/await executor with lockstep simulation
- ✅ Function-typed modules with implicit outputs
- ✅ Comprehensive examples and test suite
- 🚧 Verilog code generation — full `FIR → CHIR → SHIR → VLIR → SystemVerilog`
  pipeline landed; `copper-transpile` CLI emits Verilator-lint-clean output.
  Two examples (`counter`, `lfsr`) are verified behaviorally equivalent to the
  Copper simulation under Verilator. Widening front-end feature coverage (const
  generics, arrays, for-loops, LHS bit-assignment) is ongoing — see
  [TRANSPILATION_ROADMAP.md](TRANSPILATION_ROADMAP.md).
- ⏳ Clock domain crossing safety verification
- ⏳ Formal semantics and correctness proofs

See [TRANSPILATION_ROADMAP.md](TRANSPILATION_ROADMAP.md) for the current
transpilation status, progress log, and notes; [PROGRESS.md](PROGRESS.md) for
detailed development tracking.

## Documentation

### Transpilation Pipeline

- [TRANSPILATION_ROADMAP.md](TRANSPILATION_ROADMAP.md) - **Current status, progress log, decisions, and notes (canonical)**
- [TRANSPILATION_COVERAGE_MAP.md](TRANSPILATION_COVERAGE_MAP.md) - Which examples force which features, and the dependency-ordered order of attack toward the RV32I capstone
- [TRANSPILATION_PLAN.md](TRANSPILATION_PLAN.md) - Original pipeline architecture and decision log (⚠️ predates the In/Out port model)
- [ASYNC_AWAIT_SEMANTICS.md](ASYNC_AWAIT_SEMANTICS.md) - Clock/tick/emit runtime semantics (canonical reference)
- [FIR_DESIGN.md](FIR_DESIGN.md) - Phase A: Frontend IR capture from Rust AST ✅
- [CHIR_DESIGN.md](CHIR_DESIGN.md) - Phase B: Canonical Hardware IR semantic lowering ✅
- [SHIR_DESIGN.md](SHIR_DESIGN.md) - Phase C: Scheduled Hardware IR timing and state construction ⏳
- [VLIR_DESIGN.md](VLIR_DESIGN.md) - Phase D: Verilog-Legal IR legalization (designed, not yet implemented)
- [EMISSION_DESIGN.md](EMISSION_DESIGN.md) - Phase E: Verilog text emission (designed, not yet implemented)
- [VALIDATION_DESIGN.md](VALIDATION_DESIGN.md) - Phase F: Equivalence validation strategy
- [VERILOG_OUTPUT_STANDARDS.md](VERILOG_OUTPUT_STANDARDS.md) - Verilog/SystemVerilog output standards and PR review checklist

### Runtime and Features

- [MEMORY_DESIGN.md](MEMORY_DESIGN.md) - Memory as a first-class hardware construct (plan; simulation side complete)
- [MODULE_COMPOSITION.md](MODULE_COMPOSITION.md) - Module composition patterns
- [LATENCY_INSENSITIVE_DESIGN.md](LATENCY_INSENSITIVE_DESIGN.md) - Latency-insensitive memory interface design
- [RV32I_CPU_DESIGN.md](RV32I_CPU_DESIGN.md) - RV32I CPU example design

### Testing and Verification

- [PROGRESS.md](PROGRESS.md) - Detailed development progress and roadmap
- [VERILATOR_VERIFICATION.md](VERILATOR_VERIFICATION.md) - Verilator integration guide
- [BINARY_TESTING_GUIDE.md](BINARY_TESTING_GUIDE.md) - Binary testing guide
- [TEST_RESULTS.md](TEST_RESULTS.md) - Test results snapshot

## Contributing

This is currently a research project. Feedback and suggestions welcome!

## License

TBD (will be open source upon publication)

## Citation

```bibtex
@inproceedings{copper-hdl-2027,
  title={Copper: A Type-Safe Hardware Description Language with Ownership-Based CDC Safety},
  author={TBD},
  booktitle={PLDI 2027},
  year={2027}
}
```
