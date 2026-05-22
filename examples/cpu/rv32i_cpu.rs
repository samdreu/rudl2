// Non-pipelined RV32I CPU using Copper async/await semantics.
//
// Architecture Design (inspired by ECE437 MIPS CPU):
// ════════════════════════════════════════════════════════════════════════════════
// This CPU uses a modular, interface-based architecture with separate concerns:
//
// 1. **Dynamic Latency Detection via is_ready() Polling**
//    - Issues read/write requests to memory ports
//    - Polls `is_ready()` on read ports to detect when data is available
//    - Automatically adapts to any Memory latency configuration
//
// 2. **Modular Components** (following ECE437 design patterns)
//    - types: RV32I instruction types, opcodes, ALU operations
//    - decoder: Instruction decoding (fields extraction)
//    - alu: ALU operations with overflow/zero/negative flags
//    - regfile: Register file abstraction
//    - memory: Memory access abstraction
//
// 3. **Clean Pipeline Stages**
//    - IF: Instruction fetch from IMEM (with is_ready polling)
//    - ID: Instruction decode and register reads
//    - EX: Execute ALU operations
//    - MEM: Load/store from DMEM
//    - WB: Write results to register file
//
// Output type: (committed_pc, halted, a0)
//   halted=false during normal execution, true after ECALL.

use copper_core::{Clock, ClockDomain, Memory};
use copper_macros::hardware;
use copper_sim::{emit, HardwareExecutor};

mod binary_test_utils;
use binary_test_utils::{CpuTestConfig, RV32IProgram, BinaryTestResult};

struct MainClk;
impl ClockDomain for MainClk {}

// ── RV32I Types (inspired by cpu_types_pkg.vh from ECE437) ──────────────────────

mod rv32i_types {
    /// RV32I Opcodes (7-bit primary opcode)
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Opcode {
        LUI    = 0x37,
        AUIPC  = 0x17,
        JAL    = 0x6F,
        JALR   = 0x67,
        BRANCH = 0x63,
        LOAD   = 0x03,
        STORE  = 0x23,
        ALU_IMM = 0x13,
        ALU_REG = 0x33,
        ECALL  = 0x73,
    }

    impl Opcode {
        pub fn from_u32(op: u32) -> Option<Self> {
            match op {
                0x37 => Some(Opcode::LUI),
                0x17 => Some(Opcode::AUIPC),
                0x6F => Some(Opcode::JAL),
                0x67 => Some(Opcode::JALR),
                0x63 => Some(Opcode::BRANCH),
                0x03 => Some(Opcode::LOAD),
                0x23 => Some(Opcode::STORE),
                0x13 => Some(Opcode::ALU_IMM),
                0x33 => Some(Opcode::ALU_REG),
                0x73 => Some(Opcode::ECALL),
                _ => None,
            }
        }
    }

    /// ALU Operations (RV32I func3 and func7 encoding)
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum AluOp {
        Add,  Addi,
        Sub,
        And,  Andi,
        Or,   Ori,
        Xor,  Xori,
        Sll,  Slli,
        Srl,  Srli,
        Sra,  Srai,
        Slt,  Slti,
        Sltu, Sltiu,
    }

    /// Decoded instruction fields (following ECE437 patterns)
    #[derive(Debug, Clone, Copy)]
    pub struct InstrDecoded {
        pub opcode: Opcode,
        pub rd: usize,
        pub rs1: usize,
        pub rs2: usize,
        pub f3: u32,   // funct3
        pub f7: u32,   // funct7
        pub imm_i: i32,  // I-type immediate
        pub imm_s: i32,  // S-type immediate
        pub imm_b: i32,  // B-type immediate
        pub imm_j: i32,  // J-type immediate
        pub imm_u: u32,  // U-type immediate
    }

    /// Branch condition types
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum BranchCond {
        Beq,   // Equal
        Bne,   // Not Equal
        Blt,   // Less Than (signed)
        Bge,   // Greater or Equal (signed)
        Bltu,  // Less Than Unsigned
        Bgeu,  // Greater or Equal Unsigned
    }

