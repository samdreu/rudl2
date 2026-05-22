use copper_core::{Clock, ClockDomain, Memory, Bits};
use copper_sim::HardwareExecutor;
use std::sync::{Arc, Mutex};

// ────────────────────────────────────────────────────────────────────────────
// RV32I ISA Subset — Non-Pipelined CPU
// ────────────────────────────────────────────────────────────────────────────
//
// Minimal RV32I: ADD, SUB, AND, OR, XOR, SLL, SRL, ADDI, LW, SW, BEQ, BNE, JAL
// Word width: 32 bits
// Register count: 32 (x0-x31, x0 always 0)
// Memory: 1024 words (4 KB)
//
// Execution: fetch → decode → execute → memory → writeback per cycle

struct MainClk;
impl ClockDomain for MainClk {}

// instruction specifications
type RegIndex = Bits<5>;
type Opcode = Bits<7>;
type Funct3 = Bits<3>;
type Funct7 = Bits<7>;
type Immediate20 = Bits<20>;
type Immediate12 = Bits<12>;
type Immediate7 = Bits<7>;
type Immediate5 = Bits<5>;

fn bits<const N: usize>(value: u128) -> Bits<N> {
    Bits::from_u128(value)
}

struct RType {
    opcode: Opcode,
    rd: RegIndex,
    funct3: Funct3,
    rs1: RegIndex,
    rs2: RegIndex,
    funct7: Funct7,
}

struct IType {
    opcode: Opcode,
    rd: RegIndex,
    funct3: Funct3,
    rs1: RegIndex,
    imm12: Immediate12,
}

struct SType {
    opcode: Opcode,
    imm5: Immediate5,
    funct3: Funct3,
    rs1: RegIndex,
    rs2: RegIndex,
    imm7: Immediate7,
}

struct BType {
    opcode: Opcode,
    imm5: Immediate5,
    funct3: Funct3,
    rs1: RegIndex,
    rs2: RegIndex,
    imm7: Immediate7,
}

struct UType {
    opcode: Opcode,
    rd: RegIndex,
    imm20: Immediate20,
}

struct JType {
    opcode: Opcode,
    rd: RegIndex,
    imm20: Immediate20,
}

enum Instruction {
    R(RType),
    I(IType),
    S(SType),
    B(BType),
    U(UType),
    J(JType),
}

const LUI_OPCODE: u128 = 0b0110111;
const AUIPC_OPCODE: u128 = 0b0010111;
const JAL_OPCODE: u128 = 0b1101111;
const JALR_OPCODE: u128 = 0b1100111;
const BRANCH_OPCODE: u128 = 0b1100011;
const LOAD_OPCODE: u128 = 0b0000011;
const STORE_OPCODE: u128 = 0b0100011;
const ALU_IMM_OPCODE: u128 = 0b0010011;
const ALU_REG_OPCODE: u128 = 0b0110011;
const FENCE_OPCODE: u128 = 0b0001111;
const ECALL_OPCODE: u128 = 0b1110011;

const RD_SHIFT: u8 = 7;
const R_MASK: u32 = 0x1F;
const FUNCT3_SHIFT: u8 = 12;
const FUNCT7_SHIFT: u8 = 25;
const FUNCT3_MASK: u32 = 0x7;
const FUNCT7_MASK: u32 = 0x7F;
const RS1_SHIFT: u8 = 15;
const RS2_SHIFT: u8 = 20;
const IMM5_SHIFT: u8 = 7;
const IMM7_SHIFT: u8 = 25;
const IMM12_SHIFT: u8 = 20;
const IMM20_SHIFT: u8 = 12;



