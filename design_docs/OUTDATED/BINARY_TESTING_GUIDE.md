# RV32I Binary Testing Infrastructure

## Overview

The RV32I CPU module now includes a comprehensive binary testing infrastructure that allows you to load and test actual RV32I binaries (ELF or raw binary format) in simulation. The infrastructure is designed to be reusable for other CPU modules as well.

## Architecture

### Components

1. **`binary_test_utils.rs`** - Core testing utilities module containing:
   - `RV32IProgram`: Represents a loaded program with support for:
     - ELF binary loading (with proper segment extraction)
     - Raw binary loading
     - Program metadata (entry point, source info)
   
   - `BinaryTestRunner`: Interface for loading and managing programs
   
   - `CpuTestConfig`: Configuration for test execution:
     - `max_cycles`: Maximum clock cycles before timeout (default: 10000)
     - `verbose`: Enable detailed logging during execution

2. **`rv32i_cpu.rs`** - Main CPU module integration:
   - `run_program_with_config()`: Execute programs with custom configuration
   - `run_binary_program()`: Execute RV32IProgram objects
   - `test_elf_binary()`: Public function to test ELF binaries
   - `test_raw_binary()`: Public function to test raw binaries

## Usage Examples

### Basic ELF Binary Testing

```rust
use binary_test_utils::{RV32IProgram, CpuTestConfig};

// Load an ELF binary with expected result verification
test_elf_binary(
    "path/to/program.elf",
    Some(42),  // Expected value in a0 register
    Some(CpuTestConfig::with_max_cycles(10000).verbose()),
)?;
```

### Raw Binary Testing

```rust
// Load a raw binary file
test_raw_binary(
    "path/to/program.bin",
    Some(expected_value),
    None,  // Use default config (10000 cycles, not verbose)
)?;
```

### Direct Program Execution

```rust
let program = RV32IProgram::from_elf("my_program.elf")?;
let config = CpuTestConfig::default();
let result = run_binary_program(&program, &config)?;
```

### Custom Configuration

```rust
// Verbose output, custom cycle limit
let config = CpuTestConfig::with_max_cycles(50000).verbose();

let result = test_elf_binary(
    "program.elf",
    Some(expected_value),
    Some(config),
)?;
```

## Supported Binary Formats

### ELF Binaries

The infrastructure supports:
- 32-bit ELF files (`ELFCLASS32`)
- Little-endian format
- LOAD segments (text section extraction)
- Entry point specification

The loader:
1. Validates ELF magic number and header
2. Extracts all LOAD segments
3. Converts segments to u32 instructions (little-endian)
4. Reads entry point for future use

### Raw Binary Files

Raw binary format requirements:
- File size must be a multiple of 4 bytes
- Instructions in little-endian format
- No metadata or header needed

## Using with Other CPU Modules

The infrastructure is designed to be reusable. To use it with another CPU module:

1. **Module Integration**:
   ```rust
   mod binary_test_utils;
   use binary_test_utils::{RV32IProgram, CpuTestConfig};
   ```

2. **Create Execution Wrapper**:
   ```rust
   fn run_binary_program(program: &RV32IProgram, config: &CpuTestConfig) -> BinaryTestResult<u32> {
       // Your CPU-specific execution logic
       // Use program.instructions and program.entry_point
       // Respect config.max_cycles and config.verbose
   }
   ```

3. **Create Test Functions** (following the pattern in `rv32i_cpu.rs`):
   ```rust
   pub fn test_elf_binary<P: AsRef<Path>>(
       elf_path: P,
       expected_result: Option<u32>,
       config: Option<CpuTestConfig>,
   ) -> BinaryTestResult<u32> {
       // Load and execute, with error handling
   }
   ```

## Error Handling

The infrastructure uses a standard `BinaryTestResult<T>` type that provides detailed error information:

