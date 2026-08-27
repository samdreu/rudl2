// RV32I 5-stage pipelined CPU — the TRANSPILABLE spelling.
//
// Same design as `rv32i_cpu_pipelined.rs`: IF → ID → EX → MEM → WB, branches
// resolved in EX with a 2-cycle flush, a 1-cycle load-use stall, and EX/MEM →
// EX / MEM/WB → EX forwarding. What differs is only HOW it is written. Every
// difference from its readable sibling is here because the transpiler rejects
// the natural spelling or, worse, accepts it and emits something else:
//
//   struct pipeline latches ........ scalarized into individual locals; a struct
//                                    is not a hardware type.
//   `let (a, b, c) = if …`.......... three TOTAL if-chain EXPRESSIONS. A tuple is
//                                    not a hardware type — and the obvious
//                                    workaround, `let mut a; if … { a = … }`, is
//                                    a TRAP: a conditionally-assigned mutable
//                                    local lowers to a HELD REGISTER, so it keeps
//                                    last cycle's value on the paths that do not
//                                    assign, while the simulator re-initialises it
//                                    every iteration. Only a total expression is
//                                    combinational.
//   `regs[rd] = v` .................. 31 named scalars with an if-chain demux.
//                                    Element-wise assignment to an array local is
//                                    refused ("would infer a latch"), and there is
//                                    no other spelling: array literals are
//                                    unsupported and `[x; N]` collapses the array
//                                    dimension. Array READS are fine, but an array
//                                    you cannot write cannot hold state.
//   `match op { OP_LUI | … }` ....... if-chains. A named `const` in a match pattern
//                                    is refused, and a `match` on a `usize` emits
//                                    64-bit case literals (WIDTHEXPAND).
//   `x as i32` ...................... gone. Casts are STRIPPED and signedness is
//                                    never emitted, so `(a as i32) < (b as i32)`
//                                    became an unsigned compare and
//                                    `as i32 >> 20` became a logical shift. Signed
//                                    compares bias by 0x8000_0000; sign extension
//                                    is written out of mask-and-OR.
//   `!bits` ......................... gone. `!` on a `Bits<N>` emits SystemVerilog
//                                    `!` (LOGICAL not) rather than `~`, so the
//                                    JALR alignment mask `!1` became `1'b0` and
//                                    every JALR target became 0. Written as an
//                                    explicit mask instead. (`!` on a `bool` is
//                                    one bit and is correct — only `Bits` is wrong.)
//   `arithmetic_shift_right` ........ built from `>>` and a sign-fill mask.
//   the `Memory` parameter .......... declared in-module. A received memory has no
//                                    port ABI yet (TODO cause P), so the program
//                                    arrives through a boot interface instead:
//                                    hold `rstn` low and write it in a word at a
//                                    time. That is also what makes this module
//                                    resettable, and therefore sweepable.
//
// The one thing that could NOT be fixed here is the memory ADDRESS WIDTH: the
// address nets are sized to the memory (10 bits for 1024 words) and every index
// is `Bits<32>`-derived, so the assignment truncates and Verilator's `-Wall`
// rejects it. No source spelling avoids it — `truncate` and `part_select` are not
// supported methods, and a `Bits<10>` local still takes a 32-bit right-hand side.
// It needs a width cast in `vlir_lower`, or a lowering for `truncate`. Until then
// this module transpiles but does not Verilate, which is why `build.rs` still
// skips it — and why that SKIP entry names a codegen fix rather than a language
// gap.

use copper_core::{Bits, Clock, ClockDomain, Logic, Memory};
use copper_core::port::{wire, In, Out};
use copper_macros::hardware;
use copper_sim::HardwareExecutor;

pub struct MainClk;
impl ClockDomain for MainClk {}

// Words in the unified instruction/data memory.
pub const MEM_WORDS: usize = 1024;

// ── Opcodes ───────────────────────────────────────────────────────────────────
// Compared with `==` rather than matched: a named const in a match PATTERN is
// refused by the transpiler (it reads as an enum variant), while a const in an
// EXPRESSION lowers to a localparam and is fine.

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

// ── Arithmetic written without signed types ───────────────────────────────────

/// Signed `<`. Flipping the sign bit maps two's-complement order onto unsigned
/// order, so the comparison the transpiler actually emits is the right one.
fn slt(a: Bits<32>, b: Bits<32>) -> bool {
    let bias: Bits<32> = Bits::<32>::from_u32(0x8000_0000);
    (a ^ bias).as_u32() < (b ^ bias).as_u32()
}

/// Arithmetic shift right. `ones ^ (ones >> n)` is the high-n-bits mask — the
/// bitwise NOT of `ones >> n`, spelled as an XOR because `!` on a `Bits` lowers
/// to a logical negation.
fn sra(v: Bits<32>, n: usize) -> Bits<32> {
    let logical: Bits<32> = v >> n;
    let ones: Bits<32> = Bits::<32>::from_u32(0xFFFF_FFFF);
    let neg: bool = ((v >> 31) & Bits::<32>::from_u32(1)) == Bits::<32>::from_u32(1);
    if neg && n != 0 { logical | (ones ^ (ones >> n)) } else { logical }
}