fn parse_instruction(word: u32) -> Instruction {
    let opcode_raw = (word & 0x7F) as u128;
    let opcode = bits::<7>(opcode_raw);
    match opcode_raw {
        LUI_OPCODE | AUIPC_OPCODE => {
            let rd = bits::<5>(((word >> RD_SHIFT) & R_MASK) as u128);
            let imm20 = bits::<20>((word >> IMM20_SHIFT) as u128);
            Instruction::U(UType { opcode, rd, imm20 })
        }
        JAL_OPCODE => {
            let rd = bits::<5>(((word >> RD_SHIFT) & R_MASK) as u128);
            let imm20 = bits::<20>(((word >> IMM20_SHIFT) & 0xFFFFF) as u128);
            Instruction::J(JType { opcode, rd, imm20 })
        }
        JALR_OPCODE | ALU_IMM_OPCODE | LOAD_OPCODE => {
            let rd = bits::<5>(((word >> RD_SHIFT) & R_MASK) as u128);
            let funct3 = bits::<3>(((word >> FUNCT3_SHIFT) & FUNCT3_MASK) as u128);
            let rs1 = bits::<5>(((word >> RS1_SHIFT) & R_MASK) as u128);
            let imm12 = bits::<12>(((word as i32) >> IMM12_SHIFT) as u128); // sign-extend
            Instruction::I(IType { opcode, rd, funct3, rs1, imm12 })
        }
        BRANCH_OPCODE => {
            let imm5 = bits::<5>(((word >> IMM5_SHIFT) & 0x1F) as u128);
            let funct3 = bits::<3>(((word >> FUNCT3_SHIFT) & FUNCT3_MASK) as u128);
            let rs1 = bits::<5>(((word >> RS1_SHIFT) & R_MASK) as u128);
            let rs2 = bits::<5>(((word >> RS2_SHIFT) & R_MASK) as u128);  
            let imm7 = bits::<7>(((word as i32) >> IMM7_SHIFT) as u128); // sign-extend
            Instruction::B(BType { opcode, imm5, funct3, rs1, rs2, imm7 })
        }
        STORE_OPCODE => {
            let imm5 = bits::<5>(((word >> IMM5_SHIFT) & 0x1F) as u128);
            let funct3 = bits::<3>(((word >> FUNCT3_SHIFT) & FUNCT3_MASK) as u128);
            let rs1 = bits::<5>(((word >> RS1_SHIFT) & R_MASK) as u128);
            let rs2 = bits::<5>(((word >> RS2_SHIFT) & R_MASK) as u128);
            let imm7 = bits::<7>(((word as i32) >> IMM7_SHIFT) as u128); // sign-extend
            Instruction::S(SType { opcode, imm5, funct3, rs1, rs2, imm7 })
        }
        ALU_REG_OPCODE => {
            let rd = bits::<5>(((word >> RD_SHIFT) & R_MASK) as u128);
            let funct3 = bits::<3>(((word >> FUNCT3_SHIFT) & FUNCT3_MASK) as u128);
            let rs1 = bits::<5>(((word >> RS1_SHIFT) & R_MASK) as u128);
            let rs2 = bits::<5>(((word >> RS2_SHIFT) & R_MASK) as u128);
            let funct7 = bits::<7>(((word >> FUNCT7_SHIFT) & FUNCT7_MASK) as u128);
            Instruction::R(RType { opcode, rd, funct3, rs1, rs2, funct7 })
        }
        FENCE_OPCODE | ECALL_OPCODE => {
            // For simplicity, treat FENCE and ECALL as R-type with no operands
            Instruction::R(RType { opcode, rd: bits::<5>(0), funct3: bits::<3>(0), rs1: bits::<5>(0), rs2: bits::<5>(0), funct7: bits::<7>(0) })
        }
        _ => panic!("Unsupported opcode: {:02x}", opcode_raw),   
    }
    
}


enum ALUOperation {
    ADD,
    SUB,
    AND,
    OR,
    XOR,
    SLL,
    SRL,
}

fn alu(a:u32, b: u32, operation: ALUOperation) -> u32{
    match(operation) {
        ALUOperation::ADD => a.wrapping_add(b),
        ALUOperation::SUB => a.wrapping_sub(b),
        ALUOperation::AND => a & b,
        ALUOperation::OR => a | b,
        ALUOperation::XOR => a ^ b,
        ALUOperation::SLL => a << (b & 0x1F),
        ALUOperation::SRL => a >> (b & 0x1F),
    }
}

