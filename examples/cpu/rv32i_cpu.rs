use copper_core::{Bits, Clock, ClockDomain, Logic, Memory};
use copper_core::port::{wire, In, Out};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk{}

/// Contract for memory read operations
pub trait ReadOp {
    fn issue_read(&self, addr: usize);
    fn read_ready(&self) -> bool;
    fn read_data(&self) -> Bits<32>;
}

/// Contract for memory write operations
pub trait WriteOp {
    fn issue_write(&self, addr: usize, value: Bits<32>);
    fn write_ready(&self) -> bool;
}

// Instructions

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
    // An unrecognized opcode. Hardware has no `Option`, so decode is total: an
    // illegal instruction is an explicit value the module traps on, not `None`.
    INVALID = 0x00,
}

impl Opcode {
    pub fn from_bits(op: Bits<7>) -> Self {
        match op.as_usize() {
            0x37 => Opcode::LUI,
            0x17 => Opcode::AUIPC,
            0x6F => Opcode::JAL,
            0x67 => Opcode::JALR,
            0x63 => Opcode::BRANCH,
            0x03 => Opcode::LOAD,
            0x23 => Opcode::STORE,
            0x13 => Opcode::ALU_IMM,
            0x33 => Opcode::ALU_REG,
            0x73 => Opcode::ECALL,
            _ => Opcode::INVALID,
        }
    }
}

pub enum BranchCond {
    Beq,   Bne,   Blt,   Bge,   Bltu,  Bgeu,
}

impl BranchCond {
    // Total: an unrecognized funct3 defaults to `Beq` (the old `.unwrap_or(Beq)`).
    pub fn from_bits(f3: Bits<3>) -> Self {
        match f3.as_usize() {
            0x0 => BranchCond::Beq,
            0x1 => BranchCond::Bne,
            0x4 => BranchCond::Blt,
            0x5 => BranchCond::Bge,
            0x6 => BranchCond::Bltu,
            0x7 => BranchCond::Bgeu,
            _ => BranchCond::Beq,
        }
    }
}

pub struct InstrDecoded {
    pub opcode: Opcode,
    pub rd: usize,      pub rs1: usize,      pub rs2: usize,
    pub f3: Bits<3>,    pub f7: Bits<7>,
    pub imm_i: Bits<32>, pub imm_s: Bits<32>, pub imm_b: Bits<32>,
    pub imm_j: Bits<32>, pub imm_u: Bits<32>,
}

pub struct AluOutput {
    pub result: Bits<32>,
    pub overflow: bool,
    pub zero: bool,
    pub negative: bool,
}

fn sign_ext_i(instr: Bits<32>) -> Bits<32> {
    Bits::<32>::from_u32((instr.as_u32() as i32 >> 20) as u32)
}

fn sign_ext_s(instr: Bits<32>) -> Bits<32> {
    let w = instr.as_u32();
    let hi7 = (w >> 25) & 0x7F;
    let lo5 = (w >> 7) & 0x1F;
    Bits::<32>::from_u32(((((hi7 << 5) | lo5) as i32) << 20 >> 20) as u32)
}

fn sign_ext_b(instr: Bits<32>) -> Bits<32> {
    let w = instr.as_u32();
    let b12 = (w >> 31) & 1;
    let b11 = (w >> 7) & 1;
    let b10_5 = (w >> 25) & 0x3F;
    let b4_1 = (w >> 8) & 0xF;
    let raw = (b12 << 12) | (b11 << 11) | (b10_5 << 5) | (b4_1 << 1);
    Bits::<32>::from_u32(((raw as i32) << 19 >> 19) as u32)
}

fn sign_ext_j(instr: Bits<32>) -> Bits<32> {
    let w = instr.as_u32();
    let b20 = (w >> 31) & 1;
    let b10_1 = (w >> 21) & 0x3FF;
    let b11 = (w >> 20) & 1;
    let b19_12 = (w >> 12) & 0xFF;
    let raw = (b20 << 20) | (b19_12 << 12) | (b11 << 11) | (b10_1 << 1);
    Bits::<32>::from_u32(((raw as i32) << 11 >> 11) as u32)
}

