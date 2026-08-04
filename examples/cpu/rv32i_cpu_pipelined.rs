// RV32I 5-stage pipelined CPU
//
// Pipeline: IF → ID → EX → MEM → WB
// Branch resolution: EX stage (2-cycle flush penalty)
// Load-use hazard: 1 stall cycle detected in EX/ID boundary
// Forwarding: EX/MEM → EX and MEM/WB → EX (EX/MEM has priority)
//
// Adapted from a MIPS pipeline reference to RV32I ISA.
// Instruction/data memory modelled as direct Vec indexing (no-latency)
// to keep pipeline control logic clean.

use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_core::port::{wire, In, Out};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

struct MainClk;
impl ClockDomain for MainClk {}

// ── Opcode constants ──────────────────────────────────────────────────────────

const OP_LUI:     u32 = 0x37;
const OP_AUIPC:   u32 = 0x17;
const OP_JAL:     u32 = 0x6F;
const OP_JALR:    u32 = 0x67;
const OP_BRANCH:  u32 = 0x63;
const OP_LOAD:    u32 = 0x03;
const OP_STORE:   u32 = 0x23;
const OP_ALU_IMM: u32 = 0x13;
const OP_ALU_REG: u32 = 0x33;
const OP_ECALL:   u32 = 0x73;

// ── Pipeline register types ───────────────────────────────────────────────────

// Fetch → Decode latch
#[derive(Clone, Copy)]
struct IFIDReg {
    valid: bool,
    pc:    Bits<32>,
    instr: Bits<32>,
}

impl IFIDReg {
    fn bubble() -> Self {
        Self { valid: false, pc: Bits::zero(), instr: Bits::zero() }
    }
}

// Decode → Execute latch
#[derive(Clone, Copy)]
struct IDEXReg {
    valid:     bool,
    pc:        Bits<32>,
    rs1:       usize,
    rs2:       usize,
    rd:        usize,
    rs1_val:   Bits<32>,
    rs2_val:   Bits<32>,
    imm_i:     Bits<32>,
    imm_s:     Bits<32>,
    imm_b:     Bits<32>,
    imm_j:     Bits<32>,
    imm_u:     Bits<32>,
    f3:        usize,
    f7:        usize,
    is_load:   bool,
    is_store:  bool,
    is_branch: bool,
    is_jal:    bool,
    is_jalr:   bool,
    is_lui:    bool,
    is_auipc:  bool,
    is_alu_imm: bool,
    is_ecall:  bool,
    writes_reg: bool,
}

impl IDEXReg {
    fn bubble() -> Self {
        Self {
            valid: false, pc: Bits::zero(),
            rs1: 0, rs2: 0, rd: 0,
            rs1_val: Bits::zero(), rs2_val: Bits::zero(),
            imm_i: Bits::zero(), imm_s: Bits::zero(), imm_b: Bits::zero(),
            imm_j: Bits::zero(), imm_u: Bits::zero(),
            f3: 0, f7: 0,
            is_load: false, is_store: false, is_branch: false,
            is_jal: false, is_jalr: false, is_lui: false, is_auipc: false,
            is_alu_imm: false, is_ecall: false, writes_reg: false,
        }
    }
}

// Execute → Memory latch
#[derive(Clone, Copy)]
struct EXMEMReg {
    valid:      bool,
    alu_result: Bits<32>,
    rs2_val:    Bits<32>,  // store data (forwarded in EX)
    rd:         usize,
    writes_reg: bool,
    is_load:    bool,
    is_store:   bool,
    is_ecall:   bool,
}

impl EXMEMReg {
    fn bubble() -> Self {
        Self {
            valid: false,
            alu_result: Bits::zero(), rs2_val: Bits::zero(),
            rd: 0, writes_reg: false,
            is_load: false, is_store: false, is_ecall: false,
        }
    }
}

// Memory → Writeback latch
#[derive(Clone, Copy)]
struct MEMWBReg {
    valid:      bool,
    result:     Bits<32>,  // ALU result or loaded value
    rd:         usize,
    writes_reg: bool,
    is_ecall:   bool,
}

