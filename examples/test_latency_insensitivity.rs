// Test: Latency Insensitivity
//
// This test verifies that the RV32I CPU works correctly with DIFFERENT memory latencies.
// The CPU should produce identical results regardless of READ_LAT/WRITE_LAT values.
//
// To run: cargo run --example test_latency_insensitivity

use copper_core::{Clock, ClockDomain, Memory};
use copper_macros::hardware;
use copper_sim::{emit, HardwareExecutor};

struct MainClk;
impl ClockDomain for MainClk {}

// ─────────────────────────────────────────────────────────────────────────────
// Minimal CPU (used to test latency variations)
// ─────────────────────────────────────────────────────────────────────────────

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
    Beq, Bne, Blt, Bge, Bltu, Bgeu,
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

fn alu_reg(a: u32, b: u32, f3: u32, f7: u32) -> u32 {
    match (f3, f7) {
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
    }
}

fn alu_imm(a: u32, imm: i32, f3: u32, f7: u32) -> u32 {
    let shamt = (imm as u32) & 0x1F;
    match f3 {
        0x0 => a.wrapping_add(imm as u32),
        0x1 => a << shamt,
        0x2 => ((a as i32) < imm) as u32,
        0x3 => (a < (imm as u32)) as u32,
        0x4 => a ^ (imm as u32),
        0x5 => if f7 & 0x20 != 0 { ((a as i32) >> shamt) as u32 } else { a >> shamt },
        0x6 => a | (imm as u32),
        0x7 => a & (imm as u32),
        _ => 0,
    }
}

// Generic CPU function with configurable memory latencies
#[hardware(function_typed)]
async fn rv32i_cpu_with_latencies<
    const IMEM_READ_LAT: usize,
    const DMEM_READ_LAT: usize,
    const DMEM_WRITE_LAT: usize,
    const REGFILE_READ_LAT: usize,
    const REGFILE_WRITE_LAT: usize,