/// Sign-extend `raw`, whose sign bit is `sign_bit`, by OR-ing in `fill`.
fn sext(raw: Bits<32>, sign: Bits<32>, fill: Bits<32>) -> Bits<32> {
    if sign == Bits::<32>::from_u32(1) { raw | fill } else { raw }
}

fn imm_i(instr: Bits<32>) -> Bits<32> {
    let raw: Bits<32> = (instr >> 20) & Bits::<32>::from_u32(0xFFF);
    let s: Bits<32> = (instr >> 31) & Bits::<32>::from_u32(1);
    sext(raw, s, Bits::<32>::from_u32(0xFFFF_F000))
}

fn imm_s(instr: Bits<32>) -> Bits<32> {
    let hi7: Bits<32> = (instr >> 25) & Bits::<32>::from_u32(0x7F);
    let lo5: Bits<32> = (instr >>  7) & Bits::<32>::from_u32(0x1F);
    let raw: Bits<32> = (hi7 << 5) | lo5;
    let s: Bits<32> = (instr >> 31) & Bits::<32>::from_u32(1);
    sext(raw, s, Bits::<32>::from_u32(0xFFFF_F000))
}

fn imm_b(instr: Bits<32>) -> Bits<32> {
    let b12:   Bits<32> = (instr >> 31) & Bits::<32>::from_u32(1);
    let b11:   Bits<32> = (instr >>  7) & Bits::<32>::from_u32(1);
    let b10_5: Bits<32> = (instr >> 25) & Bits::<32>::from_u32(0x3F);
    let b4_1:  Bits<32> = (instr >>  8) & Bits::<32>::from_u32(0xF);
    let raw: Bits<32> = (b12 << 12) | (b11 << 11) | (b10_5 << 5) | (b4_1 << 1);
    sext(raw, b12, Bits::<32>::from_u32(0xFFFF_E000))
}

fn imm_j(instr: Bits<32>) -> Bits<32> {
    let b20:    Bits<32> = (instr >> 31) & Bits::<32>::from_u32(1);
    let b10_1:  Bits<32> = (instr >> 21) & Bits::<32>::from_u32(0x3FF);
    let b11:    Bits<32> = (instr >> 20) & Bits::<32>::from_u32(1);
    let b19_12: Bits<32> = (instr >> 12) & Bits::<32>::from_u32(0xFF);
    let raw: Bits<32> = (b20 << 20) | (b19_12 << 12) | (b11 << 11) | (b10_1 << 1);
    sext(raw, b20, Bits::<32>::from_u32(0xFFE0_0000))
}

fn alu(a: Bits<32>, b: Bits<32>, f3: usize, f7: usize, is_reg: bool) -> Bits<32> {
    let shamt: usize = (b.as_u32() & 0x1F) as usize;
    let one: Bits<32> = Bits::<32>::from_u32(1);
    let zero: Bits<32> = Bits::zero();
    let alt: bool = (f7 & 0x20) != 0;
    if f3 == 0 {
        if is_reg && alt { a - b } else { a + b }
    } else if f3 == 1 {
        a << shamt
    } else if f3 == 2 {
        if slt(a, b) { one } else { zero }
    } else if f3 == 3 {
        if a.as_u32() < b.as_u32() { one } else { zero }
    } else if f3 == 4 {
        a ^ b
    } else if f3 == 5 {
        if alt { sra(a, shamt) } else { a >> shamt }
    } else if f3 == 6 {
        a | b
    } else if f3 == 7 {
        a & b
    } else {
        zero
    }
}

fn branch_taken(rv1: Bits<32>, rv2: Bits<32>, f3: usize) -> bool {
    if f3 == 0 { rv1 == rv2 }
    else if f3 == 1 { rv1 != rv2 }
    else if f3 == 4 { slt(rv1, rv2) }
    else if f3 == 5 { !slt(rv1, rv2) }
    else if f3 == 6 { rv1.as_u32() < rv2.as_u32() }
    else if f3 == 7 { !(rv1.as_u32() < rv2.as_u32()) }
    else { false }
}

// ── CPU ───────────────────────────────────────────────────────────────────────