    impl BranchCond {
        pub fn from_f3(f3: u32) -> Option<Self> {
            match f3 {
                0x0 => Some(BranchCond::Beq),
                0x1 => Some(BranchCond::Bne),
                0x4 => Some(BranchCond::Blt),
                0x5 => Some(BranchCond::Bge),
                0x6 => Some(BranchCond::Bltu),
                0x7 => Some(BranchCond::Bgeu),
                _ => None,
            }
        }
    }

    /// ALU output with status flags (following ECE437 alu_if pattern)
    #[derive(Debug, Clone, Copy)]
    pub struct AluOutput {
        pub result: u32,
        pub overflow: bool,
        pub zero: bool,
        pub negative: bool,
    }
}

// ── RV32I Instruction Decoder (inspired by ECE437 modular approach) ──────────────

mod rv32i_decoder {
    use super::rv32i_types::*;

    /// Sign-extend I-type immediate (12 bits → 32 bits)
    pub fn sign_ext_i(instr: u32) -> i32 {
        (instr as i32) >> 20
    }

    /// Sign-extend S-type immediate (12 bits spread → 32 bits)
    pub fn sign_ext_s(instr: u32) -> i32 {
        let hi7 = (instr >> 25) & 0x7F;
        let lo5 = (instr >> 7) & 0x1F;
        (((hi7 << 5) | lo5) as i32) << 20 >> 20
    }

    /// Sign-extend B-type immediate (12 bits spread → 32 bits)
    pub fn sign_ext_b(instr: u32) -> i32 {
        let b12   = (instr >> 31) & 1;
        let b11   = (instr >> 7)  & 1;
        let b10_5 = (instr >> 25) & 0x3F;
        let b4_1  = (instr >> 8)  & 0xF;
        let raw   = (b12 << 12) | (b11 << 11) | (b10_5 << 5) | (b4_1 << 1);
        ((raw as i32) << 19) >> 19
    }

    /// Sign-extend J-type immediate (20 bits spread → 32 bits)
    pub fn sign_ext_j(instr: u32) -> i32 {
        let b20    = (instr >> 31) & 1;
        let b10_1  = (instr >> 21) & 0x3FF;
        let b11    = (instr >> 20) & 1;
        let b19_12 = (instr >> 12) & 0xFF;
        let raw    = (b20 << 20) | (b19_12 << 12) | (b11 << 11) | (b10_1 << 1);
        ((raw as i32) << 11) >> 11
    }

    /// Decode instruction into fields (like cpu_types in ECE437)
    pub fn decode(instr: u32) -> Option<InstrDecoded> {
        let opcode_bits = instr & 0x7F;
        let opcode = Opcode::from_u32(opcode_bits)?;

        Some(InstrDecoded {
            opcode,
            rd: ((instr >> 7) & 0x1F) as usize,
            rs1: ((instr >> 15) & 0x1F) as usize,
            rs2: ((instr >> 20) & 0x1F) as usize,
            f3: (instr >> 12) & 0x7,
            f7: (instr >> 25) & 0x7F,
            imm_i: sign_ext_i(instr),
            imm_s: sign_ext_s(instr),
            imm_b: sign_ext_b(instr),
            imm_j: sign_ext_j(instr),
            imm_u: instr & 0xFFFF_F000,
        })
    }
}

// ── RV32I ALU (inspired by alu.sv from ECE437) ───────────────────────────────────

mod rv32i_alu {
    use super::rv32i_types::AluOutput;

    /// Execute ALU operation on two operands (like always_comb in alu.sv)
    pub fn execute_reg(a: u32, b: u32, f3: u32, f7: u32) -> AluOutput {
        let result = match (f3, f7) {
            (0x0, 0x00) => a.wrapping_add(b),        // ADD
            (0x0, 0x20) => a.wrapping_sub(b),        // SUB
            (0x1, _)    => a << (b & 0x1F),          // SLL
            (0x2, _)    => ((a as i32) < (b as i32)) as u32,  // SLT (signed)
            (0x3, _)    => (a < b) as u32,           // SLTU (unsigned)
            (0x4, _)    => a ^ b,                    // XOR
            (0x5, 0x00) => a >> (b & 0x1F),          // SRL (logical)
            (0x5, 0x20) => ((a as i32) >> (b & 0x1F)) as u32, // SRA (arithmetic)
            (0x6, _)    => a | b,                    // OR
            (0x7, _)    => a & b,                    // AND
            _ => 0,
        };

        let overflow = match (f3, f7) {
            (0x0, 0x00) => {
                // ADD overflow: same sign operands, different sign result
                (a >> 31 == b >> 31) && (a >> 31 != result >> 31)
            }
            (0x0, 0x20) => {
                // SUB overflow: different sign operands, different sign in result from minuend
                (a >> 31 != b >> 31) && (b >> 31 == result >> 31)
            }
            _ => false,
        };

        AluOutput {
            result,
            overflow,
            zero: result == 0,
            negative: (result as i32) < 0,
        }
    }

