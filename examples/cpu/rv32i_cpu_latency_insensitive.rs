// RV32I Non-pipelined CPU - Latency Insensitive Architecture
//
// - Interface-based abstractions for all memory operations
// - Ready/valid handshaking for latency insensitivity
// - Modular component design
//
// Key Design Principle:
// The CPU never assumes fixed latencies. It only knows about the interface contract:
// - Issue a request (address or write)
// - Poll is_ready()
// - Wait until ready, then capture data
//
// This design automatically works with ANY memory latency configuration.

use copper_core::{Clock, ClockDomain, Memory};
use copper_macros::hardware;
use copper_sim::{emit, HardwareExecutor};

struct MainClk;
impl ClockDomain for MainClk {}

// ══════════════════════════════════════════════════════════════════════════════
// INTERFACE ABSTRACTIONS 
// ══════════════════════════════════════════════════════════════════════════════
//
// Here we define the same contracts as Rust trait bounds.
// This allows the CPU to work with ANY implementation that satisfies the interface.

/// Contract for memory read operations
/// The CPU can issue a read request, poll readiness, and fetch data.
pub trait ReadOp {
    fn issue_read(&self, addr: usize);
    fn read_ready(&self) -> bool;
    fn read_data(&self) -> u32;
}

/// Contract for memory write operations
pub trait WriteOp {
    fn issue_write(&self, addr: usize, value: u32);
    fn write_ready(&self) -> bool;
}