```rust
pub enum BinaryTestError {
    IoError(std::io::Error),              // File I/O failures
    InvalidElfFormat(String),              // ELF parsing errors
    InvalidBinaryFormat(String),           // Raw binary format errors
    ExecutionTimeout(String),              // Program timeout
    ExecutionError(String),                // Execution or assertion failures
}
```

Example error handling:
```rust
match test_elf_binary("program.elf", Some(42), None) {
    Ok(result) => println!("Program result: {}", result),
    Err(e) => eprintln!("Test failed: {}", e),
}
```

## Configuration Options

### CpuTestConfig

```rust
// Default configuration
let config = CpuTestConfig::default();  // 10000 cycles, not verbose

// Custom cycle limit
let config = CpuTestConfig::with_max_cycles(50000);

// Enable verbose output
let config = CpuTestConfig::with_max_cycles(10000).verbose();

// Builder pattern
let config = CpuTestConfig {
    max_cycles: 20000,
    verbose: true,
};
```

When `verbose` is enabled, the module prints:
- Program summary (source, instruction count, entry point)
- Execution progress (halted cycle, final PC, a0 value)

## Example: Adding Tests for Your Binaries

Once you have your RV32I binaries, add them to the test suite:

```rust
#[test]
fn test_my_program() {
    let result = test_elf_binary(
        "test_binaries/my_program.elf",
        Some(expected_a0_value),
        Some(CpuTestConfig::with_max_cycles(10000).verbose()),
    ).expect("Binary test failed");
    
    println!("my_program returned: {}", result);
}

#[test]
fn test_another_program() {
    match test_raw_binary(
        "test_binaries/another.bin",
        Some(42),
        None,
    ) {
        Ok(result) => assert_eq!(result, 42),
        Err(e) => panic!("Test failed: {}", e),
    }
}
```

## Testing Infrastructure Example

The module includes `test_binary_infrastructure()` which demonstrates:
1. Creating a program programmatically
2. Using the RV32IProgram API
3. Running through the binary test interface
4. Verifying results

Run it with:
```bash
cargo run --example rv32i_cpu
```

## Performance Considerations

- **Cycle Budgeting**: Each instruction takes a variable number of cycles depending on memory latency
  - Most ALU operations: 2-3 cycles
  - Load operations: 4-5 cycles (with READ_LAT=2)
  - Store operations: 2 cycles (with WRITE_LAT=1)
  
- **Max Cycles Setting**: Set `max_cycles` based on your program's expected behavior
  - Simple arithmetic: 100-500 cycles
  - Loops with iterations: 1000-10000 cycles
  - Complex programs: 50000+ cycles

## Future Enhancements

Possible extensions to the infrastructure:

1. **Instruction Disassembly**: Add Verilog/Verilator output for verification
2. **Memory Snapshot**: Capture memory state at program end
3. **Cycle Profiling**: Track instruction execution times
4. **Breakpoints**: Support debugging breakpoints in simulation
5. **Multiple Register Results**: Return multiple register values (not just a0)
6. **Test Batching**: Run multiple tests efficiently

## Troubleshooting

### "Program did not halt within X cycles"
- Increase `max_cycles` in config
- Check if program has an infinite loop or missing ECALL
- Enable `verbose` mode to see execution progress

### "Invalid ELF format"
- Ensure binary is compiled for RV32I
- Check that it's little-endian (not big-endian)
- Verify the file is a valid ELF file

### "Binary file size must be a multiple of 4 bytes"
- Raw binaries must have size divisible by 4
- Verify binary wasn't corrupted or truncated

## Integration Checklist

When adding binary tests to a new CPU module:

- [ ] Import `binary_test_utils` module
- [ ] Import `RV32IProgram`, `CpuTestConfig`, `BinaryTestResult`
- [ ] Create `run_binary_program()` wrapper for your CPU
- [ ] Create `test_elf_binary()` and/or `test_raw_binary()` functions
- [ ] Add test functions to your test suite
- [ ] Place binary files in appropriate directory
- [ ] Document expected cycle counts for your programs
- [ ] Add verbose mode for debugging new programs