    /// Execute immediate ALU operation (I-type)
    pub fn execute_imm(a: u32, imm: i32, f3: u32, f7: u32) -> AluOutput {
        let shamt = (imm as u32) & 0x1F;
        let result = match f3 {
            0x0 => a.wrapping_add(imm as u32),        // ADDI
            0x1 => a << shamt,                         // SLLI
            0x2 => ((a as i32) < imm) as u32,          // SLTI
            0x3 => (a < (imm as u32)) as u32,          // SLTIU
            0x4 => a ^ (imm as u32),                   // XORI
            0x5 => if f7 & 0x20 != 0 {
                ((a as i32) >> shamt) as u32           // SRAI
            } else {
                a >> shamt                             // SRLI
            },
            0x6 => a | (imm as u32),                   // ORI
            0x7 => a & (imm as u32),                   // ANDI
            _ => 0,
        };

        AluOutput {
            result,
            overflow: false,  // Immediates don't compute overflow in RV32I
            zero: result == 0,
            negative: (result as i32) < 0,
        }
    }
}

// ── Sign-extension helpers ────────────────────────────────────────────────────
// Re-export from decoder module for backward compatibility with assembler helpers

use rv32i_decoder::*;
use rv32i_types::Opcode;

fn sign_ext_i(instr: u32) -> i32 {
    rv32i_decoder::sign_ext_i(instr)
}

fn sign_ext_s(instr: u32) -> i32 {
    rv32i_decoder::sign_ext_s(instr)
}

fn sign_ext_b(instr: u32) -> i32 {
    rv32i_decoder::sign_ext_b(instr)
}

fn sign_ext_j(instr: u32) -> i32 {
    rv32i_decoder::sign_ext_j(instr)
}

// ── ALU ───────────────────────────────────────────────────────────────────────

fn alu_reg(a: u32, b: u32, f3: u32, f7: u32) -> u32 {
    rv32i_alu::execute_reg(a, b, f3, f7).result
}

fn alu_imm(a: u32, imm_i: i32, f3: u32, f7: u32) -> u32 {
    rv32i_alu::execute_imm(a, imm_i, f3, f7).result
}

// ── Wait for ready using is_ready() polling ───────────────────────────────────
// These helpers replace hardcoded cycle counting. The CPU polls is_ready() and
// adapts dynamically to the actual memory latency, matching real hardware behavior.

macro_rules! wait_for {
    ($clk:expr, $port:expr) => {{
        loop {
            $clk.tick().await;
            if $port.is_ready() {
                break;
            }
        }
    }};
}

// ── CPU ───────────────────────────────────────────────────────────────────────

