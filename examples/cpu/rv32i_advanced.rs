/// RV32I Advanced Examples: Loops, Branches, and Memory
///
/// This example demonstrates:
/// 1. Loop using BNE (loop sum from 1 to 5)
/// 2. Conditional branch
/// 3. Memory operations (load/store)
use std::sync::{Arc, Mutex};

struct MainClk;

// ────────────────────────────────────────────────────────────────────────────
// Constants from main rv32i.rs (duplicated for standalone compilation)
// ────────────────────────────────────────────────────────────────────────────

const OP_LOAD: u32 = 0x03;
const OP_IMMEDIATEX: u32 = 0x13;
const OP_STORE: u32 = 0x23;
const OP_REGISTER: u32 = 0x33;
const OP_BRANCH: u32 = 0x63;
const OP_JAL: u32 = 0x6F;

const F3_ADDI: u32 = 0;
const F3_ANDI: u32 = 7;
const F3_BEQ: u32 = 0;
const F3_BNE: u32 = 1;
const F3_LW: u32 = 2;
const F3_SW: u32 = 2;
const F3_ADDSUB: u32 = 0;
const F7_ADD: u32 = 0;

#[derive(Clone)]
struct CpuState {
    pc: u32,
    regs: Vec<u32>,
    imem: Arc<Mutex<Vec<u32>>>,
    dmem: Arc<Mutex<Vec<u32>>>,
}

impl CpuState {
    fn new(imem: Arc<Mutex<Vec<u32>>>, dmem: Arc<Mutex<Vec<u32>>>) -> Self {
        CpuState {
            pc: 0,
            regs: vec![0; 32],
            imem,
            dmem,
        }
    }

    fn get_reg(&self, idx: u32) -> u32 {
        if idx == 0 {
            0
        } else {
            self.regs[idx as usize]
        }
    }

    fn set_reg(&mut self, idx: u32, val: u32) {
        if idx != 0 {
            self.regs[idx as usize] = val;
        }
    }