impl MEMWBReg {
    fn bubble() -> Self {
        Self { valid: false, result: Bits::zero(), rd: 0, writes_reg: false, is_ecall: false }
    }
}

// ── Immediate decoders ────────────────────────────────────────────────────────

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
    let b12   = (w >> 31) & 1;
    let b11   = (w >> 7)  & 1;
    let b10_5 = (w >> 25) & 0x3F;
    let b4_1  = (w >> 8)  & 0xF;
    let raw = (b12 << 12) | (b11 << 11) | (b10_5 << 5) | (b4_1 << 1);
    Bits::<32>::from_u32(((raw as i32) << 19 >> 19) as u32)
}

fn sign_ext_j(instr: Bits<32>) -> Bits<32> {
    let w = instr.as_u32();
    let b20    = (w >> 31) & 1;
    let b10_1  = (w >> 21) & 0x3FF;
    let b11    = (w >> 20) & 1;
    let b19_12 = (w >> 12) & 0xFF;
    let raw = (b20 << 20) | (b19_12 << 12) | (b11 << 11) | (b10_1 << 1);
    Bits::<32>::from_u32(((raw as i32) << 11 >> 11) as u32)
}

// ── ALU ───────────────────────────────────────────────────────────────────────

fn alu(a: Bits<32>, b: Bits<32>, f3: usize, f7: usize, is_reg: bool) -> Bits<32> {
    let shamt = (b.as_u32() & 0x1F) as usize;
    match f3 {
        0x0 => if is_reg && (f7 & 0x20 != 0) { a - b } else { a + b },
        0x1 => a << shamt,
        0x2 => if (a.as_u32() as i32) <  (b.as_u32() as i32) { Bits::from_lit::<1>() } else { Bits::zero() },
        0x3 => if a.as_u32() < b.as_u32() { Bits::from_lit::<1>() } else { Bits::zero() },
        0x4 => a ^ b,
        0x5 => if f7 & 0x20 != 0 { a.arithmetic_shift_right(shamt) } else { a >> shamt },
        0x6 => a | b,
        0x7 => a & b,
        _   => Bits::zero(),
    }
}

fn branch_taken(rv1: Bits<32>, rv2: Bits<32>, f3: usize) -> bool {
    match f3 {
        0x0 => rv1 == rv2,
        0x1 => rv1 != rv2,
        0x4 => (rv1.as_u32() as i32) <  (rv2.as_u32() as i32),
        0x5 => (rv1.as_u32() as i32) >= (rv2.as_u32() as i32),
        0x6 => rv1.as_u32() <  rv2.as_u32(),
        0x7 => rv1.as_u32() >= rv2.as_u32(),
        _   => false,
    }
}

// ── CPU ───────────────────────────────────────────────────────────────────────

