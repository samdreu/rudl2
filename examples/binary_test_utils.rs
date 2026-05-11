/// Binary testing utilities for RV32I and other CPU modules.
///
/// This module provides reusable infrastructure for loading and executing
/// RV32I binaries in simulation, supporting both ELF binaries and raw binary files.
///
/// ## Quick Start
///
/// ### Loading and running an ELF binary:
/// ```ignore
/// let program = RV32IProgram::from_elf("path/to/binary.elf")?;
/// let config = CpuTestConfig::default();
/// let result = run_binary_program(&program, &config)?;
/// println!("Program returned: {}", result);
/// ```
///
/// ### Loading a raw binary:
/// ```ignore
/// let program = RV32IProgram::from_raw("path/to/binary.bin")?;
/// let config = CpuTestConfig::with_max_cycles(5000).verbose();
/// let result = run_binary_program(&program, &config)?;
/// ```
///
/// ### Testing with verification:
/// ```ignore
/// let result = test_elf_binary(
///     "program.elf",
///     Some(42),  // Expected a0 value
///     Some(CpuTestConfig::with_max_cycles(10000)),
/// )?;
/// ```
///
/// ## Supported Formats
///
/// - **ELF**: 32-bit, little-endian ELF files with LOAD segments
/// - **Raw Binary**: Files with size multiple of 4 bytes, little-endian instructions
///
/// ## Integration with Other CPU Modules
///
/// To use this infrastructure with a new CPU module:
///
/// 1. Import this module
/// 2. Create a `run_binary_program()` wrapper that executes your CPU with a program
/// 3. Call `test_elf_binary()` or `test_raw_binary()` from your tests
///
/// See BINARY_TESTING_GUIDE.md for detailed integration instructions.

use std::fs;
use std::path::Path;

/// Result type for binary testing operations
pub type BinaryTestResult<T> = Result<T, BinaryTestError>;

/// Error types for binary testing operations
#[derive(Debug)]
pub enum BinaryTestError {
    IoError(std::io::Error),
    InvalidElfFormat(String),
    InvalidBinaryFormat(String),
    ExecutionTimeout(String),
    ExecutionError(String),
}

impl From<std::io::Error> for BinaryTestError {
    fn from(err: std::io::Error) -> Self {
        BinaryTestError::IoError(err)
    }
}

impl std::fmt::Display for BinaryTestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryTestError::IoError(e) => write!(f, "IO error: {}", e),
            BinaryTestError::InvalidElfFormat(msg) => write!(f, "Invalid ELF format: {}", msg),
            BinaryTestError::InvalidBinaryFormat(msg) => write!(f, "Invalid binary format: {}", msg),
            BinaryTestError::ExecutionTimeout(msg) => write!(f, "Execution timeout: {}", msg),
            BinaryTestError::ExecutionError(msg) => write!(f, "Execution error: {}", msg),
        }
    }
}

impl std::error::Error for BinaryTestError {}

/// Represents a loaded RV32I program ready for execution
#[derive(Debug, Clone)]
pub struct RV32IProgram {
    /// Program instructions as u32 words
    pub instructions: Vec<u32>,
    /// Entry point (default 0)
    pub entry_point: u32,
    /// Original source for debugging
    pub source: String,
}

impl RV32IProgram {
    /// Create a new program from instructions
    pub fn new(instructions: Vec<u32>) -> Self {
        RV32IProgram {
            instructions,
            entry_point: 0,
            source: "manual".to_string(),
        }
    }

    /// Create a program from an ELF file
    pub fn from_elf<P: AsRef<Path>>(path: P) -> BinaryTestResult<Self> {
        let path = path.as_ref();
        let data = fs::read(path)?;

        // Basic ELF header validation
        if data.len() < 52 {
            return Err(BinaryTestError::InvalidElfFormat(
                "File too small for ELF header".to_string(),
            ));
        }

        // Check ELF magic number
        if &data[0..4] != b"\x7FELF" {
            return Err(BinaryTestError::InvalidElfFormat(
                "Missing ELF magic number".to_string(),
            ));
        }

        // Check for 32-bit architecture
        let ei_class = data[4];
        if ei_class != 1 {
            return Err(BinaryTestError::InvalidElfFormat(
                "Expected 32-bit ELF file (ELFCLASS32)".to_string(),
            ));
        }

        // Check endianness (1 = little-endian, 2 = big-endian)
        let ei_data = data[5];
        if ei_data != 1 {
            return Err(BinaryTestError::InvalidElfFormat(
                "Expected little-endian ELF file".to_string(),
            ));
        }

        // Read entry point (offset 0x18, 4 bytes, little-endian)
        let entry_point = u32::from_le_bytes([data[0x18], data[0x19], data[0x1A], data[0x1B]]);

        // Read program header offset (offset 0x1C, 4 bytes, little-endian)
        let phoff = u32::from_le_bytes([data[0x1C], data[0x1D], data[0x1E], data[0x1F]]) as usize;

        // Read number of program headers (offset 0x2C, 2 bytes, little-endian)
        let phnum = u16::from_le_bytes([data[0x2C], data[0x2D]]) as usize;

        // Program header size (offset 0x2A, 2 bytes, little-endian)
        let phentsize = u16::from_le_bytes([data[0x2A], data[0x2B]]) as usize;

        // Find the LOAD segment containing the text
        let mut instructions = Vec::new();

        for i in 0..phnum {
            let ph_offset = phoff + i * phentsize;
            if ph_offset + 32 > data.len() {
                continue;
            }

            // Program header type (offset 0, 4 bytes)
            let p_type = u32::from_le_bytes([
                data[ph_offset],
                data[ph_offset + 1],
                data[ph_offset + 2],
                data[ph_offset + 3],
            ]);

            // PT_LOAD = 1
            if p_type != 1 {
                continue;
            }

            // p_offset: file offset of segment (offset 4)
            let p_offset = u32::from_le_bytes([
                data[ph_offset + 4],
                data[ph_offset + 5],
                data[ph_offset + 6],
                data[ph_offset + 7],
            ]) as usize;

            // p_filesz: size of segment in file (offset 16)
            let p_filesz = u32::from_le_bytes([
                data[ph_offset + 16],
                data[ph_offset + 17],
                data[ph_offset + 18],
                data[ph_offset + 19],
            ]) as usize;

            // Extract the segment and convert to u32 instructions
            if p_offset + p_filesz <= data.len() {
                let segment = &data[p_offset..p_offset + p_filesz];
                for chunk in segment.chunks_exact(4) {
                    let instr = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    instructions.push(instr);
                }
            }
        }

        if instructions.is_empty() {
            return Err(BinaryTestError::InvalidElfFormat(
                "No LOAD segments found or segments are empty".to_string(),
            ));
        }

        Ok(RV32IProgram {
            instructions,
            entry_point,
            source: path.to_string_lossy().to_string(),
        })
    }