// ══════════════════════════════════════════════════════════════════════════════
// RV32I TYPE SYSTEM 
// ══════════════════════════════════════════════════════════════════════════════

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchCond {
    Beq,   Bne,   Blt,   Bge,   Bltu,  Bgeu,
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

#[derive(Debug, Clone, Copy)]
pub struct InstrDecoded {
    pub opcode: Opcode,
    pub rd: usize,    pub rs1: usize,   pub rs2: usize,
    pub f3: u32,      pub f7: u32,
    pub imm_i: i32,   pub imm_s: i32,   pub imm_b: i32,
    pub imm_j: i32,   pub imm_u: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct AluOutput {
    pub result: u32,
    pub overflow: bool,
    pub zero: bool,
    pub negative: bool,
}

// ══════════════════════════════════════════════════════════════════════════════
// INSTRUCTION DECODER & ALU 
// ══════════════════════════════════════════════════════════════════════════════

fn sign_ext_i(instr: u32) -> i32 { (instr as i32) >> 20 }
fn sign_ext_s(instr: u32) -> i32 {
    let hi7 = (instr >> 25) & 0x7F;
    let lo5 = (instr >> 7) & 0x1F;
    (((hi7 << 5) | lo5) as i32) << 20 >> 20
}
fn sign_ext_b(instr: u32) -> i32 {
    let b12 = (instr >> 31) & 1;
    let b11 = (instr >> 7) & 1;
    let b10_5 = (instr >> 25) & 0x3F;
    let b4_1 = (instr >> 8) & 0xF;
    let raw = (b12 << 12) | (b11 << 11) | (b10_5 << 5) | (b4_1 << 1);
    ((raw as i32) << 19) >> 19
}
fn sign_ext_j(instr: u32) -> i32 {
    let b20 = (instr >> 31) & 1;
    let b10_1 = (instr >> 21) & 0x3FF;
    let b11 = (instr >> 20) & 1;
    let b19_12 = (instr >> 12) & 0xFF;
    let raw = (b20 << 20) | (b19_12 << 12) | (b11 << 11) | (b10_1 << 1);
    ((raw as i32) << 11) >> 11
}

fn decode(instr: u32) -> Option<InstrDecoded> {
    let opcode = Opcode::from_u32(instr & 0x7F)?;
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

fn alu_exec_reg(a: u32, b: u32, f3: u32, f7: u32) -> AluOutput {
    let result = match (f3, f7) {
        (0x0, 0x00) => a.wrapping_add(b),
        (0x0, 0x20) => a.wrapping_sub(b),
        (0x1, _) => a << (b & 0x1F),
        (0x2, _) => ((a as i32) < (b as i32)) as u32,
        (0x3, _) => (a < b) as u32,
        (0x4, _) => a ^ b,
        (0x5, 0x00) => a >> (b & 0x1F),
        (0x5, 0x20) => ((a as i32) >> (b & 0x1F)) as u32,
        (0x6, _) => a | b,
        (0x7, _) => a & b,
        _ => 0,
    };
    AluOutput {
        result,
        overflow: false,
        zero: result == 0,
        negative: (result as i32) < 0,
    }
}

fn alu_exec_imm(a: u32, imm: i32, f3: u32, f7: u32) -> AluOutput {
    let shamt = (imm as u32) & 0x1F;
    let result = match f3 {
        0x0 => a.wrapping_add(imm as u32),
        0x1 => a << shamt,
        0x2 => ((a as i32) < imm) as u32,
        0x3 => (a < (imm as u32)) as u32,
        0x4 => a ^ (imm as u32),
        0x5 => if f7 & 0x20 != 0 { ((a as i32) >> shamt) as u32 } else { a >> shamt },
        0x6 => a | (imm as u32),
        0x7 => a & (imm as u32),
        _ => 0,
    };
    AluOutput {
        result,
        overflow: false,
        zero: result == 0,
        negative: (result as i32) < 0,
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// CPU: LATENCY INSENSITIVE (uses interface abstractions only)
// ══════════════════════════════════════════════════════════════════════════════

// ══════════════════════════════════════════════════════════════════════════════
// CPU: LATENCY INSENSITIVE (uses is_ready() polling, not hardcoded latencies)
// ══════════════════════════════════════════════════════════════════════════════
//
// Core principle: The CPU never assumes fixed latencies.
// - Issues request (read/write)
// - Polls is_ready() 
// - Acts when ready
//
// This works with ANY Memory latency (READ_LAT, WRITE_LAT).
// Just change the Memory type parameters, CPU logic unchanged.

#[hardware(function_typed)]
async fn rv32i_cpu(
    clk: Clock<MainClk>,
    program: Vec<u32>,
) -> (u32, bool, u32) {
    // Memory instantiations - only type parameters control latency
    let imem = Memory::<u32, 1, 0, MainClk, 2, 1>::from_contents(clk.clone(), program);
    let dmem = Memory::<u32, 1, 1, MainClk, 2, 1>::new(clk.clone(), 1024);
    let regfile = Memory::<u32, 2, 1, MainClk, 1, 1>::new(clk.clone(), 32);

    let mut pc: u32 = 0;
    emit!((pc, false, 0u32));

    loop {
        // ── IF: Latency-insensitive fetch (works with any IMEM READ_LAT) ───
        imem.read_port::<0>().read((pc >> 2) as usize);
        loop {
            clk.tick().await;
            if imem.read_port::<0>().is_ready() {
                break;
            }
        }
        let instr = imem.read_port::<0>().data();

        // ── Decode instruction ─────────────────────────────────────────────
        let decoded = match decode(instr) {
            Some(d) => d,
            None => panic!("Invalid instr 0x{:08x} at PC 0x{:08x}", instr, pc),
        };

        match decoded.opcode {
            Opcode::LUI => {
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, decoded.imm_u);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() {
                            break;
                        }
                    }
                }
                emit!((pc, false, 0u32));
                pc = pc.wrapping_add(4);
            }

            Opcode::AUIPC => {
                let result = pc.wrapping_add(decoded.imm_u);
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, result);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() {
                            break;
                        }
                    }
                }
                emit!((pc, false, 0u32));
                pc = pc.wrapping_add(4);
            }

            Opcode::JAL => {
                let link = pc.wrapping_add(4);
                let next = (pc as i32).wrapping_add(decoded.imm_j) as u32;
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, link);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() {
                            break;
                        }
                    }
                }
                emit!((pc, false, 0u32));
                pc = next;
            }

            Opcode::JALR => {
                regfile.read_port::<0>().read(decoded.rs1);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() {
                        break;
                    }
                }
                let rv1 = if decoded.rs1 == 0 { 0 } else { regfile.read_port::<0>().data() };
                let link = pc.wrapping_add(4);
                let next = ((rv1 as i32).wrapping_add(decoded.imm_i) & !1) as u32;
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, link);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() {
                            break;
                        }
                    }
                }
                emit!((pc, false, 0u32));
                pc = next;
            }

            Opcode::BRANCH => {
                regfile.read_port::<0>().read(decoded.rs1);
                regfile.read_port::<1>().read(decoded.rs2);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() && regfile.read_port::<1>().is_ready() {
                        break;
                    }
                }
                let rv1 = if decoded.rs1 == 0 { 0 } else { regfile.read_port::<0>().data() };
                let rv2 = if decoded.rs2 == 0 { 0 } else { regfile.read_port::<1>().data() };
                
                let branch_cond = BranchCond::from_f3(decoded.f3)
                    .unwrap_or(BranchCond::Beq);
                let taken = match branch_cond {
                    BranchCond::Beq => rv1 == rv2,
                    BranchCond::Bne => rv1 != rv2,
                    BranchCond::Blt => (rv1 as i32) < (rv2 as i32),
                    BranchCond::Bge => (rv1 as i32) >= (rv2 as i32),
                    BranchCond::Bltu => rv1 < rv2,
                    BranchCond::Bgeu => rv1 >= rv2,
                };
                emit!((pc, false, 0u32));
                pc = if taken {
                    (pc as i32).wrapping_add(decoded.imm_b) as u32
                } else {
                    pc.wrapping_add(4)
                };
            }

            Opcode::LOAD => {
                regfile.read_port::<0>().read(decoded.rs1);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() {
                        break;
                    }
                }
                let rv1 = if decoded.rs1 == 0 { 0 } else { regfile.read_port::<0>().data() };
                let addr = ((rv1 as i32).wrapping_add(decoded.imm_i) as u32) >> 2;
                dmem.read_port::<0>().read(addr as usize);
                loop {
                    clk.tick().await;
                    if dmem.read_port::<0>().is_ready() {
                        break;
                    }
                }
                let loaded = dmem.read_port::<0>().data();
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, loaded);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() {
                            break;
                        }
                    }
                }
                emit!((pc, false, 0u32));
                pc = pc.wrapping_add(4);
            }

            Opcode::STORE => {
                regfile.read_port::<0>().read(decoded.rs1);
                regfile.read_port::<1>().read(decoded.rs2);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() && regfile.read_port::<1>().is_ready() {
                        break;
                    }
                }
                let rv1 = if decoded.rs1 == 0 { 0 } else { regfile.read_port::<0>().data() };
                let rv2 = if decoded.rs2 == 0 { 0 } else { regfile.read_port::<1>().data() };
                let addr = ((rv1 as i32).wrapping_add(decoded.imm_s) as u32) >> 2;
                dmem.write_port::<0>().write(addr as usize, rv2);
                emit!((pc, false, 0u32));
                pc = pc.wrapping_add(4);
            }

            Opcode::ALU_IMM => {
                regfile.read_port::<0>().read(decoded.rs1);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() {
                        break;
                    }
                }
                let rv1 = if decoded.rs1 == 0 { 0 } else { regfile.read_port::<0>().data() };
                let alu_out = alu_exec_imm(rv1, decoded.imm_i, decoded.f3, decoded.f7);
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, alu_out.result);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() {
                            break;
                        }
                    }
                }
                emit!((pc, false, 0u32));
                pc = pc.wrapping_add(4);
            }

            Opcode::ALU_REG => {
                regfile.read_port::<0>().read(decoded.rs1);
                regfile.read_port::<1>().read(decoded.rs2);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() && regfile.read_port::<1>().is_ready() {
                        break;
                    }
                }
                let rv1 = if decoded.rs1 == 0 { 0 } else { regfile.read_port::<0>().data() };
                let rv2 = if decoded.rs2 == 0 { 0 } else { regfile.read_port::<1>().data() };
                let alu_out = alu_exec_reg(rv1, rv2, decoded.f3, decoded.f7);
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, alu_out.result);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() {
                            break;
                        }
                    }
                }
                emit!((pc, false, 0u32));
                pc = pc.wrapping_add(4);
            }

            Opcode::ECALL => {
                regfile.read_port::<0>().read(10);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() {
                        break;
                    }
                }
                let a0 = regfile.read_port::<0>().data();
                emit!((pc, true, a0));
                loop { clk.tick().await; }
            }
        }
    }
}