// Total decode: `opcode` is `Opcode::INVALID` for an unrecognized instruction,
// which the module traps on. No `Option`/`?` — those have no hardware meaning.
fn decode(instr: Bits<32>) -> InstrDecoded {
    let w = instr.as_u32();
    let opcode = Opcode::from_bits(instr.truncate::<7>());
    InstrDecoded {
        opcode,
        rd:  ((w >> 7)  & 0x1F) as usize,
        rs1: ((w >> 15) & 0x1F) as usize,
        rs2: ((w >> 20) & 0x1F) as usize,
        f3: instr.part_select::<3>(12),
        f7: instr.part_select::<7>(25),
        imm_i: sign_ext_i(instr),
        imm_s: sign_ext_s(instr),
        imm_b: sign_ext_b(instr),
        imm_j: sign_ext_j(instr),
        imm_u: Bits::<32>::from_u32(w & 0xFFFF_F000),
    }
}

fn alu_exec_reg(a: Bits<32>, b: Bits<32>, f3: Bits<3>, f7: Bits<7>) -> AluOutput {
    let shamt = (b & Bits::<32>::from_lit::<0x1F>()).as_usize();
    let result = match (f3.as_usize(), f7.as_usize()) {
        (0x0, 0x00) => a + b,
        (0x0, 0x20) => a - b,
        (0x1, _)    => a << shamt,
        (0x2, _)    => if (a.as_u32() as i32) < (b.as_u32() as i32) { Bits::from_lit::<1>() } else { Bits::zero() },
        (0x3, _)    => if a.as_u32() < b.as_u32() { Bits::from_lit::<1>() } else { Bits::zero() },
        (0x4, _)    => a ^ b,
        (0x5, 0x00) => a >> shamt,
        (0x5, 0x20) => a.arithmetic_shift_right(shamt),
        (0x6, _)    => a | b,
        (0x7, _)    => a & b,
        _           => Bits::zero(),
    };
    AluOutput {
        result,
        overflow: false,
        zero: result == Bits::zero(),
        negative: result.get(31) == Logic::One,
    }
}

fn alu_exec_imm(a: Bits<32>, imm: Bits<32>, f3: Bits<3>, f7: Bits<7>) -> AluOutput {
    let shamt = (imm & Bits::<32>::from_lit::<0x1F>()).as_usize();
    let result = match f3.as_usize() {
        0x0 => a + imm,
        0x1 => a << shamt,
        0x2 => if (a.as_u32() as i32) < (imm.as_u32() as i32) { Bits::from_lit::<1>() } else { Bits::zero() },
        0x3 => if a.as_u32() < imm.as_u32() { Bits::from_lit::<1>() } else { Bits::zero() },
        0x4 => a ^ imm,
        0x5 => if f7.as_usize() & 0x20 != 0 { a.arithmetic_shift_right(shamt) } else { a >> shamt },
        0x6 => a | imm,
        0x7 => a & imm,
        _   => Bits::zero(),
    };
    AluOutput {
        result,
        overflow: false,
        zero: result == Bits::zero(),
        negative: result.get(31) == Logic::One,
    }
}