#[hardware(sequential)]
async fn rv32i_cpu_transpilable(
    clk: Clock<MainClk>,
    // Active-low reset. Held low, the core is idle and the boot port owns the
    // memory's write bus; released, it runs. This is also what makes the module
    // resettable — a design whose state is X until reset cannot be swept.
    rstn: In<Logic, MainClk>,
    boot_en: In<Logic, MainClk>,
    boot_addr: In<Bits<32>, MainClk>,
    boot_data: In<Bits<32>, MainClk>,
    // A plain `Out`, and it has to be WRITTEN IN THE TRAILING SEGMENT — see the
    // note where it is written. Both halves were found by the sweep, in that order.
    program_counter: Out<Bits<32>, MainClk>,
    halted: Out<Logic, MainClk>,
    a0_out: Out<Bits<32>, MainClk>,
) {
    // Unified instruction/data memory. Read port 0 = fetch, read port 1 = load,
    // write port 0 = store (and the boot loader while `rstn` is low; the two are
    // on exclusive paths, which the one-bus rule reads as a mux, not a conflict).
    // WriteFirst so a store is visible to a fetch of the same word in the same
    // cycle, matching the `Vec` version's statement order.
    let memory = Memory::<Bits<32>, 2, 1, MainClk, 1, 1>::new(clk.clone(), 1024).write_first();

    let mut pc: Bits<32> = Bits::zero();
    let mut halted_r: bool = false;
    let mut a0_r: Bits<32> = Bits::zero();

    // IF/ID. `if_id_instr` is a real register: the memory's output register holds
    // the instruction being FETCHED this cycle, which is one ahead of the one
    // being DECODED. Collapsing the two would lose a pipeline stage.
    let mut if_id_valid: bool = false;
    let mut if_id_pc: Bits<32> = Bits::zero();
    let mut if_id_instr: Bits<32> = Bits::zero();

    // ID/EX
    let mut id_ex_valid: bool = false;
    let mut id_ex_pc: Bits<32> = Bits::zero();
    let mut id_ex_rs1: usize = 0;
    let mut id_ex_rs2: usize = 0;
    let mut id_ex_rd: usize = 0;
    let mut id_ex_rs1_val: Bits<32> = Bits::zero();
    let mut id_ex_rs2_val: Bits<32> = Bits::zero();
    let mut id_ex_imm_i: Bits<32> = Bits::zero();
    let mut id_ex_imm_s: Bits<32> = Bits::zero();
    let mut id_ex_imm_b: Bits<32> = Bits::zero();
    let mut id_ex_imm_j: Bits<32> = Bits::zero();
    let mut id_ex_imm_u: Bits<32> = Bits::zero();
    let mut id_ex_f3: usize = 0;
    let mut id_ex_f7: usize = 0;
    let mut id_ex_is_load: bool = false;
    let mut id_ex_is_store: bool = false;
    let mut id_ex_is_branch: bool = false;
    let mut id_ex_is_jal: bool = false;
    let mut id_ex_is_jalr: bool = false;
    let mut id_ex_is_lui: bool = false;
    let mut id_ex_is_auipc: bool = false;
    let mut id_ex_is_alu_imm: bool = false;
    let mut id_ex_is_ecall: bool = false;
    let mut id_ex_writes_reg: bool = false;

    // EX/MEM
    let mut ex_mem_valid: bool = false;
    let mut ex_mem_alu: Bits<32> = Bits::zero();
    let mut ex_mem_rs2v: Bits<32> = Bits::zero();
    let mut ex_mem_rd: usize = 0;
    let mut ex_mem_writes_reg: bool = false;
    let mut ex_mem_is_load: bool = false;
    let mut ex_mem_is_store: bool = false;
    let mut ex_mem_is_ecall: bool = false;

    // MEM/WB
    let mut mem_wb_valid: bool = false;
    let mut mem_wb_result: Bits<32> = Bits::zero();
    let mut mem_wb_rd: usize = 0;
    let mut mem_wb_writes_reg: bool = false;
    let mut mem_wb_is_ecall: bool = false;

    // Architectural registers. x0 is not stored: it reads as the final `else` arm
    // of the mux and its writes are suppressed.
    let mut x1: Bits<32> = Bits::zero();
    let mut x2: Bits<32> = Bits::zero();
    let mut x3: Bits<32> = Bits::zero();
    let mut x4: Bits<32> = Bits::zero();
    let mut x5: Bits<32> = Bits::zero();
    let mut x6: Bits<32> = Bits::zero();
    let mut x7: Bits<32> = Bits::zero();
    let mut x8: Bits<32> = Bits::zero();
    let mut x9: Bits<32> = Bits::zero();
    let mut x10: Bits<32> = Bits::zero();
    let mut x11: Bits<32> = Bits::zero();
    let mut x12: Bits<32> = Bits::zero();
    let mut x13: Bits<32> = Bits::zero();
    let mut x14: Bits<32> = Bits::zero();
    let mut x15: Bits<32> = Bits::zero();
    let mut x16: Bits<32> = Bits::zero();
    let mut x17: Bits<32> = Bits::zero();
    let mut x18: Bits<32> = Bits::zero();
    let mut x19: Bits<32> = Bits::zero();
    let mut x20: Bits<32> = Bits::zero();
    let mut x21: Bits<32> = Bits::zero();
    let mut x22: Bits<32> = Bits::zero();
    let mut x23: Bits<32> = Bits::zero();
    let mut x24: Bits<32> = Bits::zero();
    let mut x25: Bits<32> = Bits::zero();
    let mut x26: Bits<32> = Bits::zero();
    let mut x27: Bits<32> = Bits::zero();
    let mut x28: Bits<32> = Bits::zero();
    let mut x29: Bits<32> = Bits::zero();
    let mut x30: Bits<32> = Bits::zero();
    let mut x31: Bits<32> = Bits::zero();

    memory.read_port::<0>().read(0);

    loop {
        clk.tick().await;

        let rst: bool = rstn.read() == Logic::Zero;
        let b_en: bool = boot_en.read() == Logic::One;
        let b_addr: Bits<32> = boot_addr.read();
        let b_data: Bits<32> = boot_data.read();

        // Staged at the bottom of the previous iteration, with the addresses that
        // iteration committed. A port that was not issued reads back not-ready,
        // which stands in for the `else { zero }` arms written by hand.
        let fetched: Bits<32> = if memory.read_port::<0>().is_ready() {
            memory.read_port::<0>().data()
        } else {
            Bits::zero()
        };
        let mem_load: Bits<32> = if memory.read_port::<1>().is_ready() {
            memory.read_port::<1>().data()
        } else {
            Bits::zero()
        };

        let running: bool = !rst && !halted_r;

        // ── WB ──────────────────────────────────────────────────────────────
        let wb_we: bool = running && mem_wb_valid && mem_wb_writes_reg && mem_wb_rd != 0;
        let wb_rd: usize = mem_wb_rd;
        let wb_val: Bits<32> = mem_wb_result;
        let ecall_now: bool = running && mem_wb_valid && mem_wb_is_ecall;

        // ── ID: decode the instruction in IF/ID ─────────────────────────────
        let w: u32 = if_id_instr.as_u32();
        let op: u32 = w & 0x7F;
        let rd: usize = ((w >> 7) & 0x1F) as usize;
        let rs1: usize = ((w >> 15) & 0x1F) as usize;
        let rs2: usize = ((w >> 20) & 0x1F) as usize;
        let f3: usize = ((w >> 12) & 0x7) as usize;
        let f7: usize = ((w >> 25) & 0x7F) as usize;

        // Register read. The write-through bypass reproduces the `Vec` version's
        // statement order, where writeback ran before the ID stage read the array.
        let rs1_raw: Bits<32> =
            if rs1 == 1  { x1 
            } else if rs1 == 2  { x2 
            } else if rs1 == 3  { x3 
            } else if rs1 == 4  { x4 
            } else if rs1 == 5  { x5 
            } else if rs1 == 6  { x6 
            } else if rs1 == 7  { x7 
            } else if rs1 == 8  { x8 
            } else if rs1 == 9  { x9 
            } else if rs1 == 10 { x10 
            } else if rs1 == 11 { x11 
            } else if rs1 == 12 { x12 
            } else if rs1 == 13 { x13 
            } else if rs1 == 14 { x14 
            } else if rs1 == 15 { x15 
            } else if rs1 == 16 { x16 
            } else if rs1 == 17 { x17 
            } else if rs1 == 18 { x18 
            } else if rs1 == 19 { x19 
            } else if rs1 == 20 { x20 
            } else if rs1 == 21 { x21 
            } else if rs1 == 22 { x22 
            } else if rs1 == 23 { x23 
            } else if rs1 == 24 { x24 
            } else if rs1 == 25 { x25 
            } else if rs1 == 26 { x26 
            } else if rs1 == 27 { x27 
            } else if rs1 == 28 { x28 
            } else if rs1 == 29 { x29 
            } else if rs1 == 30 { x30 
            } else if rs1 == 31 { x31 
            } else { Bits::zero() };
        let rs2_raw: Bits<32> =
            if rs2 == 1  { x1 
            } else if rs2 == 2  { x2 
            } else if rs2 == 3  { x3 
            } else if rs2 == 4  { x4 
            } else if rs2 == 5  { x5 
            } else if rs2 == 6  { x6 
            } else if rs2 == 7  { x7 
            } else if rs2 == 8  { x8 
            } else if rs2 == 9  { x9 
            } else if rs2 == 10 { x10 
            } else if rs2 == 11 { x11 
            } else if rs2 == 12 { x12 
            } else if rs2 == 13 { x13 
            } else if rs2 == 14 { x14 
            } else if rs2 == 15 { x15 
            } else if rs2 == 16 { x16 
            } else if rs2 == 17 { x17 
            } else if rs2 == 18 { x18 
            } else if rs2 == 19 { x19 
            } else if rs2 == 20 { x20 
            } else if rs2 == 21 { x21 
            } else if rs2 == 22 { x22 
            } else if rs2 == 23 { x23 
            } else if rs2 == 24 { x24 
            } else if rs2 == 25 { x25 
            } else if rs2 == 26 { x26 
            } else if rs2 == 27 { x27 
            } else if rs2 == 28 { x28 
            } else if rs2 == 29 { x29 
            } else if rs2 == 30 { x30 
            } else if rs2 == 31 { x31 
            } else { Bits::zero() };
        let rs1_val: Bits<32> = if rs1 == 0 {
            Bits::zero()
        } else if wb_we && wb_rd == rs1 {
            wb_val
        } else {
            rs1_raw
        };
        let rs2_val: Bits<32> = if rs2 == 0 {
            Bits::zero()
        } else if wb_we && wb_rd == rs2 {
            wb_val
        } else {
            rs2_raw
        };

        // ── MEM ─────────────────────────────────────────────────────────────
        let n_mem_wb_valid: bool = ex_mem_valid;
        let n_mem_wb_result: Bits<32> = if ex_mem_is_load { mem_load } else { ex_mem_alu };
        let n_mem_wb_rd: usize = if ex_mem_valid { ex_mem_rd } else { 0 };
        let n_mem_wb_writes_reg: bool = ex_mem_valid && ex_mem_writes_reg;
        let n_mem_wb_is_ecall: bool = ex_mem_valid && ex_mem_is_ecall;

        // ── Forwarding: EX/MEM (newest) beats MEM/WB ────────────────────────
        let fwd_rs1: Bits<32> = if ex_mem_valid && ex_mem_writes_reg && !ex_mem_is_load
            && ex_mem_rd != 0 && ex_mem_rd == id_ex_rs1 {
            ex_mem_alu
        } else if mem_wb_valid && mem_wb_writes_reg && mem_wb_rd != 0 && mem_wb_rd == id_ex_rs1 {
            mem_wb_result
        } else {
            id_ex_rs1_val
        };
        let fwd_rs2: Bits<32> = if ex_mem_valid && ex_mem_writes_reg && !ex_mem_is_load
            && ex_mem_rd != 0 && ex_mem_rd == id_ex_rs2 {
            ex_mem_alu
        } else if mem_wb_valid && mem_wb_writes_reg && mem_wb_rd != 0 && mem_wb_rd == id_ex_rs2 {
            mem_wb_result
        } else {
            id_ex_rs2_val
        };

        // ── EX ──────────────────────────────────────────────────────────────
        // Three TOTAL if-chain expressions instead of one tuple-returning chain.
        let alu_b: Bits<32> = if id_ex_is_alu_imm { id_ex_imm_i } else { fwd_rs2 };
        let ex_alu: Bits<32> = if !id_ex_valid {
            Bits::zero()
        } else if id_ex_is_lui {
            id_ex_imm_u
        } else if id_ex_is_auipc {
            id_ex_pc + id_ex_imm_u
        } else if id_ex_is_jal || id_ex_is_jalr {
            id_ex_pc + Bits::<32>::from_u32(4)
        } else if id_ex_is_branch {
            Bits::zero()
        } else if id_ex_is_load {
            fwd_rs1 + id_ex_imm_i
        } else if id_ex_is_store {
            fwd_rs1 + id_ex_imm_s
        } else if id_ex_is_ecall {
            Bits::zero()
        } else {
            alu(fwd_rs1, alu_b, id_ex_f3, id_ex_f7, !id_ex_is_alu_imm)
        };
        let flush: bool = id_ex_valid
            && (id_ex_is_jal
                || id_ex_is_jalr
                || (id_ex_is_branch && branch_taken(fwd_rs1, fwd_rs2, id_ex_f3)));
        let branch_target: Bits<32> = if id_ex_is_jal {
            id_ex_pc + id_ex_imm_j
        } else if id_ex_is_jalr {
            // `& !1` in the readable version. `!` on a `Bits` lowers to a LOGICAL
            // negation, so the mask is written out.
            (fwd_rs1 + id_ex_imm_i) & Bits::<32>::from_u32(0xFFFF_FFFE)
        } else {
            id_ex_pc + id_ex_imm_b
        };
        let n_ex_mem_rd: usize = if id_ex_is_branch || id_ex_is_store || id_ex_is_ecall {
            0
        } else {
            id_ex_rd
        };
        let n_ex_mem_rs2v: Bits<32> = if id_ex_is_store { fwd_rs2 } else { Bits::zero() };

        // ── Load-use hazard, detected at the EX/ID boundary ─────────────────
        let load_use_stall: bool = id_ex_valid && id_ex_is_load && if_id_valid
            && id_ex_rd != 0
            && (id_ex_rd == rs1 || id_ex_rd == rs2);

        // ── ID → new ID/EX ──────────────────────────────────────────────────
        let id_bubble: bool = !if_id_valid || flush || load_use_stall;
        let dec_writes_reg: bool = (op == OP_LUI
            || op == OP_AUIPC
            || op == OP_JAL
            || op == OP_JALR
            || op == OP_LOAD
            || op == OP_ALU_IMM
            || op == OP_ALU_REG)
            && rd != 0;

        // ── IF and PC ───────────────────────────────────────────────────────
        let n_if_id_valid: bool = if flush {
            false
        } else if load_use_stall {
            if_id_valid
        } else {
            true
        };
        let n_if_id_pc: Bits<32> = if flush {
            Bits::zero()
        } else if load_use_stall {
            if_id_pc
        } else {
            pc
        };
        let n_if_id_instr: Bits<32> = if flush {
            Bits::zero()
        } else if load_use_stall {
            if_id_instr
        } else {
            fetched
        };
        let n_pc: Bits<32> = if flush {
            branch_target
        } else if load_use_stall {
            pc
        } else {
            pc + Bits::<32>::from_u32(4)
        };

        // ── Outputs ─────────────────────────────────────────────────────────
        let n_halted: bool = if rst { false } else { halted_r || ecall_now };
        let n_a0: Bits<32> = if rst {
            Bits::zero()
        } else if ecall_now {
            x10
        } else {
            a0_r
        };
        // ── Commit ──────────────────────────────────────────────────────────
        // IN REVERSE PIPELINE ORDER. These are plain sequential Rust statements,
        // so committing MEM/WB after EX/MEM would read an EX/MEM that had already
        // been overwritten this iteration. The readable version sidesteps it by
        // building whole `new_*` structs first; without structs, the order IS the
        // mechanism, and getting it wrong costs one stage of every value.
        halted_r = n_halted;
        a0_r = n_a0;

        mem_wb_valid      = if rst { false } else if running { n_mem_wb_valid } else { mem_wb_valid };
        mem_wb_result     = if rst { Bits::zero() } else if running { n_mem_wb_result } else { mem_wb_result };
        mem_wb_rd         = if rst { 0 } else if running { n_mem_wb_rd } else { mem_wb_rd };
        mem_wb_writes_reg = if rst { false } else if running { n_mem_wb_writes_reg } else { mem_wb_writes_reg };
        mem_wb_is_ecall   = if rst { false } else if running { n_mem_wb_is_ecall } else { mem_wb_is_ecall };

        ex_mem_valid      = if rst { false } else if running { id_ex_valid } else { ex_mem_valid };
        ex_mem_alu        = if rst { Bits::zero() } else if running { ex_alu } else { ex_mem_alu };
        ex_mem_rs2v       = if rst { Bits::zero() } else if running { n_ex_mem_rs2v } else { ex_mem_rs2v };
        ex_mem_rd         = if rst { 0 } else if running { n_ex_mem_rd } else { ex_mem_rd };
        ex_mem_writes_reg = if rst { false } else if running { id_ex_valid && id_ex_writes_reg } else { ex_mem_writes_reg };
        ex_mem_is_load    = if rst { false } else if running { id_ex_valid && id_ex_is_load } else { ex_mem_is_load };
        ex_mem_is_store   = if rst { false } else if running { id_ex_valid && id_ex_is_store } else { ex_mem_is_store };
        ex_mem_is_ecall   = if rst { false } else if running { id_ex_valid && id_ex_is_ecall } else { ex_mem_is_ecall };

        id_ex_valid      = if rst { false } else if running { !id_bubble } else { id_ex_valid };
        id_ex_pc         = if rst { Bits::zero() } else if running && !id_bubble { if_id_pc } else if running { Bits::zero() } else { id_ex_pc };
        id_ex_rs1        = if rst { 0 } else if running && !id_bubble { rs1 } else if running { 0 } else { id_ex_rs1 };
        id_ex_rs2        = if rst { 0 } else if running && !id_bubble { rs2 } else if running { 0 } else { id_ex_rs2 };
        id_ex_rd         = if rst { 0 } else if running && !id_bubble { rd } else if running { 0 } else { id_ex_rd };
        id_ex_rs1_val    = if rst { Bits::zero() } else if running && !id_bubble { rs1_val } else if running { Bits::zero() } else { id_ex_rs1_val };
        id_ex_rs2_val    = if rst { Bits::zero() } else if running && !id_bubble { rs2_val } else if running { Bits::zero() } else { id_ex_rs2_val };
        id_ex_imm_i      = if rst { Bits::zero() } else if running && !id_bubble { imm_i(if_id_instr) } else if running { Bits::zero() } else { id_ex_imm_i };
        id_ex_imm_s      = if rst { Bits::zero() } else if running && !id_bubble { imm_s(if_id_instr) } else if running { Bits::zero() } else { id_ex_imm_s };
        id_ex_imm_b      = if rst { Bits::zero() } else if running && !id_bubble { imm_b(if_id_instr) } else if running { Bits::zero() } else { id_ex_imm_b };
        id_ex_imm_j      = if rst { Bits::zero() } else if running && !id_bubble { imm_j(if_id_instr) } else if running { Bits::zero() } else { id_ex_imm_j };
        id_ex_imm_u      = if rst { Bits::zero() } else if running && !id_bubble { if_id_instr & Bits::<32>::from_u32(0xFFFF_F000) } else if running { Bits::zero() } else { id_ex_imm_u };
        id_ex_f3         = if rst { 0 } else if running && !id_bubble { f3 } else if running { 0 } else { id_ex_f3 };
        id_ex_f7         = if rst { 0 } else if running && !id_bubble { f7 } else if running { 0 } else { id_ex_f7 };
        id_ex_is_load    = if rst { false } else if running && !id_bubble { op == OP_LOAD } else if running { false } else { id_ex_is_load };
        id_ex_is_store   = if rst { false } else if running && !id_bubble { op == OP_STORE } else if running { false } else { id_ex_is_store };
        id_ex_is_branch  = if rst { false } else if running && !id_bubble { op == OP_BRANCH } else if running { false } else { id_ex_is_branch };
        id_ex_is_jal     = if rst { false } else if running && !id_bubble { op == OP_JAL } else if running { false } else { id_ex_is_jal };
        id_ex_is_jalr    = if rst { false } else if running && !id_bubble { op == OP_JALR } else if running { false } else { id_ex_is_jalr };
        id_ex_is_lui     = if rst { false } else if running && !id_bubble { op == OP_LUI } else if running { false } else { id_ex_is_lui };
        id_ex_is_auipc   = if rst { false } else if running && !id_bubble { op == OP_AUIPC } else if running { false } else { id_ex_is_auipc };
        id_ex_is_alu_imm = if rst { false } else if running && !id_bubble { op == OP_ALU_IMM } else if running { false } else { id_ex_is_alu_imm };
        id_ex_is_ecall   = if rst { false } else if running && !id_bubble { op == OP_ECALL } else if running { false } else { id_ex_is_ecall };
        id_ex_writes_reg = if rst { false } else if running && !id_bubble { dec_writes_reg } else if running { false } else { id_ex_writes_reg };

        if_id_valid = if rst { false } else if running { n_if_id_valid } else { if_id_valid };
        if_id_pc    = if rst { Bits::zero() } else if running { n_if_id_pc } else { if_id_pc };
        if_id_instr = if rst { Bits::zero() } else if running { n_if_id_instr } else { if_id_instr };

        pc = if rst { Bits::zero() } else if running { n_pc } else { pc };

        // Register file: an enabled register per architectural register.
        if rst {
            x1 = Bits::zero();
            x2 = Bits::zero();
            x3 = Bits::zero();
            x4 = Bits::zero();
            x5 = Bits::zero();
            x6 = Bits::zero();
            x7 = Bits::zero();
            x8 = Bits::zero();
            x9 = Bits::zero();
            x10 = Bits::zero();
            x11 = Bits::zero();
            x12 = Bits::zero();
            x13 = Bits::zero();
            x14 = Bits::zero();
            x15 = Bits::zero();
            x16 = Bits::zero();
            x17 = Bits::zero();
            x18 = Bits::zero();
            x19 = Bits::zero();
            x20 = Bits::zero();
            x21 = Bits::zero();
            x22 = Bits::zero();
            x23 = Bits::zero();
            x24 = Bits::zero();
            x25 = Bits::zero();
            x26 = Bits::zero();
            x27 = Bits::zero();
            x28 = Bits::zero();
            x29 = Bits::zero();
            x30 = Bits::zero();
            x31 = Bits::zero();
        } else if wb_we {
                if wb_rd == 1  { x1 = wb_val; 
                } else if wb_rd == 2  { x2 = wb_val; 
                } else if wb_rd == 3  { x3 = wb_val; 
                } else if wb_rd == 4  { x4 = wb_val; 
                } else if wb_rd == 5  { x5 = wb_val; 
                } else if wb_rd == 6  { x6 = wb_val; 
                } else if wb_rd == 7  { x7 = wb_val; 
                } else if wb_rd == 8  { x8 = wb_val; 
                } else if wb_rd == 9  { x9 = wb_val; 
                } else if wb_rd == 10 { x10 = wb_val; 
                } else if wb_rd == 11 { x11 = wb_val; 
                } else if wb_rd == 12 { x12 = wb_val; 
                } else if wb_rd == 13 { x13 = wb_val; 
                } else if wb_rd == 14 { x14 = wb_val; 
                } else if wb_rd == 15 { x15 = wb_val; 
                } else if wb_rd == 16 { x16 = wb_val; 
                } else if wb_rd == 17 { x17 = wb_val; 
                } else if wb_rd == 18 { x18 = wb_val; 
                } else if wb_rd == 19 { x19 = wb_val; 
                } else if wb_rd == 20 { x20 = wb_val; 
                } else if wb_rd == 21 { x21 = wb_val; 
                } else if wb_rd == 22 { x22 = wb_val; 
                } else if wb_rd == 23 { x23 = wb_val; 
                } else if wb_rd == 24 { x24 = wb_val; 
                } else if wb_rd == 25 { x25 = wb_val; 
                } else if wb_rd == 26 { x26 = wb_val; 
                } else if wb_rd == 27 { x27 = wb_val; 
                } else if wb_rd == 28 { x28 = wb_val; 
                } else if wb_rd == 29 { x29 = wb_val; 
                } else if wb_rd == 30 { x30 = wb_val; 
                } else if wb_rd == 31 { x31 = wb_val; 
            }
        }

        // ── Outputs, in the TRAILING segment ────────────────────────────────
        // AFTER the commit, so each one reads the register it reports. This is not
        // a style choice and the sweep found both halves of it:
        //
        //   written BEFORE the commit, a plain `Out` carries the value the register
        //   held when the statement ran, while the emitted `assign` reads the
        //   register itself — the hardware leads by a cycle (`Cycle 197
        //   program_counter expected 4 got 8`). D1's guard does not catch it here,
        //   because an `In` read precedes the write and that exempts the segment.
        //
        //   reaching for `RegOut` instead does NOT fix it, which is the part worth
        //   knowing: in a single-tick loop the trailing statements share the head's
        //   phase, so the transpiler folds a trailing `RegOut` write into THIS
        //   edge while the simulator still commits it on the NEXT one. Same
        //   one-cycle lead, opposite cause.
        //
        // A plain `Out` written after the commit is the shape where the two agree:
        // continuous assignment from a register, and the simulator writes that same
        // register's post-commit value in the same cycle.
        program_counter.write(pc);
        halted.write(if halted_r { Logic::One } else { Logic::Zero });
        a0_out.write(a0_r);

        // ── Stage the next cycle's memory accesses ──────────────────────────
        let fetch_idx: usize = (pc >> 2).as_usize();
        if fetch_idx < MEM_WORDS {
            memory.read_port::<0>().read(fetch_idx);
        }
        if rst {
            let b_idx: usize = (b_addr >> 2).as_usize();
            if b_en && b_idx < MEM_WORDS {
                memory.write_port::<0>().write(b_idx, b_data);
            }
        } else if ex_mem_valid {
            let d_idx: usize = (ex_mem_alu >> 2).as_usize();
            if d_idx < MEM_WORDS {
                if ex_mem_is_load {
                    memory.read_port::<1>().read(d_idx);
                } else if ex_mem_is_store {
                    memory.write_port::<0>().write(d_idx, ex_mem_rs2v);
                }
            }
        }
    }
}