#[hardware(function_typed)]
fn rv32i (
    clk: Clock<MainClk>,
    instr_mem: Arc<Mutex<Memory<u32, 1, 0, MainClk>>>,
    data_mem: Arc<Mutex<Memory<u32, 1, 1, MainClk>>>,
) {
    let mut registers = Memory::<u32, 1, 1, MainClk>::new(clk.clone(), 32); // 32 registers
    let mut pc: u32 = 0;
    
    loop {
        instruction = fetch_instruction()
    }
    
}

#[test]
fn parses_r_type_instruction() {
    let instr = parse_instruction(0x00208233); // add x4, x1, x2

    match instr {
        Instruction::R(r) => {
            assert_eq!(r.opcode.as_u128(), ALU_REG_OPCODE);
            assert_eq!(r.rd, bits::<5>(4));
            assert_eq!(r.funct3, bits::<3>(0));
            assert_eq!(r.rs1, bits::<5>(1));
            assert_eq!(r.rs2, bits::<5>(2));
            assert_eq!(r.funct7, bits::<7>(0));
        }
        _ => panic!("expected R-type instruction"),
    }
}

#[test]
fn parses_i_type_instruction() {
    let instr = parse_instruction(0x00A00093); // addi x1, x0, 10

    match instr {
        Instruction::I(i) => {
            assert_eq!(i.opcode.as_u128(), ALU_IMM_OPCODE);
            assert_eq!(i.rd, bits::<5>(1));
            assert_eq!(i.funct3, bits::<3>(0));
            assert_eq!(i.rs1, bits::<5>(0));
            assert_eq!(i.imm12, bits::<12>(10));
        }
        _ => panic!("expected I-type instruction"),
    }
}

#[test]
fn parses_s_type_instruction() {
    let instr = parse_instruction(0x0020A823); // sw x2, 16(x1)

    match instr {
        Instruction::S(s) => {
            assert_eq!(s.opcode.as_u128(), STORE_OPCODE);
            assert_eq!(s.imm5, bits::<5>(16 & 0x1F));
            assert_eq!(s.funct3, bits::<3>(2));
            assert_eq!(s.rs1, bits::<5>(1));
            assert_eq!(s.rs2, bits::<5>(2));
            assert_eq!(s.imm7, bits::<7>(0));
        }
        _ => panic!("expected S-type instruction"),
    }
}

#[test]
fn parses_b_type_instruction() {
    let instr = parse_instruction(0x00208663); // beq x1, x2, +12 (raw fields)

    match instr {
        Instruction::B(b) => {
            assert_eq!(b.opcode.as_u128(), BRANCH_OPCODE);
            assert_eq!(b.imm5, bits::<5>(12 & 0x1F));
            assert_eq!(b.funct3, bits::<3>(0));
            assert_eq!(b.rs1, bits::<5>(1));
            assert_eq!(b.rs2, bits::<5>(2));
            assert_eq!(b.imm7, bits::<7>(0));
        }
        _ => panic!("expected B-type instruction"),
    }
}

#[test]
fn parses_u_type_instruction() {
    let instr = parse_instruction(0x123450B7); // lui x1, 0x12345

    match instr {
        Instruction::U(u) => {
            assert_eq!(u.opcode.as_u128(), LUI_OPCODE);
            assert_eq!(u.rd, bits::<5>(1));
            assert_eq!(u.imm20, bits::<20>(0x12345));
        }
        _ => panic!("expected U-type instruction"),
    }
}

#[test]
fn parses_j_type_instruction() {
    let instr = parse_instruction(0x004000EF); // jal x1, +4

    match instr {
        Instruction::J(j) => {
            assert_eq!(j.opcode.as_u128(), JAL_OPCODE);
            assert_eq!(j.rd, bits::<5>(1));
            assert_eq!(j.imm20, bits::<20>(0x00400));
        }
        _ => panic!("expected J-type instruction"),
    }
}

#[test]
#[should_panic(expected = "Unsupported opcode")]
fn rejects_unsupported_opcode() {
    let _ = parse_instruction(0x00000000);
}