#[hardware(sequential)]
async fn rv32i_cpu_pipelined(
    clk: Clock<MainClk>,
    program: In<Vec<Bits<32>>, MainClk>,
    program_counter: Out<Bits<32>, MainClk>,
    halted: Out<Logic, MainClk>,
    a0_out: Out<Bits<32>, MainClk>,
) {
    // Unified address space: instruction fetch and data loads/stores all hit
    // the same Vec.  run_program pads it to 1024 words so the stack region
    // (sp=0x1000, growing down) and any .rodata constants have room.
    let mut memory = { let mut v = program.read(); v.resize(1024, Bits::zero()); v };
    let mut regs = [Bits::<32>::zero(); 32];

    let mut pc: Bits<32> = Bits::zero();

    let mut if_id  = IFIDReg::bubble();
    let mut id_ex  = IDEXReg::bubble();
    let mut ex_mem = EXMEMReg::bubble();
    let mut mem_wb = MEMWBReg::bubble();

    program_counter.write(pc);
    halted.write(Logic::Zero);
    a0_out.write(Bits::zero());

    loop {
        clk.tick().await;

        // ── WB ──────────────────────────────────────────────────────────────
        if mem_wb.valid && mem_wb.is_ecall {
            a0_out.write(regs[10]);
            halted.write(Logic::One);
            program_counter.write(pc);
            loop { clk.tick().await; }
        }
        if mem_wb.valid && mem_wb.writes_reg && mem_wb.rd != 0 {
            regs[mem_wb.rd] = mem_wb.result;
        }

        // ── MEM ─────────────────────────────────────────────────────────────
        let mem_result: Bits<32> = if ex_mem.valid && ex_mem.is_load {
            let addr = (ex_mem.alu_result >> 2).as_usize();
            if addr < memory.len() { memory[addr] } else { Bits::zero() }
        } else {
            Bits::zero()
        };
        if ex_mem.valid && ex_mem.is_store {
            let addr = (ex_mem.alu_result >> 2).as_usize();
            if addr < memory.len() { memory[addr] = ex_mem.rs2_val; }
        }
        let new_mem_wb = if ex_mem.valid {
            MEMWBReg {
                valid: true,
                result: if ex_mem.is_load { mem_result } else { ex_mem.alu_result },
                rd: ex_mem.rd,
                writes_reg: ex_mem.writes_reg,
                is_ecall: ex_mem.is_ecall,
            }
        } else {
            MEMWBReg::bubble()
        };

        // ── Forwarding unit ─────────────────────────────────────────────────
        // Priority: EX/MEM (newest) > MEM/WB (older), matching the reference.
        // EX/MEM forwarding is skipped for loads: the loaded value is only
        // available after MEM, so the hazard unit stalls in advance.
        let fwd_rs1 = {
            let from_ex_mem = ex_mem.valid && ex_mem.writes_reg && !ex_mem.is_load
                              && ex_mem.rd != 0 && ex_mem.rd == id_ex.rs1;
            let from_mem_wb = mem_wb.valid && mem_wb.writes_reg
                              && mem_wb.rd != 0 && mem_wb.rd == id_ex.rs1;
            if from_ex_mem      { ex_mem.alu_result }
            else if from_mem_wb { mem_wb.result }
            else                { id_ex.rs1_val }
        };
        let fwd_rs2 = {
            let from_ex_mem = ex_mem.valid && ex_mem.writes_reg && !ex_mem.is_load
                              && ex_mem.rd != 0 && ex_mem.rd == id_ex.rs2;
            let from_mem_wb = mem_wb.valid && mem_wb.writes_reg
                              && mem_wb.rd != 0 && mem_wb.rd == id_ex.rs2;
            if from_ex_mem      { ex_mem.alu_result }
            else if from_mem_wb { mem_wb.result }
            else                { id_ex.rs2_val }
        };

        // ── EX ──────────────────────────────────────────────────────────────
        // Returns (new_ex_mem, flush, branch_target).
        // flush = true for JAL, JALR, and taken branches → 2-cycle penalty.
        let (new_ex_mem, flush, branch_target) = if !id_ex.valid {
            (EXMEMReg::bubble(), false, Bits::zero())
        } else if id_ex.is_lui {
            (EXMEMReg { valid: true, alu_result: id_ex.imm_u, rs2_val: Bits::zero(),
                        rd: id_ex.rd, writes_reg: id_ex.writes_reg,
                        is_load: false, is_store: false, is_ecall: false },
             false, Bits::zero())
        } else if id_ex.is_auipc {
            (EXMEMReg { valid: true, alu_result: id_ex.pc + id_ex.imm_u, rs2_val: Bits::zero(),
                        rd: id_ex.rd, writes_reg: id_ex.writes_reg,
                        is_load: false, is_store: false, is_ecall: false },
             false, Bits::zero())
        } else if id_ex.is_jal {
            let link = id_ex.pc + Bits::<32>::from_lit::<4>();
            let tgt  = id_ex.pc + id_ex.imm_j;
            (EXMEMReg { valid: true, alu_result: link, rs2_val: Bits::zero(),
                        rd: id_ex.rd, writes_reg: id_ex.writes_reg,
                        is_load: false, is_store: false, is_ecall: false },
             true, tgt)
        } else if id_ex.is_jalr {
            let link = id_ex.pc + Bits::<32>::from_lit::<4>();
            let tgt  = (fwd_rs1 + id_ex.imm_i) & !(Bits::<32>::from_lit::<1>());
            (EXMEMReg { valid: true, alu_result: link, rs2_val: Bits::zero(),
                        rd: id_ex.rd, writes_reg: id_ex.writes_reg,
                        is_load: false, is_store: false, is_ecall: false },
             true, tgt)
        } else if id_ex.is_branch {
            let taken = branch_taken(fwd_rs1, fwd_rs2, id_ex.f3);
            let tgt   = id_ex.pc + id_ex.imm_b;
            (EXMEMReg { valid: true, alu_result: Bits::zero(), rs2_val: Bits::zero(),
                        rd: 0, writes_reg: false,
                        is_load: false, is_store: false, is_ecall: false },
             taken, tgt)
        } else if id_ex.is_load {
            let addr = fwd_rs1 + id_ex.imm_i;
            (EXMEMReg { valid: true, alu_result: addr, rs2_val: Bits::zero(),
                        rd: id_ex.rd, writes_reg: id_ex.writes_reg,
                        is_load: true, is_store: false, is_ecall: false },
             false, Bits::zero())
        } else if id_ex.is_store {
            let addr = fwd_rs1 + id_ex.imm_s;
            (EXMEMReg { valid: true, alu_result: addr, rs2_val: fwd_rs2,
                        rd: 0, writes_reg: false,
                        is_load: false, is_store: true, is_ecall: false },
             false, Bits::zero())
        } else if id_ex.is_ecall {
            (EXMEMReg { valid: true, alu_result: Bits::zero(), rs2_val: Bits::zero(),
                        rd: 0, writes_reg: false,
                        is_load: false, is_store: false, is_ecall: true },
             false, Bits::zero())
        } else {
            // ALU_IMM or ALU_REG
            let b      = if id_ex.is_alu_imm { id_ex.imm_i } else { fwd_rs2 };
            let result = alu(fwd_rs1, b, id_ex.f3, id_ex.f7, !id_ex.is_alu_imm);
            (EXMEMReg { valid: true, alu_result: result, rs2_val: Bits::zero(),
                        rd: id_ex.rd, writes_reg: id_ex.writes_reg,
                        is_load: false, is_store: false, is_ecall: false },
             false, Bits::zero())
        };

        // ── Load-use hazard detection ────────────────────────────────────────
        // Detected at the EX/ID boundary: id_ex is a load and the instruction
        // currently in ID (if_id) reads the register id_ex will produce.
        // Resolution: stall PC and IF/ID, flush ID/EX (insert bubble into EX).
        let if_id_rs1 = ((if_id.instr.as_u32() >> 15) & 0x1F) as usize;
        let if_id_rs2 = ((if_id.instr.as_u32() >> 20) & 0x1F) as usize;
        let load_use_stall = id_ex.valid && id_ex.is_load && if_id.valid
            && id_ex.rd != 0
            && (id_ex.rd == if_id_rs1 || id_ex.rd == if_id_rs2);

        // ── ID ──────────────────────────────────────────────────────────────
        let new_id_ex = if !if_id.valid || flush || load_use_stall {
            IDEXReg::bubble()
        } else {
            let instr = if_id.instr;
            let w     = instr.as_u32();
            let op    = w & 0x7F;
            let rd    = ((w >>  7) & 0x1F) as usize;
            let rs1   = ((w >> 15) & 0x1F) as usize;
            let rs2   = ((w >> 20) & 0x1F) as usize;
            let f3    = ((w >> 12) & 0x7) as usize;
            let f7    = ((w >> 25) & 0x7F) as usize;

            let rs1_val = if rs1 == 0 { Bits::zero() } else { regs[rs1] };
            let rs2_val = if rs2 == 0 { Bits::zero() } else { regs[rs2] };

            let writes_reg = match op {
                OP_LUI | OP_AUIPC | OP_JAL | OP_JALR
                | OP_LOAD | OP_ALU_IMM | OP_ALU_REG => rd != 0,
                _ => false,
            };

            IDEXReg {
                valid: true,
                pc: if_id.pc,
                rs1, rs2, rd,
                rs1_val, rs2_val,
                imm_i: sign_ext_i(instr),
                imm_s: sign_ext_s(instr),
                imm_b: sign_ext_b(instr),
                imm_j: sign_ext_j(instr),
                imm_u: Bits::<32>::from_u32(w & 0xFFFF_F000),
                f3, f7,
                is_load:    op == OP_LOAD,
                is_store:   op == OP_STORE,
                is_branch:  op == OP_BRANCH,
                is_jal:     op == OP_JAL,
                is_jalr:    op == OP_JALR,
                is_lui:     op == OP_LUI,
                is_auipc:   op == OP_AUIPC,
                is_alu_imm: op == OP_ALU_IMM,
                is_ecall:   op == OP_ECALL,
                writes_reg,
            }
        };

        // ── IF ──────────────────────────────────────────────────────────────
        // On flush: discard the speculatively-fetched instruction (wrong path).
        // On load-use stall: hold the current if_id (re-process it next cycle).
        let new_if_id = if flush {
            IFIDReg::bubble()
        } else if load_use_stall {
            if_id  // hold
        } else {
            let idx   = (pc >> 2).as_usize();
            let instr = if idx < memory.len() { memory[idx] } else { Bits::zero() };
            IFIDReg { valid: true, pc, instr }
        };

        // ── PC update ───────────────────────────────────────────────────────
        let new_pc = if flush {
            branch_target
        } else if load_use_stall {
            pc
        } else {
            pc + Bits::<32>::from_lit::<4>()
        };

        // ── Commit ──────────────────────────────────────────────────────────
        pc     = new_pc;
        if_id  = new_if_id;
        id_ex  = new_id_ex;
        ex_mem = new_ex_mem;
        mem_wb = new_mem_wb;

        program_counter.write(pc);
    }
}