// ── Execution engine ──────────────────────────────────────────────────────────

/// Hold `rstn` low, write the program in a word at a time through the boot port,
/// then release reset and run until `halted`.
///
/// Returns `(a0, execution_cycles)` — boot cycles are not counted, so the number
/// is comparable with the readable version's.
pub fn run_program(program: Vec<u32>, max_cycles: usize) -> (u32, usize) {
    assert!(
        program.len() <= MEM_WORDS,
        "program is {} words but the memory is {MEM_WORDS}",
        program.len()
    );

    let mut clk  = Clock::<MainClk>::new();
    let mut exec = HardwareExecutor::new();

    let (rstn_drv,  rstn_in)  = wire::<Logic, MainClk>(Logic::Zero);
    let (ben_drv,   ben_in)   = wire::<Logic, MainClk>(Logic::Zero);
    let (baddr_drv, baddr_in) = wire::<Bits<32>, MainClk>(Bits::zero());
    let (bdata_drv, bdata_in) = wire::<Bits<32>, MainClk>(Bits::zero());
    let (pc_out,   _pc_in)    = wire::<Bits<32>, MainClk>(Bits::zero());
    let (halt_out,  halt_in)  = wire::<Logic, MainClk>(Logic::Zero);
    let (a0_out,    a0_in)    = wire::<Bits<32>, MainClk>(Bits::zero());

    let reads = vec![
        rstn_in.wire_id(),
        ben_in.wire_id(),
        baddr_in.wire_id(),
        bdata_in.wire_id(),
    ];
    exec.spawn_untracked(
        rv32i_cpu_transpilable(
            clk.clone(), rstn_in, ben_in, baddr_in, bdata_in, pc_out, halt_out, a0_out,
        ),
        reads,
    );

    // Boot. The core samples the boot port after the edge and stages the write
    // before the next one, so each word takes one cycle and one extra cycle is
    // needed to flush the last one.
    for (i, word) in program.iter().enumerate() {
        ben_drv.write(Logic::One);
        baddr_drv.write(Bits::<32>::from_usize(i * 4));
        bdata_drv.write(Bits::<32>::from_u32(*word));
        exec.tick_clock(&mut clk);
    }
    ben_drv.write(Logic::Zero);
    exec.tick_clock(&mut clk);

    // Release reset and run.
    rstn_drv.write(Logic::One);
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

pub fn test_addi() -> Vec<u32> {
    vec![
        addi(1, 0, 10),
        addi(2, 0, 5),
        add(10, 1, 2),
        0x0000_0073,
    ]
}

pub fn test_sub() -> Vec<u32> {
    vec![
        addi(1, 0, 15),
        addi(2, 0, 10),
        sub(10, 1, 2),
        0x0000_0073,
    ]
}

pub fn test_multiple_adds() -> Vec<u32> {
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

pub fn test_branch_taken() -> Vec<u32> {
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

pub fn test_branch_not_taken() -> Vec<u32> {
    vec![
        addi(1, 0, 5),
        addi(2, 0, 10),
        beq(1, 2, 8),
        addi(10, 0, 99),
        0x0000_0073,
    ]
}

pub fn test_load_store() -> Vec<u32> {
    vec![
        addi(1, 0, 88),
        sw(0, 1, 0),
        lw(10, 0, 0),
        0x0000_0073,
    ]
}

pub fn test_negative_numbers() -> Vec<u32> {
    vec![
        addi(1, 0, 10),
        addi(2, 0, -3),
        add(10, 1, 2),
        0x0000_0073,
    ]
}

pub fn test_zero_operations() -> Vec<u32> {
    vec![
        addi(1, 0, 0),
        addi(2, 0, 42),
        add(10, 1, 2),
        0x0000_0073,
    ]
}

pub fn test_fibonacci() -> Vec<u32> {
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

pub fn test_jal() -> Vec<u32> {
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

pub fn test_data_hazard_forwarding() -> Vec<u32> {
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

pub fn test_load_use_stall() -> Vec<u32> {
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

pub fn test_bubblesort() -> Vec<u32> {
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

// Excluded under cfg(test) so an integration test can `include!` this file
// without its harness `main` clashing (see rv32i_cpu.rs for the rationale).
/// The same self-check `main` runs, as a `#[test]`.
///
/// `cargo test` BUILDS an example but never RUNS it — cargo cannot know whether an
/// arbitrary `main` is safe or terminating. Setting `test = true` on the
/// `[[example]]` compiles this file as a test target instead, so this runs under
/// `cargo test` while `main` still works for `cargo run --example`. The body is
/// shared, so the two cannot drift.
#[cfg(test)]
#[test]
fn selfcheck() {
    assert_eq!(run_all(), 0, "some programs failed");
}

#[cfg(not(test))]
fn main() {
    std::process::exit(if run_all() == 0 { 0 } else { 1 });
}

/// Runs every test program; returns the number that failed.
fn run_all() -> usize {
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║   RV32I CPU - 5-Stage Pipelined - TRANSPILABLE spelling        ║");
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
        ("Bubblesort: sort+sum 10 elements",  test_bubblesort(),            363),
    ];

    let mut passed = 0;
    let mut failed = 0;

    for (name, prog, expected) in tests {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_program(prog, 1000))) {
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

    failed
}