#[hardware(sequential)]
async fn rv32i_cpu(
    clk: Clock<MainClk>,
    program: In<Vec<Bits<32>>, MainClk>,
    program_counter: Out<Bits<32>, MainClk>,
    halted: Out<Logic, MainClk>,
    a0_out: Out<Bits<32>, MainClk>,
) {
    // Both imem and dmem are seeded from the same flat binary so that
    // .rodata constants (e.g. bubblesort's array at 0xC0) are reachable
    // via data loads, and the stack region (0xFD0-0xFFC) is zero-initialised.
    let flat = { let mut v = program.read(); v.resize(1024, Bits::zero()); v };
    let imem    = Memory::<Bits<32>, 1, 0, MainClk, 2, 1>::from_contents(clk.clone(), flat.clone());
    let dmem    = Memory::<Bits<32>, 1, 1, MainClk, 2, 1>::from_contents(clk.clone(), flat);
    let regfile = Memory::<Bits<32>, 2, 1, MainClk, 1, 1>::new(clk.clone(), 32);

    let mut pc: Bits<32> = Bits::zero();
    program_counter.write(pc);
    halted.write(Logic::Zero);
    a0_out.write(Bits::zero());

    loop {
        // Instruction fetch
        imem.read_port::<0>().read((pc >> 2).as_usize());
        loop {
            clk.tick().await;
            if imem.read_port::<0>().is_ready() { break; }
        }
        let instr: Bits<32> = imem.read_port::<0>().data();

        // Decode (total: `opcode` is `INVALID` for an unrecognized instruction)
        let decoded = decode(instr);

        match decoded.opcode {
            Opcode::LUI => {
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, decoded.imm_u);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() { break; }
                    }
                }
                program_counter.write(pc);
                halted.write(Logic::Zero);
                a0_out.write(Bits::zero());
                pc = pc + Bits::<32>::from_lit::<4>();
            }

            Opcode::AUIPC => {
                let result = pc + decoded.imm_u;
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, result);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() { break; }
                    }
                }
                program_counter.write(pc);
                halted.write(Logic::Zero);
                a0_out.write(Bits::zero());
                pc = pc + Bits::<32>::from_lit::<4>();
            }

            Opcode::JAL => {
                let link = pc + Bits::<32>::from_lit::<4>();
                let next = pc + decoded.imm_j;
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, link);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() { break; }
                    }
                }
                program_counter.write(pc);
                halted.write(Logic::Zero);
                a0_out.write(Bits::zero());
                pc = next;
            }

            Opcode::JALR => {
                regfile.read_port::<0>().read(decoded.rs1);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() { break; }
                }
                let rv1: Bits<32> = if decoded.rs1 == 0 { Bits::zero() } else { regfile.read_port::<0>().data() };
                let link = pc + Bits::<32>::from_lit::<4>();
                let next = (rv1 + decoded.imm_i) & !(Bits::<32>::from_lit::<1>());
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, link);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() { break; }
                    }
                }
                program_counter.write(pc);
                halted.write(Logic::Zero);
                a0_out.write(Bits::zero());
                pc = next;
            }

            Opcode::BRANCH => {
                regfile.read_port::<0>().read(decoded.rs1);
                regfile.read_port::<1>().read(decoded.rs2);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() && regfile.read_port::<1>().is_ready() { break; }
                }
                let rv1: Bits<32> = if decoded.rs1 == 0 { Bits::zero() } else { regfile.read_port::<0>().data() };
                let rv2: Bits<32> = if decoded.rs2 == 0 { Bits::zero() } else { regfile.read_port::<1>().data() };

                let branch_cond = BranchCond::from_bits(decoded.f3);
                let taken = match branch_cond {
                    BranchCond::Beq  => rv1 == rv2,
                    BranchCond::Bne  => rv1 != rv2,
                    BranchCond::Blt  => (rv1.as_u32() as i32) <  (rv2.as_u32() as i32),
                    BranchCond::Bge  => (rv1.as_u32() as i32) >= (rv2.as_u32() as i32),
                    BranchCond::Bltu => rv1.as_u32() <  rv2.as_u32(),
                    BranchCond::Bgeu => rv1.as_u32() >= rv2.as_u32(),
                };
                program_counter.write(pc);
                halted.write(Logic::Zero);
                a0_out.write(Bits::zero());
                pc = if taken { pc + decoded.imm_b } else { pc + Bits::<32>::from_lit::<4>() };
            }

            Opcode::LOAD => {
                regfile.read_port::<0>().read(decoded.rs1);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() { break; }
                }
                let rv1: Bits<32> = if decoded.rs1 == 0 { Bits::zero() } else { regfile.read_port::<0>().data() };
                let addr = ((rv1 + decoded.imm_i) >> 2).as_usize();
                dmem.read_port::<0>().read(addr);
                loop {
                    clk.tick().await;
                    if dmem.read_port::<0>().is_ready() { break; }
                }
                let loaded: Bits<32> = dmem.read_port::<0>().data();
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, loaded);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() { break; }
                    }
                }
                program_counter.write(pc);
                halted.write(Logic::Zero);
                a0_out.write(Bits::zero());
                pc = pc + Bits::<32>::from_lit::<4>();
            }

            Opcode::STORE => {
                regfile.read_port::<0>().read(decoded.rs1);
                regfile.read_port::<1>().read(decoded.rs2);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() && regfile.read_port::<1>().is_ready() { break; }
                }
                let rv1: Bits<32> = if decoded.rs1 == 0 { Bits::zero() } else { regfile.read_port::<0>().data() };
                let rv2: Bits<32> = if decoded.rs2 == 0 { Bits::zero() } else { regfile.read_port::<1>().data() };
                let addr = ((rv1 + decoded.imm_s) >> 2).as_usize();
                dmem.write_port::<0>().write(addr, rv2);
                loop {
                    clk.tick().await;
                    if dmem.write_port::<0>().is_ready() { break; }
                }
                program_counter.write(pc);
                halted.write(Logic::Zero);
                a0_out.write(Bits::zero());
                pc = pc + Bits::<32>::from_lit::<4>();
            }

            Opcode::ALU_IMM => {
                regfile.read_port::<0>().read(decoded.rs1);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() { break; }
                }
                let rv1: Bits<32> = if decoded.rs1 == 0 { Bits::zero() } else { regfile.read_port::<0>().data() };
                let alu_out = alu_exec_imm(rv1, decoded.imm_i, decoded.f3, decoded.f7);
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, alu_out.result);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() { break; }
                    }
                }
                program_counter.write(pc);
                halted.write(Logic::Zero);
                a0_out.write(Bits::zero());
                pc = pc + Bits::<32>::from_lit::<4>();
            }

            Opcode::ALU_REG => {
                regfile.read_port::<0>().read(decoded.rs1);
                regfile.read_port::<1>().read(decoded.rs2);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() && regfile.read_port::<1>().is_ready() { break; }
                }
                let rv1: Bits<32> = if decoded.rs1 == 0 { Bits::zero() } else { regfile.read_port::<0>().data() };
                let rv2: Bits<32> = if decoded.rs2 == 0 { Bits::zero() } else { regfile.read_port::<1>().data() };
                let alu_out = alu_exec_reg(rv1, rv2, decoded.f3, decoded.f7);
                if decoded.rd != 0 {
                    regfile.write_port::<0>().write(decoded.rd, alu_out.result);
                    loop {
                        clk.tick().await;
                        if regfile.write_port::<0>().is_ready() { break; }
                    }
                }
                program_counter.write(pc);
                halted.write(Logic::Zero);
                a0_out.write(Bits::zero());
                pc = pc + Bits::<32>::from_lit::<4>();
            }

            Opcode::ECALL => {
                regfile.read_port::<0>().read(10);
                loop {
                    clk.tick().await;
                    if regfile.read_port::<0>().is_ready() { break; }
                }
                let a0: Bits<32> = regfile.read_port::<0>().data();
                program_counter.write(pc);
                halted.write(Logic::One);
                a0_out.write(a0);
                loop { clk.tick().await; }
            }

            // Illegal instruction: trap by halting. This is the explicit-hardware
            // replacement for the old `None => panic!()` — an unrepresentable
            // opcode becomes an observable halt, not a simulation panic.
            Opcode::INVALID => {
                program_counter.write(pc);
                halted.write(Logic::One);
                a0_out.write(Bits::zero());
                loop { clk.tick().await; }
            }
        }
    }
}