// ── Execution Engine ───────────────────────────────────────────────────────────

fn run_program(program: Vec<u32>, max_cycles: usize) -> u32 {
    let mut clk = Clock::<MainClk>::new();
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

// ── Assembler Helpers ──────────────────────────────────────────────────────────

fn i_type(rd: u32, rs1: u32, imm: i32, f3: u32, opcode: u32) -> u32 {
    ((imm as u32 & 0xFFF) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | opcode
}

fn r_type(rd: u32, rs1: u32, rs2: u32, f3: u32, f7: u32, opcode: u32) -> u32 {
    (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | opcode
}

fn b_type(rs1: u32, rs2: u32, offset: i32, f3: u32) -> u32 {
    let o = offset as u32;
    let b12 = (o >> 12) & 1;
    let b11 = (o >> 11) & 1;
    let b10_5 = (o >> 5) & 0x3F;
    let b4_1 = (o >> 1) & 0xF;
    (b12 << 31) | (b10_5 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (b4_1 << 8) | (b11 << 7) | 0x63
}

fn s_type(rs1: u32, rs2: u32, offset: i32, f3: u32) -> u32 {
    let imm12 = offset as u32 & 0xFFF;
    ((imm12 >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | ((imm12 & 0x1F) << 7) | 0x23
}

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 { i_type(rd, rs1, imm, 0x0, 0x13) }
fn add(rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(rd, rs1, rs2, 0x0, 0x00, 0x33) }
fn sub(rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(rd, rs1, rs2, 0x0, 0x20, 0x33) }
fn beq(rs1: u32, rs2: u32, off: i32) -> u32 { b_type(rs1, rs2, off, 0x0) }
fn bne(rs1: u32, rs2: u32, off: i32) -> u32 { b_type(rs1, rs2, off, 0x1) }
fn blt(rs1: u32, rs2: u32, off: i32) -> u32 { b_type(rs1, rs2, off, 0x4) }
fn lw(rd: u32, rs1: u32, imm: i32) -> u32 { i_type(rd, rs1, imm, 0x2, 0x03) }
fn sw(rs1: u32, rs2: u32, off: i32) -> u32 { s_type(rs1, rs2, off, 0x2) }

// ── Test Programs ──────────────────────────────────────────────────────────────

fn test_addi() -> Vec<u32> {
    // Test: a0 = 15 (using immediate addition)
    // x1 = 10; x2 = 5; a0 = x1 + x2 (via add)
    vec![
        addi(1, 0, 10),      // x1 = 10
        addi(2, 0, 5),       // x2 = 5
        add(10, 1, 2),       // a0 = x1 + x2 = 15
        0x0000_0073,         // ecall
    ]
}

fn test_sub() -> Vec<u32> {
    // Test: a0 = 5 (using subtraction)
    // x1 = 15; x2 = 10; a0 = x1 - x2 = 5
    vec![
        addi(1, 0, 15),      // x1 = 15
        addi(2, 0, 10),      // x2 = 10
        sub(10, 1, 2),       // a0 = x1 - x2 = 5
        0x0000_0073,         // ecall
    ]
}

fn test_multiple_adds() -> Vec<u32> {
    // Test: a0 = 1+2+3+4+5 = 15
    // sum in x1, accumulate with loop
    vec![
        addi(1, 0, 0),       // x1 = 0 (sum)
        addi(1, 1, 1),       // x1 += 1
        addi(1, 1, 2),       // x1 += 2
        addi(1, 1, 3),       // x1 += 3
        addi(1, 1, 4),       // x1 += 4
        addi(1, 1, 5),       // x1 += 5
        addi(10, 1, 0),      // a0 = x1 = 15
        0x0000_0073,         // ecall
    ]
}

fn test_branch_taken() -> Vec<u32> {
    // Test: branch taken, a0 = 42
    // if x0 == x0 (true) jump to end, else a0 = 1
    // PC=0:  addi x1, x0, 1
    // PC=4:  beq x0, x0, +12  (branch to PC=16)
    // PC=8:  addi x10, x0, 1  (skipped)
    // PC=12: ecall            (skipped)
    // PC=16: addi x10, x0, 42 (branch target)
    // PC=20: ecall
    vec![
        addi(1, 0, 1),       // PC=0:  x1 = 1
        beq(0, 0, 12),       // PC=4:  if x0==x0 jump +12 (to PC=16)
        addi(10, 0, 1),      // PC=8:  a0 = 1 (skipped)
        0x0000_0073,         // PC=12: ecall (skipped)
        addi(10, 0, 42),     // PC=16: a0 = 42 (branch target)
        0x0000_0073,         // PC=20: ecall
    ]
}

fn test_branch_not_taken() -> Vec<u32> {
    // Test: branch not taken, a0 = 99
    // if x1 == x2 jump (not taken since x1=5, x2=10)
    // a0 = 99
    vec![
        addi(1, 0, 5),       // x1 = 5
        addi(2, 0, 10),      // x2 = 10
        beq(1, 2, 8),        // if x1==x2 jump (not taken)
        addi(10, 0, 99),     // a0 = 99 (executed)
        0x0000_0073,         // ecall
    ]
}

fn test_load_store() -> Vec<u32> {
    // Test: store value to memory, load it back
    // Store 88 to dmem[0], load back to a0
    vec![
        addi(1, 0, 88),      // x1 = 88
        sw(0, 1, 0),         // dmem[0] = x1
        lw(10, 0, 0),        // a0 = dmem[0]
        0x0000_0073,         // ecall
    ]
}

fn test_negative_numbers() -> Vec<u32> {
    // Test: a0 = 10 + (-3) = 7
    vec![
        addi(1, 0, 10),      // x1 = 10
        addi(2, 0, -3),      // x2 = -3
        add(10, 1, 2),       // a0 = x1 + x2 = 7
        0x0000_0073,         // ecall
    ]
}

fn test_zero_operations() -> Vec<u32> {
    // Test: various operations with 0
    // a0 = 0 + 42 = 42
    vec![
        addi(1, 0, 0),       // x1 = 0
        addi(2, 0, 42),      // x2 = 42
        add(10, 1, 2),       // a0 = 0 + 42 = 42
        0x0000_0073,         // ecall
    ]
}

// ── Main Entry ─────────────────────────────────────────────────────────────────

fn main() {
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║  RV32I CPU - Latency Insensitive Design - Extended Test Suite  ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    let tests = vec![
        ("ADDI: Simple addition", test_addi(), 15),
        ("SUB: Subtraction", test_sub(), 5),
        ("Multiple ADDIs: Sum 1+2+3+4+5", test_multiple_adds(), 15),
        ("BEQ: Branch taken", test_branch_taken(), 42),
        ("BEQ: Branch not taken", test_branch_not_taken(), 99),
        ("Load/Store: Memory operations", test_load_store(), 88),
        ("Negative: 10 + (-3)", test_negative_numbers(), 7),
        ("Zero: 0 + 42", test_zero_operations(), 42),
    ];

    let mut passed = 0;
    let mut failed = 0;

    for (name, prog, expected) in tests {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_program(prog, 500)
        })) {
            Ok(result) => {
                if result == expected {
                    println!("  ✓ PASS: {} (a0 = {})", name, result);
                    passed += 1;
                } else {
                    println!("  ✗ FAIL: {} (expected {}, got {})", name, expected, result);
                    failed += 1;
                }
            }
            Err(_) => {
                println!("  ✗ PANIC: {}", name);
                failed += 1;
            }
        }
    }

    println!("\n╔════════════════════════════════════════════════════════════════╗");
    println!("║  Results: {} passed, {} failed", passed, failed);
    if failed == 0 {
        println!("║  ✅ All tests PASSED!");
    } else {
        println!("║  ❌ Some tests FAILED!");
    }
    println!("╚════════════════════════════════════════════════════════════════╝");

    assert_eq!(failed, 0, "Some tests failed!");
}