    /// Create a program from a raw binary file
    pub fn from_raw<P: AsRef<Path>>(path: P) -> BinaryTestResult<Self> {
        let path = path.as_ref();
        let data = fs::read(path)?;

        if data.is_empty() {
            return Err(BinaryTestError::InvalidBinaryFormat(
                "Binary file is empty".to_string(),
            ));
        }

        if data.len() % 4 != 0 {
            return Err(BinaryTestError::InvalidBinaryFormat(
                "Binary file size must be a multiple of 4 bytes".to_string(),
            ));
        }

        let instructions: Vec<u32> = data
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();

        Ok(RV32IProgram {
            instructions,
            entry_point: 0,
            source: path.to_string_lossy().to_string(),
        })
    }

    /// Get the program as a string for debugging
    pub fn disassemble_summary(&self) -> String {
        format!(
            "Program from: {}\nInstructions: {} words\nEntry point: 0x{:08x}",
            self.source,
            self.instructions.len(),
            self.entry_point
        )
    }
}

/// Test runner for RV32I programs
/// 
/// This struct provides a unified interface for running CPU modules
/// with different programs, making it easy to test multiple modules
/// with the same testing infrastructure.
pub struct BinaryTestRunner {
    program: RV32IProgram,
}

impl BinaryTestRunner {
    /// Create a new test runner with a program
    pub fn new(program: RV32IProgram) -> Self {
        BinaryTestRunner { program }
    }

    /// Load an ELF binary file
    pub fn load_elf<P: AsRef<Path>>(path: P) -> BinaryTestResult<Self> {
        let program = RV32IProgram::from_elf(path)?;
        Ok(BinaryTestRunner { program })
    }

    /// Load a raw binary file
    pub fn load_raw<P: AsRef<Path>>(path: P) -> BinaryTestResult<Self> {
        let program = RV32IProgram::from_raw(path)?;
        Ok(BinaryTestRunner { program })
    }

    /// Get reference to the underlying program
    pub fn program(&self) -> &RV32IProgram {
        &self.program
    }

    /// Get mutable reference to the underlying program
    pub fn program_mut(&mut self) -> &mut RV32IProgram {
        &mut self.program
    }

    /// Print program summary for debugging
    pub fn print_summary(&self) {
        println!("{}", self.program.disassemble_summary());
        println!(
            "First 10 instructions (or all if fewer):",
        );
        for (i, instr) in self.program.instructions.iter().take(10).enumerate() {
            println!("  [{}]: 0x{:08x}", i, instr);
        }
        if self.program.instructions.len() > 10 {
            println!("  ... and {} more instructions", self.program.instructions.len() - 10);
        }
    }
}

/// Configuration for CPU test execution
#[derive(Debug, Clone)]
pub struct CpuTestConfig {
    /// Maximum number of clock cycles to run before timeout
    pub max_cycles: usize,
    /// Enable detailed logging during execution
    pub verbose: bool,
}

impl Default for CpuTestConfig {
    fn default() -> Self {
        CpuTestConfig {
            max_cycles: 10000,
            verbose: false,
        }
    }
}

impl CpuTestConfig {
    /// Create a new config with specified max cycles
    pub fn with_max_cycles(max_cycles: usize) -> Self {
        CpuTestConfig {
            max_cycles,
            verbose: false,
        }
    }

    /// Enable verbose logging
    pub fn verbose(mut self) -> Self {
        self.verbose = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_program_creation() {
        let instructions = vec![0x00000013, 0x00100113, 0x00200193];
        let program = RV32IProgram::new(instructions.clone());
        assert_eq!(program.instructions, instructions);
        assert_eq!(program.entry_point, 0);
    }

    #[test]
    fn test_cpu_test_config_default() {
        let config = CpuTestConfig::default();
        assert_eq!(config.max_cycles, 10000);
        assert!(!config.verbose);
    }

    #[test]
    fn test_cpu_test_config_builder() {
        let config = CpuTestConfig::with_max_cycles(5000).verbose();
        assert_eq!(config.max_cycles, 5000);
        assert!(config.verbose);
    }
}