// Output: (committed_pc, halted, a0_on_halt)
#[hardware(function_typed)]
async fn rv32i_cpu(clk: Clock<MainClk>, program: Vec<u32>) -> (u32, bool, u32) {
    // Memory instantiations with explicit latencies (configure these for different hardware)
    let imem    = Memory::<u32, 1, 0, MainClk, 2, 1>::from_contents(clk.clone(), program);
    let dmem    = Memory::<u32, 1, 1, MainClk, 2, 1>::new(clk.clone(), 1024);
    let regfile = Memory::<u32, 2, 1, MainClk, 1, 1>::new(clk.clone(), 32);

    let mut pc: u32 = 0;
    emit!((pc, false, 0u32));

    loop {
        // ── IF ───────────────────────────────────────────────────────────────
        // Fetch instruction from IMEM. Wait until the read is ready.
        imem.read_port::<0>().read((pc >> 2) as usize);
        wait_for!(clk, imem.read_port::<0>());
        let instr = imem.read_port::<0>().data();

        // ── Decode ──────────────────────────────────────────────────────────
        let opcode = instr & 0x7F;
        let rd     = ((instr >> 7)  & 0x1F) as usize;
        let f3     = (instr >> 12)  & 0x7;
        let rs1    = ((instr >> 15) & 0x1F) as usize;
        let rs2    = ((instr >> 20) & 0x1F) as usize;
        let f7     = (instr >> 25)  & 0x7F;
        let imm_i  = sign_ext_i(instr);
        let imm_s  = sign_ext_s(instr);
        let imm_b  = sign_ext_b(instr);
        let imm_j  = sign_ext_j(instr);
        let imm_u  = instr & 0xFFFF_F000;

        match opcode {
            // ── LUI ──────────────────────────────────────────────────────────
            0x37 => {
                if rd != 0 {
                    regfile.write_port::<0>().write(rd, imm_u);
                    wait_for!(clk, regfile.write_port::<0>());
                }
                emit!((pc, false, 0u32));
                pc = pc.wrapping_add(4);
            }

            // ── AUIPC ────────────────────────────────────────────────────────
            0x17 => {
                let result = pc.wrapping_add(imm_u);
                if rd != 0 {
                    regfile.write_port::<0>().write(rd, result);
                    wait_for!(clk, regfile.write_port::<0>());
                }
                emit!((pc, false, 0u32));
                pc = pc.wrapping_add(4);
            }

            // ── JAL ──────────────────────────────────────────────────────────
            0x6F => {
                let link = pc.wrapping_add(4);
                let next = (pc as i32).wrapping_add(imm_j) as u32;
                if rd != 0 {
                    regfile.write_port::<0>().write(rd, link);
                    wait_for!(clk, regfile.write_port::<0>());
                }
                emit!((pc, false, 0u32));
                pc = next;
            }

            // ── JALR ─────────────────────────────────────────────────────────
            0x67 => {
                regfile.read_port::<0>().read(rs1);
                wait_for!(clk, regfile.read_port::<0>());
                let rv1  = if rs1 == 0 { 0 } else { regfile.read_port::<0>().data() };
                let link = pc.wrapping_add(4);
                let next = ((rv1 as i32).wrapping_add(imm_i) & !1) as u32;
                if rd != 0 {
                    regfile.write_port::<0>().write(rd, link);
                    wait_for!(clk, regfile.write_port::<0>());
                }
                emit!((pc, false, 0u32));
                pc = next;
            }

            // ── BRANCH ───────────────────────────────────────────────────────
            0x63 => {
                regfile.read_port::<0>().read(rs1);
                regfile.read_port::<1>().read(rs2);
                // Wait for both reads to be ready
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() && regfile.read_port::<1>().is_ready() {
                        break;
                    }
                }
                let rv1   = if rs1 == 0 { 0 } else { regfile.read_port::<0>().data() };
                let rv2   = if rs2 == 0 { 0 } else { regfile.read_port::<1>().data() };
                let taken = match f3 {
                    0x0 =>  rv1 == rv2,
                    0x1 =>  rv1 != rv2,
                    0x4 => (rv1 as i32) <  (rv2 as i32),
                    0x5 => (rv1 as i32) >= (rv2 as i32),
                    0x6 =>  rv1 <  rv2,
                    0x7 =>  rv1 >= rv2,
                    _   =>  false,
                };
                emit!((pc, false, 0u32));
                pc = if taken {
                    (pc as i32).wrapping_add(imm_b) as u32
                } else {
                    pc.wrapping_add(4)
                };
            }

            // ── LOAD (LW) ────────────────────────────────────────────────────
            0x03 => {
                regfile.read_port::<0>().read(rs1);
                wait_for!(clk, regfile.read_port::<0>());
                let rv1  = if rs1 == 0 { 0 } else { regfile.read_port::<0>().data() };
                let addr = ((rv1 as i32).wrapping_add(imm_i) as u32) >> 2;
                dmem.read_port::<0>().read(addr as usize);
                wait_for!(clk, dmem.read_port::<0>());
                let loaded = dmem.read_port::<0>().data();
                if rd != 0 {
                    regfile.write_port::<0>().write(rd, loaded);
                    wait_for!(clk, regfile.write_port::<0>());
                }
                emit!((pc, false, 0u32));
                pc = pc.wrapping_add(4);
            }

            // ── STORE (SW) ───────────────────────────────────────────────────
            0x23 => {
                regfile.read_port::<0>().read(rs1);
                regfile.read_port::<1>().read(rs2);
                // Wait for both reads to be ready
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() && regfile.read_port::<1>().is_ready() {
                        break;
                    }
                }
                let rv1  = if rs1 == 0 { 0 } else { regfile.read_port::<0>().data() };
                let rv2  = if rs2 == 0 { 0 } else { regfile.read_port::<1>().data() };
                let addr = ((rv1 as i32).wrapping_add(imm_s) as u32) >> 2;
                dmem.write_port::<0>().write(addr as usize, rv2);
                emit!((pc, false, 0u32));
                pc = pc.wrapping_add(4);
            }

            // ── ALU Immediate ────────────────────────────────────────────────
            0x13 => {
                regfile.read_port::<0>().read(rs1);
                wait_for!(clk, regfile.read_port::<0>());
                let rv1    = if rs1 == 0 { 0 } else { regfile.read_port::<0>().data() };
                let result = alu_imm(rv1, imm_i, f3, f7);
                if rd != 0 {
                    regfile.write_port::<0>().write(rd, result);
                    wait_for!(clk, regfile.write_port::<0>());
                }
                emit!((pc, false, 0u32));
                pc = pc.wrapping_add(4);
            }

            // ── ALU Register ─────────────────────────────────────────────────
            0x33 => {
                regfile.read_port::<0>().read(rs1);
                regfile.read_port::<1>().read(rs2);
                // Wait for both reads to be ready
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() && regfile.read_port::<1>().is_ready() {
                        break;
                    }
                }
                let rv1    = if rs1 == 0 { 0 } else { regfile.read_port::<0>().data() };
                let rv2    = if rs2 == 0 { 0 } else { regfile.read_port::<1>().data() };
                let result = alu_reg(rv1, rv2, f3, f7);
                if rd != 0 {
                    regfile.write_port::<0>().write(rd, result);
                    wait_for!(clk, regfile.write_port::<0>());
                }
                emit!((pc, false, 0u32));
                pc = pc.wrapping_add(4);
            }

            // ── ECALL: halt, expose a0 (x10) ─────────────────────────────────
            0x73 => {
                regfile.read_port::<0>().read(10); // x10 = a0
                wait_for!(clk, regfile.read_port::<0>());
                let a0 = regfile.read_port::<0>().data();
                emit!((pc, true, a0));
                // Spin without changing output so the executor stays alive.
                loop { clk.tick().await; }
            }

            _ => panic!("rv32i_cpu: unsupported opcode 0x{:02x} at PC=0x{:08x}", opcode, pc),
        }
    }
}

