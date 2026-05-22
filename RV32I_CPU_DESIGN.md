# RV32I Non-Pipelined CPU in Copper HDL

## Overview

A simple, non-pipelined RV32I CPU implementation demonstrating how Copper HDL can model complex hardware in pure Rust with cycle-accurate semantics.

**Key Features:**
- **Non-pipelined execution:** Fetch → Decode → Execute → Memory → Writeback in a single cycle
- **RV32I subset:** 13 instructions for arithmetic, logic, memory, and control flow
- **32-bit data path:** 32 integer registers (x0-x31, x0 hardwired to 0)
- **Unified instruction/data memory:** Separate I-mem and D-mem for simplicity
- **Cycle-accurate simulation:** Same code validates behavior before Verilog synthesis

---

## ISA Subset (RV32I)

### Supported Instructions

| Instruction | Encoding | Semantics | Notes |
|-------------|----------|-----------|-------|
| **ADDI rd, rs1, imm12** | I-type | `rd ← rs1 + sign_ext(imm12)` | 12-bit signed immediate |
| **ADD rd, rs1, rs2** | R-type | `rd ← rs1 + rs2` | Register-register add |
| **SUB rd, rs1, rs2** | R-type | `rd ← rs1 - rs2` | funct7=0x20 |
| **ANDI rd, rs1, imm12** | I-type | `rd ← rs1 & sign_ext(imm12)` | Bitwise AND |
| **AND rd, rs1, rs2** | R-type | `rd ← rs1 & rs2` | Register-register AND |
| **ORI rd, rs1, imm12** | I-type | `rd ← rs1 \| sign_ext(imm12)` | Bitwise OR |
| **OR rd, rs1, rs2** | R-type | `rd ← rs1 \| rs2` | Register-register OR |
| **XORI rd, rs1, imm12** | I-type | `rd ← rs1 ^ sign_ext(imm12)` | Bitwise XOR |
| **XOR rd, rs1, rs2** | R-type | `rd ← rs1 ^ rs2` | Register-register XOR |
| **SLLI rd, rs1, shamt** | I-type | `rd ← rs1 << (imm12 & 0x1F)` | Shift left logical |
| **SLL rd, rs1, rs2** | R-type | `rd ← rs1 << (rs2 & 0x1F)` | Register shift amount |
| **SRLI rd, rs1, shamt** | I-type | `rd ← rs1 >> (imm12 & 0x1F)` | Shift right logical (unsigned) |
| **SRL rd, rs1, rs2** | R-type | `rd ← rs1 >> (rs2 & 0x1F)` | Register shift amount |
| **LW rd, offset(rs1)** | I-type | `rd ← mem[rs1 + sign_ext(offset)]` | 32-bit load |
| **SW rs2, offset(rs1)** | S-type | `mem[rs1 + sign_ext(offset)] ← rs2` | 32-bit store |
| **BEQ rs1, rs2, offset** | B-type | `if rs1 == rs2: pc ← pc + offset` | Branch if equal |
| **BNE rs1, rs2, offset** | B-type | `if rs1 != rs2: pc ← pc + offset` | Branch if not equal |
| **JAL rd, offset** | J-type | `rd ← pc + 4; pc ← pc + offset` | Jump and link (call) |

### Encoding Summary

**R-type:** `funct7[7] | rs2[5] | rs1[5] | funct3[3] | rd[5] | opcode[7]`
- Opcodes: 0x33 (ADD/SUB/AND/OR/XOR/SLL/SRL)
- funct3: 0=ADD/SUB, 1=SLL, 4=XOR, 6=OR, 7=AND, 5=SRL
- funct7: 0=ADD/SLL/SRL/etc., 0x20=SUB

**I-type:** `imm12[12] | rs1[5] | funct3[3] | rd[5] | opcode[7]`
- Opcodes: 0x13 (ADDI/ANDI/ORI/XORI/SLLI/SRLI), 0x03 (LW)
- funct3: 0=ADDI, 1=SLLI, 4=XORI, 5=SRLI, 6=ORI, 7=ANDI, 2=LW

**S-type:** `imm11_5[7] | rs2[5] | rs1[5] | funct3[3] | imm4_0[5] | opcode[7]`
- Opcode: 0x23 (SW)
- funct3: 2 (LW/SW)

**B-type:** `imm12[1] | imm10_5[6] | rs2[5] | rs1[5] | funct3[3] | imm4_1[4] | imm11[1] | opcode[7]`
- Opcode: 0x63 (BEQ, BNE)
- funct3: 0=BEQ, 1=BNE

**J-type:** `imm20[1] | imm10_1[10] | imm11[1] | imm19_12[8] | rd[5] | opcode[7]`
- Opcode: 0x6F (JAL)

---

## CPU State