    fn step(&mut self) {
        // Simplified single-cycle execution (combining all stages)
        let imem_lock = self.imem.lock().unwrap();
        let addr = (self.pc >> 2) as usize;
        let instr = if addr < imem_lock.len() {
            imem_lock[addr]
        } else {
            0
        };
        drop(imem_lock);

        let opcode = instr & 0x7F;
        let rd = (instr >> 7) & 0x1F;
        let rs1_idx = (instr >> 15) & 0x1F;
        let rs2_idx = (instr >> 20) & 0x1F;
        let rs1_val = self.get_reg(rs1_idx);
        let rs2_val = self.get_reg(rs2_idx);

        let mut next_pc = self.pc.wrapping_add(4);
        let mut wb_rd = 0u32;
        let mut wb_value = 0u32;
        let mut wb_valid = false;

        match opcode {
            OP_IMMEDIATEX => {
                let funct3 = (instr >> 12) & 0x7;
                let imm = (instr >> 20) as i32;
                let imm = if imm & 0x800 != 0 {
                    ((imm as u32) | 0xFFFFF000u32) as i32
                } else {
                    imm
                };

                match funct3 {
                    F3_ADDI => {
                        if rd != 0 {
                            wb_rd = rd;
                            wb_value = rs1_val.wrapping_add(imm as u32);
                            wb_valid = true;
                        }
                    }
                    _ => {}
                }
            }
            OP_REGISTER => {
                let funct3 = (instr >> 12) & 0x7;
                let funct7 = (instr >> 25) & 0x7F;

                match funct3 {
                    F3_ADDSUB => {
                        if funct7 == F7_ADD && rd != 0 {
                            wb_rd = rd;
                            wb_value = rs1_val.wrapping_add(rs2_val);
                            wb_valid = true;
                        }
                    }
                    _ => {}
                }
            }
            OP_LOAD => {
                let funct3 = (instr >> 12) & 0x7;
                let imm = (instr >> 20) as i32;
                let imm = if imm & 0x800 != 0 {
                    ((imm as u32) | 0xFFFFF000u32) as i32
                } else {
                    imm
                };

                if funct3 == F3_LW {
                    let addr = rs1_val.wrapping_add(imm as u32);
                    let dmem_lock = self.dmem.lock().unwrap();
                    let mem_addr = (addr >> 2) as usize;
                    let value = if mem_addr < dmem_lock.len() {
                        dmem_lock[mem_addr]
                    } else {
                        0
                    };
                    drop(dmem_lock);

                    if rd != 0 {
                        wb_rd = rd;
                        wb_value = value;
                        wb_valid = true;
                    }
                }
            }
            OP_STORE => {
                let funct3 = (instr >> 12) & 0x7;
                let imm11_5 = (instr >> 25) & 0x7F;
                let imm4_0 = (instr >> 7) & 0x1F;
                let imm = (imm11_5 << 5) | imm4_0;
                let imm = if imm & 0x800 != 0 {
                    ((imm as u32) | 0xFFFFF000u32) as i32
                } else {
                    imm as i32
                };

                if funct3 == F3_SW {
                    let addr = rs1_val.wrapping_add(imm as u32);
                    let mut dmem_lock = self.dmem.lock().unwrap();
                    let mem_addr = (addr >> 2) as usize;
                    if mem_addr < dmem_lock.len() {
                        dmem_lock[mem_addr] = rs2_val;
                    }
                    drop(dmem_lock);
                }
            }
            OP_BRANCH => {
                let funct3 = (instr >> 12) & 0x7;
                let imm12 = (instr >> 31) & 1;
                let imm11 = (instr >> 7) & 1;
                let imm10_5 = (instr >> 25) & 0x3F;
                let imm4_1 = (instr >> 8) & 0xF;
                let imm = (imm12 << 12) | (imm11 << 11) | (imm10_5 << 5) | (imm4_1 << 1);
                let imm = if imm12 != 0 {
                    ((imm as u32) | 0xFFFFF000u32) as i32
                } else {
                    imm as i32
                };

                let branch_taken = match funct3 {
                    F3_BEQ => rs1_val == rs2_val,
                    F3_BNE => rs1_val != rs2_val,
                    _ => false,
                };

                if branch_taken {
                    next_pc = self.pc.wrapping_add(imm as u32);
                }
            }
            _ => {}
        }

        if wb_valid {
            self.set_reg(wb_rd, wb_value);
        }

        self.pc = next_pc;
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Example 1: Loop and Sum (Sum from 1 to 5)
// ────────────────────────────────────────────────────────────────────────────

fn example_loop_sum() {
    println!("\n=== Example 1: Loop Sum (1+2+3+4+5 = 15) ===\n");

    // Program:
    // x1 = 0    (sum accumulator)
    // x2 = 1    (counter)
    // x3 = 6    (loop exit condition)
    // loop:
    //   x1 = x1 + x2
    //   x2 = x2 + 1
    //   if x2 != x3, branch to loop
    // After loop: x1 should be 15

    let program = vec![
        0x00000093, // ADDI x1, x0, 0         → x1 = 0 (sum)
        0x00100113, // ADDI x2, x0, 1         → x2 = 1 (counter)
        0x00600193, // ADDI x3, x0, 6         → x3 = 6 (limit)
        0x00208233, // ADD x1, x1, x2         → x1 += x2
        0x00110113, // ADDI x2, x2, 1         → x2 += 1
        0xFE311AE3, // BNE x2, x3, -12 (loop) → if x2 != x3, jump back
        0x00000000, // NOP
    ];

    let imem = Arc::new(Mutex::new(program));
    let dmem = Arc::new(Mutex::new(vec![0; 256]));
    let mut cpu = CpuState::new(imem, dmem);

    // Run enough cycles for the loop to complete
    for _ in 0..30 {
        cpu.step();
    }

    println!("x1 (sum) = {}", cpu.get_reg(1));
    assert_eq!(cpu.get_reg(1), 15, "Sum should be 15");
    println!("✓ Loop sum test passed!");
}

// ────────────────────────────────────────────────────────────────────────────
// Example 2: Memory Store and Load
// ────────────────────────────────────────────────────────────────────────────

fn example_memory() {
    println!("\n=== Example 2: Memory Store and Load ===\n");

    // Program:
    // x1 = 42
    // Store x1 to mem[0]
    // x2 = Load from mem[0]
    // x1 and x2 should both be 42

    let program = vec![
        0x02A00093, // ADDI x1, x0, 42        → x1 = 42
        0x00100113, // ADDI x2, x0, 1         → x2 = 1 (unused, for padding)
        0x00102023, // SW x1, 0(x0)           → mem[0] = x1
        0x00002083, // LW x4, 0(x0)           → x4 = mem[0]
        0x00000000, // NOP
    ];

    let imem = Arc::new(Mutex::new(program));
    let dmem = Arc::new(Mutex::new(vec![0; 256]));
    let mut cpu = CpuState::new(imem, dmem);

    for _ in 0..10 {
        cpu.step();
    }

    println!("x1 (original) = {}", cpu.get_reg(1));
    println!("x4 (from memory) = {}", cpu.get_reg(4));

    assert_eq!(cpu.get_reg(1), 42, "x1 should be 42");
    assert_eq!(cpu.get_reg(4), 42, "x4 should be 42 (loaded from mem)");
    println!("✓ Memory test passed!");
}

// ────────────────────────────────────────────────────────────────────────────
// Example 3: Conditional Branch
// ────────────────────────────────────────────────────────────────────────────

fn example_conditional() {
    println!("\n=== Example 3: Conditional Branch ===\n");

    // Program:
    // x1 = 10
    // x2 = 10
    // if x1 == x2, x3 = 100 (skip if equal)
    // else x3 = 50
    // In this case, x1 == x2, so x3 should be 50 (not executed)
    // and x3 stays 0 (or gets 50 depending on branch direction)

    let program = vec![
        0x00A00093, // ADDI x1, x0, 10        → x1 = 10
        0x00A00113, // ADDI x2, x0, 10        → x2 = 10
        0x00209263, // BNE x1, x2, +4         → if x1 != x2, skip (branch not taken)
        0x06400193, // ADDI x3, x0, 100       → x3 = 100 (this executes because branch not taken)
        0x00000000, // NOP
    ];

    let imem = Arc::new(Mutex::new(program));
    let dmem = Arc::new(Mutex::new(vec![0; 256]));
    let mut cpu = CpuState::new(imem, dmem);

    for _ in 0..10 {
        cpu.step();
    }

    println!("x1 = {}", cpu.get_reg(1));
    println!("x2 = {}", cpu.get_reg(2));
    println!("x3 = {}", cpu.get_reg(3));

    assert_eq!(cpu.get_reg(1), 10, "x1 should be 10");
    assert_eq!(cpu.get_reg(2), 10, "x2 should be 10");
    assert_eq!(
        cpu.get_reg(3), 100,
        "x3 should be 100 (branch not taken, so ADDI x3, x0, 100 executed)"
    );
    println!("✓ Conditional branch test passed!");
}

fn main() {
    println!("════════════════════════════════════════════════════════");
    println!("RV32I Advanced Examples");
    println!("════════════════════════════════════════════════════════");

    example_loop_sum();
    example_memory();
    example_conditional();

    println!("\n════════════════════════════════════════════════════════");
    println!("✓ All advanced examples passed!");
    println!("════════════════════════════════════════════════════════");
}