>(
    clk: Clock<MainClk>,
    program: Vec<u32>,
) -> (u32, bool, u32) {
    let imem = Memory::<u32, 1, 0, MainClk, IMEM_READ_LAT, 1>::from_contents(clk.clone(), program);
    let dmem = Memory::<u32, 1, 1, MainClk, DMEM_READ_LAT, DMEM_WRITE_LAT>::new(clk.clone(), 1024);
    let regfile = Memory::<u32, 2, 1, MainClk, REGFILE_READ_LAT, REGFILE_WRITE_LAT>::new(clk.clone(), 32);

    let mut pc: u32 = 0;
    emit!((pc, false, 0u32));

    loop {
        // Fetch
        imem.read_port::<0>().read((pc >> 2) as usize);
        loop {
            clk.tick().await;
            if imem.read_port::<0>().is_ready() {
                break;
            }
        }
        let instr = imem.read_port::<0>().data();

        let decoded = match decode(instr) {
            Some(d) => d,
            None => panic!("Invalid instr"),
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
                let branch_cond = BranchCond::from_f3(decoded.f3).unwrap_or(BranchCond::Beq);
                let taken = match branch_cond {
                    BranchCond::Beq => rv1 == rv2,
                    BranchCond::Bne => rv1 != rv2,
                    BranchCond::Blt => (rv1 as i32) < (rv2 as i32),
                    BranchCond::Bge => (rv1 as i32) >= (rv2 as i32),
                    BranchCond::Bltu => rv1 < rv2,
                    BranchCond::Bgeu => rv1 >= rv2,
                };
                emit!((pc, false, 0u32));
                pc = if taken { (pc as i32).wrapping_add(decoded.imm_b) as u32 } else { pc.wrapping_add(4) };
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
                let result = alu_imm(rv1, decoded.imm_i, decoded.f3, decoded.f7);
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
                let result = alu_reg(rv1, rv2, decoded.f3, decoded.f7);
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

// Test helper - run program with given latencies
fn run_with_latencies<const IR: usize, const DR: usize, const DW: usize, const RR: usize, const RW: usize>(
    program: Vec<u32>,
    max_cycles: usize,
) -> u32 {
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();
    let out = exec.spawn_function_typed(
        (0u32, false, 0u32),
        rv32i_cpu_with_latencies::<IR, DR, DW, RR, RW>(clk.clone(), program),
    );

    for _ in 0..max_cycles {
        exec.tick_clock(&mut clk);
        let (_, halted, a0) = *out.lock().unwrap();
        if halted {
            return a0;
        }
    }
    panic!("Program did not halt within {} cycles", max_cycles);
}

// Assembler helpers
fn i_type(rd: u32, rs1: u32, imm: i32, f3: u32, opcode: u32) -> u32 {
    ((imm as u32 & 0xFFF) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | opcode
}
fn r_type(rd: u32, rs1: u32, rs2: u32, f3: u32, f7: u32, opcode: u32) -> u32 {
    (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | opcode
}
fn addi(rd: u32, rs1: u32, imm: i32) -> u32 { i_type(rd, rs1, imm, 0x0, 0x13) }
fn add(rd: u32, rs1: u32, rs2: u32) -> u32  { r_type(rd, rs1, rs2, 0x0, 0x00, 0x33) }

fn main() {
    println!("═══════════════════════════════════════════════════════════════");
    println!("TEST: Latency Insensitivity");
    println!("═══════════════════════════════════════════════════════════════");

    // Simple program: a0 = 10 + 5 = 15
    let prog = vec![
        addi(1, 0, 10),   // x1 = 10
        addi(2, 0, 5),    // x2 = 5
        add(10, 1, 2),    // x10 (a0) = 10 + 5
        0x0000_0073,      // ecall
    ];

    println!("\nProgram: a0 = (10) + (5) = 15");
    println!("Testing with different memory latencies...\n");

    // Configuration matrix: (IMEM_READ, DMEM_READ, DMEM_WRITE, REGFILE_READ, REGFILE_WRITE)
    let configs = vec![
        ("Fast (1-1-1-1-1)", (1, 1, 1, 1, 1), 100),
        ("Standard (2-2-1-1-1)", (2, 2, 1, 1, 1), 200),
        ("Slow IMEM (5-2-1-1-1)", (5, 2, 1, 1, 1), 300),
        ("Slow DMEM (2-5-2-1-1)", (2, 5, 2, 1, 1), 400),
        ("Slow RegFile (2-2-1-3-1)", (2, 2, 1, 3, 1), 300),
        ("Very Slow (10-10-2-5-2)", (10, 10, 2, 5, 2), 1000),
    ];

    let mut all_passed = true;

    for (name, (ir, dr, dw, rr, rw), max_cycles) in configs {
        let result = match (ir, dr, dw, rr, rw) {
            (1, 1, 1, 1, 1) => run_with_latencies::<1, 1, 1, 1, 1>(prog.clone(), max_cycles),
            (2, 2, 1, 1, 1) => run_with_latencies::<2, 2, 1, 1, 1>(prog.clone(), max_cycles),
            (5, 2, 1, 1, 1) => run_with_latencies::<5, 2, 1, 1, 1>(prog.clone(), max_cycles),
            (2, 5, 2, 1, 1) => run_with_latencies::<2, 5, 2, 1, 1>(prog.clone(), max_cycles),
            (2, 2, 1, 3, 1) => run_with_latencies::<2, 2, 1, 3, 1>(prog.clone(), max_cycles),
            (10, 10, 2, 5, 2) => run_with_latencies::<10, 10, 2, 5, 2>(prog.clone(), max_cycles),
            _ => panic!("Unsupported latency configuration"),
        };

        let status = if result == 15 { "✓ PASS" } else { "✗ FAIL" };
        if result != 15 {
            all_passed = false;
        }

        println!("  {}: {} (a0 = {}/15)", name, status, result);
    }

    println!("\n═══════════════════════════════════════════════════════════════");
    if all_passed {
        println!("RESULT: All latency configurations PASSED!");
        println!("Design is successfully latency-insensitive.");
    } else {
        println!("RESULT: Some configurations FAILED!");
        panic!("Latency insensitivity test failed");
    }
    println!("═══════════════════════════════════════════════════════════════");
}