```rust
pub struct CpuState {
    pc: u32,                              // Program counter
    regs: Vec<u32>,                       // 32 x 32-bit registers
    imem: Arc<Mutex<Vec<u32>>>,          // Instruction memory
    dmem: Arc<Mutex<Vec<u32>>>,          // Data memory
    
    // Pipeline registers (even though single-cycle, saved for clarity)
    fetched_instr: u32,
    decoded_opcode: u32,
    alu_result: u32,
    mem_read_data: u32,
    wb_rd: u32,
    wb_value: u32,
    wb_valid: bool,
}
```

---

## Execution Model (Non-Pipelined)

Each `step()` call executes one complete instruction cycle:

### Stage 1: FETCH
- Read instruction from I-mem at address `pc >> 2`
- Store in `fetched_instr`

### Stage 2: DECODE
- Extract fields: opcode, rd, rs1, rs2, funct3, funct7
- Read register file: `rs1_val = regs[rs1]`, `rs2_val = regs[rs2]`
- Decode immediates (sign-extend as needed)

### Stage 3: EXECUTE
- ALU operations: ADD, SUB, AND, OR, XOR, SLL, SRL
- Branch conditions: BEQ, BNE
- Load address calculation
- Store address calculation
- JAL return address

### Stage 4: MEMORY
- Load: read from D-mem at computed address
- Store: write to D-mem at computed address

### Stage 5: WRITEBACK
- For rd != 0 and writeback-enabled instructions, update `regs[rd]`
- Update PC:
  - Default: `pc ← pc + 4` (next instruction)
  - Branch taken: `pc ← pc + offset`
  - JAL: `pc ← pc + offset` (link address in rd)

---

## Test Program Example

```rust
let program = vec![
    0x00A00093, // ADDI x1, x0, 10       → x1 = 10
    0x01400113, // ADDI x2, x0, 20       → x2 = 20
    0x00208233, // ADD x4, x1, x2        → x4 = 30
];
```

### Execution Trace

| Cycle | PC | Fetched | Operation | Result |
|-------|----|----|-----------|--------|
| 0 | 0x00 | 0x00000000 | Fetch delay | — |
| 1 | 0x04 | 0x00A00093 | Execute NOP | — |
| 2 | 0x08 | 0x01400113 | ADDI x1, x0, 10 | x1 ← 10 |
| 3 | 0x0C | 0x00208233 | ADDI x2, x0, 20 | x2 ← 20 |
| 4 | 0x10 | 0x00000000 | ADD x4, x1, x2 | x4 ← 30 |

**Note:** Non-pipelined means one instruction completes per cycle, but fetch-decode latency means results appear one cycle after the instruction executes.

---

## Design Choices

### Why Non-Pipelined?
- Simpler state machine (easier to verify)
- Fewer forwarding hazards
- Better for educational demonstration
- Still cycle-accurate for algorithmic correctness

### Why Separate I-mem/D-mem?
- Simplifies Harvard architecture
- Avoids complex cache modeling for now
- Real RV32I is von Neumann (unified), but separation is cleaner for this demo

### Why No Pipelining Hazards?
- Writes happen in writeback stage (end of cycle)
- Reads happen in decode stage (start of next cycle)
- So results are visible to the next instruction naturally

---

## Future Extensions

1. **Pipelined version** (5-stage pipeline with hazard detection)
2. **Additional RV32I** (AUIPC, LUI for 32-bit constants)
3. **RV32M** (MUL, DIV extension)
4. **Interrupt support** (trap handling)
5. **UART I/O** (via memory-mapped ports)
6. **Compliance tests** (RISC-V official test suite)

---

## Running the Example

```bash
cargo run --example rv32i
```

Output:
```
=== RV32I CPU Simulation ===
Running 6 cycles (including fetch latency)...

Cycle 0: PC=00000000, Instr=00000000
Cycle 1: PC=00000004, Instr=00a00093
Cycle 2: PC=00000008, Instr=01400113
Cycle 3: PC=0000000c, Instr=00208233
Cycle 4: PC=00000010, Instr=00000000
Cycle 5: PC=00000014, Instr=00000000

=== Register State ===
x1 = 10
x2 = 20
x3 = 0
x4 = 30

✓ All tests passed!
```

---

## Integration with Copper Simulation

When Copper codegen matures, this same `CpuState` logic will compile to synthesizable Verilog using the transpilation pipeline:
- **Phase B (CHIR):** Semantic lowering of CPU module structure
- **Phase C (SHIR):** Scheduling and phase extraction (already complete)
- **Phase D (VLIR):** Legalization and name mangling
- **Phase E:** Verilog text generation
- **Phase F:** Equivalence checking against this Rust simulation

This demonstrates Copper's key value: **simulation and synthesis from the same source**.