// ── Execution engine ──────────────────────────────────────────────────────────

fn run_program(program: Vec<u32>, max_cycles: usize) -> (u32, usize) {
    let program_bits: Vec<Bits<32>> = program.into_iter().map(Bits::<32>::from_u32).collect();
    let mut clk  = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (prog_out, prog_in)     = wire::<Vec<Bits<32>>, MainClk>(vec![]);
    let (pc_out,  _pc_in)       = wire::<Bits<32>, MainClk>(Bits::zero());
    let (halt_out, halt_in)     = wire::<Logic, MainClk>(Logic::Zero);
    let (a0_out,  a0_in)        = wire::<Bits<32>, MainClk>(Bits::zero());

    prog_out.write(program_bits);
    let reads = vec![prog_in.wire_id()];
    exec.spawn_untracked(rv32i_cpu_pipelined(clk.clone(), prog_in, pc_out, halt_out, a0_out), reads);

    for cycle in 1..=max_cycles {
        exec.tick_clock(&mut clk);
        if halt_in.read() == Logic::One {
            return (a0_in.read().as_u32(), cycle);
        }
    }
    panic!("Program did not halt within {} cycles", max_cycles);
}

// ── Assembler helpers ─────────────────────────────────────────────────────────

fn i_type(rd: u32, rs1: u32, imm: i32, f3: u32, opcode: u32) -> u32 {
    ((imm as u32 & 0xFFF) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | opcode
}

fn r_type(rd: u32, rs1: u32, rs2: u32, f3: u32, f7: u32, opcode: u32) -> u32 {
    (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | opcode
}

fn b_type(rs1: u32, rs2: u32, offset: i32, f3: u32) -> u32 {
    let o = offset as u32;
    let b12   = (o >> 12) & 1;
    let b11   = (o >> 11) & 1;
    let b10_5 = (o >> 5)  & 0x3F;
    let b4_1  = (o >> 1)  & 0xF;
    (b12 << 31) | (b10_5 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12)
        | (b4_1 << 8) | (b11 << 7) | 0x63
}

fn s_type(rs1: u32, rs2: u32, offset: i32, f3: u32) -> u32 {
    let imm12 = offset as u32 & 0xFFF;
    ((imm12 >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12)
        | ((imm12 & 0x1F) << 7) | 0x23
}

fn j_type(rd: u32, offset: i32) -> u32 {
    let o = offset as u32;
    let b20    = (o >> 20) & 1;
    let b10_1  = (o >> 1)  & 0x3FF;
    let b11    = (o >> 11) & 1;
    let b19_12 = (o >> 12) & 0xFF;
    (b20 << 31) | (b19_12 << 12) | (b11 << 20) | (b10_1 << 21) | (rd << 7) | OP_JAL
}

fn addi(rd: u32, rs1: u32, imm: i32) -> u32 { i_type(rd, rs1, imm, 0x0, 0x13) }
fn add (rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(rd, rs1, rs2, 0x0, 0x00, 0x33) }
fn sub (rd: u32, rs1: u32, rs2: u32) -> u32 { r_type(rd, rs1, rs2, 0x0, 0x20, 0x33) }
fn beq (rs1: u32, rs2: u32, off: i32) -> u32 { b_type(rs1, rs2, off, 0x0) }
fn bne (rs1: u32, rs2: u32, off: i32) -> u32 { b_type(rs1, rs2, off, 0x1) }
fn blt (rs1: u32, rs2: u32, off: i32) -> u32 { b_type(rs1, rs2, off, 0x4) }
fn jal (rd: u32, off: i32) -> u32 { j_type(rd, off) }
fn lw  (rd: u32, rs1: u32, imm: i32) -> u32 { i_type(rd, rs1, imm, 0x2, 0x03) }
fn sw  (rs1: u32, rs2: u32, off: i32) -> u32 { s_type(rs1, rs2, off, 0x2) }

// ── Test programs ─────────────────────────────────────────────────────────────

fn test_addi() -> Vec<u32> {
    vec![
        addi(1, 0, 10),
        addi(2, 0, 5),
        add(10, 1, 2),
        0x0000_0073,
    ]
}

fn test_sub() -> Vec<u32> {
    vec![
        addi(1, 0, 15),
        addi(2, 0, 10),
        sub(10, 1, 2),
        0x0000_0073,
    ]
}

fn test_multiple_adds() -> Vec<u32> {
    vec![
        addi(1, 0, 0),
        addi(1, 1, 1),
        addi(1, 1, 2),
        addi(1, 1, 3),
        addi(1, 1, 4),
        addi(1, 1, 5),
        addi(10, 1, 0),
        0x0000_0073,
    ]
}

fn test_branch_taken() -> Vec<u32> {
    // PC=0:  addi x1,x0,1
    // PC=4:  beq  x0,x0,+12  → jump to PC=16
    // PC=8:  addi x10,x0,1   (skipped)
    // PC=12: ecall            (skipped)
    // PC=16: addi x10,x0,42
    // PC=20: ecall
    vec![
        addi(1, 0, 1),
        beq(0, 0, 12),
        addi(10, 0, 1),
        0x0000_0073,
        addi(10, 0, 42),
        0x0000_0073,
    ]
}

fn test_branch_not_taken() -> Vec<u32> {
    vec![
        addi(1, 0, 5),
        addi(2, 0, 10),
        beq(1, 2, 8),
        addi(10, 0, 99),
        0x0000_0073,
    ]
}

fn test_load_store() -> Vec<u32> {
    vec![
        addi(1, 0, 88),
        sw(0, 1, 0),
        lw(10, 0, 0),
        0x0000_0073,
    ]
}

fn test_negative_numbers() -> Vec<u32> {
    vec![
        addi(1, 0, 10),
        addi(2, 0, -3),
        add(10, 1, 2),
        0x0000_0073,
    ]
}

fn test_zero_operations() -> Vec<u32> {
    vec![
        addi(1, 0, 0),
        addi(2, 0, 42),
        add(10, 1, 2),
        0x0000_0073,
    ]
}

fn test_fibonacci() -> Vec<u32> {
    // Compute fib(10) = 55 iteratively
    // x1=prev, x2=curr, x3=countdown
    // PC=0:  x1 = 0
    // PC=4:  x2 = 1
    // PC=8:  x3 = 10
    // PC=12: if x3==0, jump to done (PC=36)
    // PC=16: x4 = x1+x2
    // PC=20: x1 = x2
    // PC=24: x2 = x4
    // PC=28: x3 -= 1
    // PC=32: jump to PC=12
    // PC=36: a0 = x1
    // PC=40: ecall
    vec![
        addi(1, 0, 0),
        addi(2, 0, 1),
        addi(3, 0, 10),
        beq(3, 0, 24),
        add(4, 1, 2),
        add(1, 2, 0),
        add(2, 4, 0),
        addi(3, 3, -1),
        beq(0, 0, -20),
        add(10, 1, 0),
        0x0000_0073,
    ]
}

fn test_jal() -> Vec<u32> {
    // PC=0: addi x1,x0,7
    // PC=4: jal  x0,+8   → jump to PC=12
    // PC=8: addi x1,x0,0 (skipped)
    // PC=12: addi x10,x1,0
    // PC=16: ecall
    vec![
        addi(1, 0, 7),
        jal(0, 8),
        addi(1, 0, 0),
        addi(10, 1, 0),
        0x0000_0073,
    ]
}

fn test_data_hazard_forwarding() -> Vec<u32> {
    // Back-to-back RAW: each addi reads the register written by the previous one.
    // Forwarding must supply the value without stalls.
    // x1=1, x1=x1+1=2, x1=x1+1=3 → a0=3
    vec![
        addi(1, 0, 1),
        addi(1, 1, 1),
        addi(1, 1, 1),
        addi(10, 1, 0),
        0x0000_0073,
    ]
}

fn test_load_use_stall() -> Vec<u32> {
    // lw immediately followed by a use of the loaded register.
    // Hazard unit must insert a stall bubble so forwarding from MEM/WB works.
    // Store 42 to dmem[0], load it back, add 1.
    vec![
        addi(1, 0, 42),
        sw(0, 1, 0),
        lw(2, 0, 0),
        addi(10, 2, 1),  // depends on lw → load-use stall
        0x0000_0073,
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

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║          RV32I CPU - 5-Stage Pipelined - Test Suite           ║");
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    let tests: Vec<(&str, Vec<u32>, u32)> = vec![
        ("ADDI: simple addition",             test_addi(),                   15),
        ("SUB: subtraction",                  test_sub(),                     5),
        ("Multiple ADDIs: 1+2+3+4+5",         test_multiple_adds(),          15),
        ("BEQ: branch taken",                 test_branch_taken(),           42),
        ("BEQ: branch not taken",             test_branch_not_taken(),       99),
        ("Load/Store: word round-trip",       test_load_store(),             88),
        ("Negative: 10 + (-3)",               test_negative_numbers(),        7),
        ("Zero: 0 + 42",                      test_zero_operations(),        42),
        ("JAL: unconditional jump",           test_jal(),                     7),
        ("Forwarding: back-to-back RAW",      test_data_hazard_forwarding(),  3),
        ("Load-use stall",                    test_load_use_stall(),         43),
        ("Fibonacci: fib(10)",                test_fibonacci(),              55),
        ("Bubblesort: sort+sum 10 elements", test_bubblesort(),            363),
    ];

    let mut passed = 0;
    let mut failed = 0;

    for (name, prog, expected) in tests {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_program(prog, 1000)
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