// ── Assembler helpers ─────────────────────────────────────────────────────────

fn i_type(rd: u32, rs1: u32, imm: i32, f3: u32, opcode: u32) -> u32 {
    ((imm as u32 & 0xFFF) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | opcode
}
fn r_type(rd: u32, rs1: u32, rs2: u32, f3: u32, f7: u32, opcode: u32) -> u32 {
    (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | opcode
}
fn b_type(rs1: u32, rs2: u32, offset: i32, f3: u32) -> u32 {
    let o    = offset as u32;
    let b12  = (o >> 12) & 1;
    let b11  = (o >> 11) & 1;
    let b10_5 = (o >> 5) & 0x3F;
    let b4_1  = (o >> 1) & 0xF;
    (b12 << 31) | (b10_5 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (b4_1 << 8) | (b11 << 7) | 0x63
}
fn j_type(rd: u32, offset: i32) -> u32 {
    let o      = offset as u32;
    let b20    = (o >> 20) & 1;
    let b19_12 = (o >> 12) & 0xFF;
    let b11    = (o >> 11) & 1;
    let b10_1  = (o >> 1)  & 0x3FF;
    (b20 << 31) | (b10_1 << 21) | (b11 << 20) | (b19_12 << 12) | (rd << 7) | 0x6F
}
fn s_type(rs1: u32, rs2: u32, offset: i32, f3: u32) -> u32 {
    let imm12 = offset as u32 & 0xFFF;
    ((imm12 >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | ((imm12 & 0x1F) << 7) | 0x23
}

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 { i_type(rd, rs1, imm, 0x0, 0x13) }
fn add(rd: u32, rs1: u32, rs2: u32) -> u32  { r_type(rd, rs1, rs2, 0x0, 0x00, 0x33) }
fn sub(rd: u32, rs1: u32, rs2: u32) -> u32  { r_type(rd, rs1, rs2, 0x0, 0x20, 0x33) }
fn beq(rs1: u32, rs2: u32, off: i32) -> u32 { b_type(rs1, rs2, off, 0x0) }
fn bne(rs1: u32, rs2: u32, off: i32) -> u32 { b_type(rs1, rs2, off, 0x1) }
fn blt(rs1: u32, rs2: u32, off: i32) -> u32 { b_type(rs1, rs2, off, 0x4) }
fn jal(rd: u32, off: i32) -> u32            { j_type(rd, off) }
fn lw(rd: u32, rs1: u32, imm: i32) -> u32  { i_type(rd, rs1, imm, 0x2, 0x03) }
fn sw(rs1: u32, rs2: u32, off: i32) -> u32 { s_type(rs1, rs2, off, 0x2) }
fn ecall() -> u32                           { 0x0000_0073 }

// ── Simulation runner ─────────────────────────────────────────────────────────

fn run_program(program: Vec<u32>, max_cycles: usize) -> u32 {
    let mut clk  = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let out = exec.spawn_function_typed((0u32, false, 0u32), rv32i_cpu(clk.clone(), program));

    for _ in 0..max_cycles {
        exec.tick_clock(&mut clk);
        let (_, halted, a0) = *out.lock().unwrap();
        if halted {
            return a0;
        }
    }
    panic!("Program did not halt within {} cycles", max_cycles);
}

/// Run a program with test configuration
fn run_program_with_config(program: Vec<u32>, config: &CpuTestConfig) -> BinaryTestResult<u32> {
    let mut clk  = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let out = exec.spawn_function_typed((0u32, false, 0u32), rv32i_cpu(clk.clone(), program));

    for cycle in 0..config.max_cycles {
        exec.tick_clock(&mut clk);
        let (pc, halted, a0) = *out.lock().unwrap();
        if halted {
            if config.verbose {
                println!("  Program halted at cycle {}, PC=0x{:08x}, a0={}", cycle, pc, a0);
            }
            return Ok(a0);
        }
    }
    Err(binary_test_utils::BinaryTestError::ExecutionTimeout(
        format!("Program did not halt within {} cycles", config.max_cycles)
    ))
}

/// Execute a binary program (from RV32IProgram) and return the result
pub fn run_binary_program(program: &RV32IProgram, config: &CpuTestConfig) -> BinaryTestResult<u32> {
    run_program_with_config(program.instructions.clone(), config)
}

// ── Test programs ─────────────────────────────────────────────────────────────

fn test_sum() {
    // Sum 1+2+3+4+5 = 15.
    // x1=sum, x2=i, x3=n=5
    // Loop: if n<i goto exit; sum+=i; i++; goto loop
    //
    // PC=0:  addi x1, x0, 0
    // PC=4:  addi x2, x0, 1
    // PC=8:  addi x3, x0, 5
    // PC=12: blt  x3, x2, +16  (n<i → PC=28)
    // PC=16: add  x1, x1, x2
    // PC=20: addi x2, x2, 1
    // PC=24: jal  x0, -12      (→ PC=12)
    // PC=28: addi x10, x1, 0
    // PC=32: ecall
    let prog = vec![
        addi(1, 0, 0),
        addi(2, 0, 1),
        addi(3, 0, 5),
        blt(3, 2, 16),
        add(1, 1, 2),
        addi(2, 2, 1),
        jal(0, -12),
        addi(10, 1, 0),
        ecall(),
    ];
    let result = run_program(prog, 500);
    assert_eq!(result, 15, "sum test: expected 15, got {}", result);
    println!("test_sum: PASS (a0 = {})", result);
}

fn test_load_store() {
    // Store 0xDEAD to dmem[0], load it back, return via a0.
    //
    // PC=0:  addi x1, x0, 0xDEAD (sign-extends to 12 bits → use LUI+ADDI)
    // Since 0xDEAD = 57005 doesn't fit in 12 bits, use two-instruction sequence:
    //   lui  x1, (0xDEAD >> 12)    = lui x1, 0xD      → x1 = 0xD000
    //   addi x1, x1, (0xDEAD&0xFFF) = addi x1, x1, 0xEAD → but 0xEAD is negative as 12-bit signed
    // 0xEAD = 3757, as signed 12-bit: bit11=1 so it's negative (-339).
    // LUI loads upper 20 bits. lui x1, 0xDE = x1 = 0xDE000.
    // addi x1, x1, -0x153 ... this is getting complicated.
    // Instead, just use 42 (fits in 12 bits).
    //
    // PC=0:  addi x1, x0, 42    # x1 = 42
    // PC=4:  sw   x0, x1, 0     # dmem[0] = 42
    // PC=8:  lw   x2, x0, 0     # x2 = dmem[0] = 42
    // PC=12: addi x10, x2, 0    # a0 = 42
    // PC=16: ecall
    let prog = vec![
        addi(1, 0, 42),
        sw(0, 1, 0),
        lw(2, 0, 0),
        addi(10, 2, 0),
        ecall(),
    ];
    let result = run_program(prog, 200);
    assert_eq!(result, 42, "load_store test: expected 42, got {}", result);
    println!("test_load_store: PASS (a0 = {})", result);
}

fn test_fibonacci() {
    // Compute fib(10) = 55 iteratively.
    // x1=fib_a=0, x2=fib_b=1, x3=count=10
    //
    // PC=0:  addi x1, x0, 0
    // PC=4:  addi x2, x0, 1
    // PC=8:  addi x3, x0, 10
    // PC=12: add  x4, x1, x2    # x4 = fib_a + fib_b
    // PC=16: addi x1, x2, 0     # x1 = x2 (fib_a ← old fib_b)
    // PC=20: addi x2, x4, 0     # x2 = x4 (fib_b ← x4)
    // PC=24: addi x3, x3, -1    # count--
    // PC=28: bne  x3, x0, -16   # if count != 0, goto PC=12
    // PC=32: addi x10, x1, 0    # a0 = fib_a = fib(10)
    // PC=36: ecall
    let prog = vec![
        addi(1, 0, 0),
        addi(2, 0, 1),
        addi(3, 0, 10),
        add(4, 1, 2),
        addi(1, 2, 0),
        addi(2, 4, 0),
        addi(3, 3, -1),
        bne(3, 0, -16),
        addi(10, 1, 0),
        ecall(),
    ];
    let result = run_program(prog, 1000);
    assert_eq!(result, 55, "fibonacci test: expected 55, got {}", result);
    println!("test_fibonacci: PASS (a0 = {})", result);
}

fn test_branches() {
    // Exercise BEQ, BNE, BLT.
    // Count how many of {1,2,3,4,5} are >= 4 (answer: 2, i.e. {4,5}).
    // Skip count++ when i < 4 (threshold), so count++ when i >= 4.
    //
    // x1=count=0, x2=i=1, x3=n=5, x4=threshold=4
    //
    // PC=0:  addi x1, x0, 0
    // PC=4:  addi x2, x0, 1
    // PC=8:  addi x3, x0, 5
    // PC=12: addi x4, x0, 4
    // Loop (PC=16):
    // PC=16: blt  x3, x2, +20   # if n<i goto exit (PC=36)
    // PC=20: blt  x2, x4,  +8   # if i<threshold skip count++ (PC=28)
    // PC=24: addi x1, x1, 1     # count++
    // PC=28: addi x2, x2, 1     # i++
    // PC=32: jal  x0, -16       # goto loop (PC=16)
    // Exit (PC=36):
    // PC=36: addi x10, x1, 0    # a0 = count
    // PC=40: ecall
    let prog = vec![
        addi(1, 0, 0),
        addi(2, 0, 1),
        addi(3, 0, 5),
        addi(4, 0, 4),
        blt(3, 2, 20),
        blt(2, 4, 8),
        addi(1, 1, 1),
        addi(2, 2, 1),
        jal(0, -16),
        addi(10, 1, 0),
        ecall(),
    ];
    let result = run_program(prog, 500);
    assert_eq!(result, 2, "branches test: expected 2, got {}", result);
    println!("test_branches: PASS (a0 = {})", result);
}

fn test_sub_and_beq() {
    // Compute 9 - 4 = 5, then branch on equality to verify SUB and BEQ.
    // PC=0:  addi x1, x0, 9
    // PC=4:  addi x2, x0, 4
    // PC=8:  sub  x3, x1, x2    # x3 = 5
    // PC=12: addi x4, x0, 5
    // PC=16: beq  x3, x4, +12   # jump to PC=28 if equal
    // PC=20: addi x10, x0, 7    # fail path
    // PC=24: ecall
    // PC=28: addi x10, x3, 0    # success path
    // PC=32: ecall
    let prog = vec![
        addi(1, 0, 9),
        addi(2, 0, 4),
        sub(3, 1, 2),
        addi(4, 0, 5),
        beq(3, 4, 12),
        addi(10, 0, 7),
        ecall(),
        addi(10, 3, 0),
        ecall(),
    ];
    let result = run_program(prog, 200);
    assert_eq!(result, 5, "sub_and_beq test: expected 5, got {}", result);
    println!("test_sub_and_beq: PASS (a0 = {})", result);
}

// ── Binary Testing Infrastructure ────────────────────────────────────────────────

/// Load and run an ELF binary
/// 
/// # Arguments
/// * `elf_path` - Path to the ELF binary file
/// * `expected_result` - Expected value in a0 when program halts
/// * `config` - Test configuration (max cycles, verbose logging)
///
/// # Returns
/// Result with the actual a0 value
pub fn test_elf_binary<P: AsRef<std::path::Path>>(
    elf_path: P,
    expected_result: Option<u32>,
    config: Option<CpuTestConfig>,
) -> BinaryTestResult<u32> {
    let program = RV32IProgram::from_elf(elf_path)?;
    let config = config.unwrap_or_default();

    if config.verbose {
        println!("{}", program.disassemble_summary());
    }

    let result = run_binary_program(&program, &config)?;

    if let Some(expected) = expected_result {
        if result != expected {
            return Err(binary_test_utils::BinaryTestError::ExecutionError(
                format!("Expected a0={}, got a0={}", expected, result),
            ));
        }
    }

    Ok(result)
}

/// Load and run a raw binary file
///
/// # Arguments
/// * `bin_path` - Path to the raw binary file
/// * `expected_result` - Expected value in a0 when program halts
/// * `config` - Test configuration (max cycles, verbose logging)
///
/// # Returns
/// Result with the actual a0 value
pub fn test_raw_binary<P: AsRef<std::path::Path>>(
    bin_path: P,
    expected_result: Option<u32>,
    config: Option<CpuTestConfig>,
) -> BinaryTestResult<u32> {
    let program = RV32IProgram::from_raw(bin_path)?;
    let config = config.unwrap_or_default();

    if config.verbose {
        println!("{}", program.disassemble_summary());
    }

    let result = run_binary_program(&program, &config)?;

    if let Some(expected) = expected_result {
        if result != expected {
            return Err(binary_test_utils::BinaryTestError::ExecutionError(
                format!("Expected a0={}, got a0={}", expected, result),
            ));
        }
    }

    Ok(result)
}

/// Example test that demonstrates binary testing
/// 
/// This shows how to use the binary testing infrastructure.
/// When you have actual binaries, you can call:
/// 
/// ```ignore
/// test_elf_binary(
///     "path/to/binary.elf",
///     Some(expected_value),
///     Some(CpuTestConfig::with_max_cycles(10000).verbose()),
/// ).expect("Binary test failed");
/// ```
fn test_binary_infrastructure() {
    println!("Binary testing infrastructure ready!");
    println!("To test a binary, call test_elf_binary() or test_raw_binary()");

    // Example: create a program and run it through the binary interface
    let prog = vec![
        addi(10, 0, 42), // a0 = 42
        ecall(),
    ];
    let program = RV32IProgram::new(prog);
    let config = CpuTestConfig::default();

    match run_binary_program(&program, &config) {
        Ok(result) => {
            assert_eq!(
                result, 42,
                "Binary infrastructure test: expected 42, got {}",
                result
            );
            println!("test_binary_infrastructure: PASS (a0 = {})", result);
        }
        Err(e) => panic!("Binary infrastructure test failed: {}", e),
    }
}

fn main() {
    test_sum();
    test_load_store();
    test_fibonacci();
    test_branches();
    test_sub_and_beq();
    test_binary_infrastructure();
    println!("All tests passed.");
}