// ── Execution Engine ───────────────────────────────────────────────────────────

fn run_program(program: Vec<u32>, max_cycles: usize) -> (u32, usize) {
    let program_bits: Vec<Bits<32>> = program.into_iter().map(Bits::<32>::from_u32).collect();
    let mut clk = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (prog_out, prog_in)      = wire::<Vec<Bits<32>>, MainClk>(vec![]);
    let (pc_out, _pc_in)         = wire::<Bits<32>, MainClk>(Bits::zero());
    let (halted_out, halted_in)  = wire::<Logic, MainClk>(Logic::Zero);
    let (a0_out, a0_in)          = wire::<Bits<32>, MainClk>(Bits::zero());

    prog_out.write(program_bits);
    let reads = vec![prog_in.wire_id()];
    exec.spawn_untracked(rv32i_cpu(clk.clone(), prog_in, pc_out, halted_out, a0_out), reads);

    for cycle in 1..=max_cycles {
        exec.tick_clock(&mut clk);
        if halted_in.read() == Logic::One {
            return (a0_in.read().as_u32(), cycle);
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

fn test_fibonacci() -> Vec<u32> {
    // Compute fib(10) = 55 iteratively
    // x1 = prev (fib(n-1)), x2 = curr (fib(n)), x3 = countdown
    // PC=0:  x1 = 0
    // PC=4:  x2 = 1
    // PC=8:  x3 = 10
    // PC=12: if x3 == 0 jump to done (PC=36)
    // PC=16: x4 = x1 + x2
    // PC=20: x1 = x2
    // PC=24: x2 = x4
    // PC=28: x3 -= 1
    // PC=32: jump to PC=12
    // PC=36: a0 = x1
    // PC=40: ecall
    vec![
        addi(1, 0, 0),       // PC=0:  x1 = 0
        addi(2, 0, 1),       // PC=4:  x2 = 1
        addi(3, 0, 10),      // PC=8:  x3 = 10 (iterations)
        beq(3, 0, 24),       // PC=12: if x3 == 0, jump to done (PC=36)
        add(4, 1, 2),        // PC=16: x4 = x1 + x2
        add(1, 2, 0),        // PC=20: x1 = x2
        add(2, 4, 0),        // PC=24: x2 = x4
        addi(3, 3, -1),      // PC=28: x3 -= 1
        beq(0, 0, -20),      // PC=32: jump back to loop (PC=12)
        add(10, 1, 0),       // PC=36: a0 = x1 = fib(10) = 55
        0x0000_0073,         // PC=40: ecall
    ]
}

fn test_bubblesort() -> Vec<u32> {
    // Bubble-sort of [64,34,25,12,22,11,90,42,8,55]; returns sum = 363.
    // Compiled: riscv64-unknown-elf-gcc -march=rv32i -mabi=ilp32 -nostdlib -O1
    // Flat binary: .text at 0x000, .rodata (array) at 0x0C0, stack at 0xFD0-0xFFC.
    // Memory is padded to 1024 words so the stack region is accessible.
    let mut mem = vec![0u32; 1024];
    let binary: &[u32] = &[
        // .text
        0x00001137, 0x008000ef, 0x00000073, 0xfd010113,
        0x0c000793, 0x0007ae03, 0x0047a303, 0x0087a883,
        0x00c7a803, 0x0107a503, 0x0147a583, 0x0187a603,
        0x01c7a683, 0x0207a703, 0x01c12423, 0x00612623,
        0x01112823, 0x01012a23, 0x00a12c23, 0x00b12e23,
        0x02c12023, 0x02d12223, 0x02e12423, 0x0247a783,
        0x02f12623, 0x00810593, 0x02c10613, 0x02c0006f,
        0x00478793, 0x00c78e63, 0x0007a703, 0x0047a683,
        0xfee6d8e3, 0x00d7a023, 0x00e7a223, 0xfe5ff06f,
        0xffc60613, 0x00b60663, 0x00058793, 0xfddff06f,
        0x02858713, 0x00000513, 0x0005a783, 0x00f50533,
        0x00458593, 0xfee59ae3, 0x03010113, 0x00008067,
        // [48] .rodata: {64, 34, 25, 12, 22, 11, 90, 42, 8, 55}
        0x00000040, 0x00000022, 0x00000019, 0x0000000c,
        0x00000016, 0x0000000b, 0x0000005a, 0x0000002a,
        0x00000008, 0x00000037,
    ];
    for (i, &w) in binary.iter().enumerate() { mem[i] = w; }
    mem
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
        ("Fibonacci: fib(10)", test_fibonacci(), 55),
        ("Bubblesort: sort+sum 10 elements", test_bubblesort(), 363),
    ];

    let mut passed = 0;
    let mut failed = 0;

    for (name, prog, expected) in tests {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_program(prog, 5000)
        })) {
            Ok((result, cycles)) => {
                if result == expected {
                    println!("  ✓ PASS: {} (a0 = {}, cycles = {})", name, result, cycles);
                    passed += 1;
                } else {
                    println!("  ✗ FAIL: {} (expected {}, got {}, cycles = {})", name, expected, result, cycles);
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
