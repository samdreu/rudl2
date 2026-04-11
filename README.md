# Copper HDL

A fundamentally safer hardware description language embedded in Rust that eliminates entire classes of bugs through ownership and type system guarantees.

## What Makes Copper Different?

Traditional HDLs like Verilog and VHDL were designed in the 1980s before modern type theory and programming language advances. Copper leverages Rust's unique features to prevent common hardware design mistakes at compile time:

- **Ownership-Based CDC Safety**: First HDL to use ownership semantics for compile-time clock domain crossing verification
- **Async/Await State Machines**: Write FSMs naturally with async/await—no manual state enumeration
- **Function-Typed Modules**: Ports inferred from function signatures—no explicit port declarations
- **Type-Driven Hardware**: Phantom types for clock domains, const generics for bit widths
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

- **copper-core**: Core type system (`Bit`, `Bits<N>`, `Clock<Domain>`)
- **copper-sim**: Cycle-accurate simulation runtime and executor
- **copper-macros**: Procedural macros (`#[hardware]`, etc.)
- **copper-codegen**: Verilog code generation backend

## Project Status

Copper is in active development for academic publication (targeting PLDI 2027). Current focus:

- ✅ Core type system with 4-state logic
- ✅ Async/await executor with lockstep simulation
- ✅ Function-typed modules with implicit outputs
- ✅ Comprehensive examples and test suite
- ⏳ Verilog code generation (in progress)
- ⏳ Clock domain crossing safety verification
- ⏳ Formal semantics and correctness proofs

See [PROGRESS.md](PROGRESS.md) for detailed development tracking.

## Documentation

- [PROGRESS.md](PROGRESS.md) - Detailed development progress and roadmap
- [MODULE_COMPOSITION.md](MODULE_COMPOSITION.md) - Module composition patterns
- [VERILATOR_VERIFICATION.md](VERILATOR_VERIFICATION.md) - Verilator integration guide
- [VERILOG_OUTPUT_STANDARDS.md](VERILOG_OUTPUT_STANDARDS.md) - Verilog/SystemVerilog output standards and PR review checklist

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
