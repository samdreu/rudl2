# Copper Scheduled Hardware IR (SHIR) Design

## Purpose

This document defines the design of the Scheduled Hardware Intermediate Representation (SHIR) for Copper — Phase C output in the transpilation pipeline defined in `TRANSPILATION_PLAN.md`.

SHIR receives a `CHIRModule` (Phase B output) and outputs an explicitly timed, assignment-intent-fixed representation suitable for Phase D legalization and Phase E emission. SHIR is the "cycle timing" layer: every operation has been placed into a specific timing region, every register has an explicit next-value expression, and assignment intent (`<=` vs `=` vs `assign`) is fully determined.

After SHIR, the only remaining work is name legalization, Verilog syntax mapping, and text emission.

---

## What SHIR Is and Is Not

**SHIR is:**
- Explicitly timed: every statement is tagged to a pre-edge or post-edge timing region, or to a specific phase index for multi-tick modules
- Assignment-intent-fixed: every update is categorized as non-blocking (register update), blocking (combinational temporary), or continuous (output wire drive)
- Phase-aware: multi-tick modules have an explicit `phase_r` register and phase-indexed timing regions
- The correct level to verify equivalence against Copper simulation traces
- The final form before legalization — SHIR is semantically complete

**SHIR is not:**
- Verilog-legal (keyword conflicts, name collisions, etc. — that is Phase D)
- Formatted or pretty (that is Phase E)
- Optimized (inlining, constant folding, dead code removal are out of scope for Milestone 1)

---

## Core Data Model

```rust
pub struct SHIRModule {
    pub name: String,
    pub ports: Vec<SHIRPort>,
    pub body: SHIRBody,
    pub span: SourceSpan,
}

pub struct SHIRPort {
    pub name: String,
    pub direction: SHIRPortDir,
    pub kind: SHIRPortKind,
}

pub enum SHIRPortDir {
    Input,
    Output,
}

pub enum SHIRPortKind {
    Clock,
    Data { ty: CHIRType },  // reuses CHIRType — widths are already resolved
}

pub enum SHIRBody {
    Combinational(SHIRCombBody),
    Sequential(SHIRSeqBody),
}
```

### Combinational Body

Combinational modules have no registers, no clock, and no timing regions. All statements are continuous assignments or blocking-assign temporaries.

```rust
pub struct SHIRCombBody {
    // #[hardware] submodule instances, carried through from CHIR unchanged
    pub submodules: Vec<SHIRSubmoduleInst>,
    // Intermediate combinational values (blocking assign / wire in Verilog)
    pub wires: Vec<SHIRWire>,
    // Drives the output port continuously
    pub output_expr: SHIRExpr,
}

pub struct SHIRWire {
    pub name: String,
    pub ty: CHIRType,
    pub value: SHIRExpr,
}
```

### Sequential Body

Sequential modules have a clock, a set of registers, a set of timing regions, and an output drive model. For single-tick modules there are two timing regions (pre and post). For multi-tick modules there are N×2 regions (pre and post for each phase).

```rust
pub struct SHIRSeqBody {
    pub clock: String,

    // State registers (all live across tick boundaries)
    pub registers: Vec<SHIRReg>,

    // #[hardware] submodule instances wired into this module.
    // Their outputs are combinational wires visible in all phases.
    pub submodules: Vec<SHIRSubmoduleInst>,

    // For single-tick modules: one entry with phase_idx = 0
    // For multi-tick modules: one entry per phase (0..N-1)
    pub phases: Vec<SHIRPhase>,

    // How the output port is driven — see Output Drive Model section
    pub output_drive: SHIROutputDrive,
}

pub struct SHIRReg {
    pub name: String,
    pub ty: CHIRType,
    pub init: Option<SHIRLit>,
}

// A #[hardware] submodule instance carried through from CHIR.
// Phase C does not transform these — they pass through unchanged.
// The output_wire name is a combinational wire available in all timing regions.
pub struct SHIRSubmoduleInst {
    pub inst_name: String,
    pub module_name: String,
    pub inputs: Vec<(String, SHIRExpr)>,
    pub output_wire: String,
    pub output_ty: CHIRType,
}

pub struct SHIRPhase {
    /// 0-indexed phase number within the loop
    pub phase_idx: usize,

    /// Statements that execute before the clock edge in this phase
    /// These become blocking assigns inside always_comb or intermediate wires
    pub pre_edge: Vec<SHIRStmt>,

    /// Register next-value updates that take effect at the clock edge
    /// These become non-blocking assigns inside always_ff
    pub post_edge: Vec<SHIRRegUpdate>,
}

/// A register's next-value assignment — always non-blocking in Verilog
pub struct SHIRRegUpdate {
    pub target: String,     // must be in SHIRSeqBody::registers
    pub next_value: SHIRExpr,
}
```

---

## Timing Region Placement Rules

This is the core of Phase C. The rules describe how each CHIR statement maps to a timing region.

### Single-tick module (one `AwaitTick` in loop_body)

CHIR `loop_body` is split at the single `AwaitTick` into two segments:

```
segment 0: statements before AwaitTick  → pre_edge
segment 1: statements after AwaitTick   → post_edge register updates
```

**Placement rules:**

| CHIR statement / construct | Where it goes |
|---|---|
| `Wire { name, value }` in segment 0 | `SHIRPhase::pre_edge` as `SHIRStmt::Wire` |
| `Wire { name, value }` in segment 1 | `SHIRPhase::pre_edge` as `SHIRStmt::Wire` (still combinational — wires are never stored) |
| `Assign { target: reg, value }` in segment 0 | `SHIRPhase::post_edge` as `SHIRRegUpdate` — see note below |
| `Assign { target: reg, value }` in segment 1 | `SHIRPhase::post_edge` as `SHIRRegUpdate` |
| `Emit { value }` in segment 0 | `SHIROutputDrive::PreEdge(expr)` |
| `Emit { value }` in segment 1 | `SHIROutputDrive::PostEdge(expr)` |
| `If`, `Match` in segment 0 | `SHIRPhase::pre_edge` as `SHIRStmt::If` / `SHIRStmt::Match` |
| `If`, `Match` in segment 1 | `SHIRPhase::post_edge` as conditional `SHIRRegUpdate` |
| `AwaitTick` | Consumed as the segment boundary — not emitted |
| `CHIRSubmoduleInst` | Moved to `SHIRSeqBody::submodules` — output wire is available as `Var` in all segments |

**Note on segment-0 register assigns:** An `Assign` to a register in segment 0 (before the tick) means "this register should have this value in the *next* cycle" — the assignment is still a non-blocking update that takes effect on the clock edge. It goes in `post_edge` regardless of which segment it appears in. This matches Copper simulator semantics: `tick_clock` runs the post-edge poll before the outputs are read.

### Multi-tick module (N `AwaitTick` nodes in loop_body)

CHIR `loop_body` is split at each `AwaitTick` into N+1 segments: `[seg_0, seg_1, ..., seg_N-1]`. Each segment corresponds to one active phase.

An implicit `phase_r` register is generated:

```rust
SHIRReg {
    name: "phase_r",
    ty: CHIRType::UInt { width: ceil_log2(N) },
    init: Some(SHIRLit { ty: UInt { width: ceil_log2(N) }, value: 0 }),
}
```

Each segment maps to a `SHIRPhase` with `phase_idx = K` for segment K. The `post_edge` of each phase always includes an auto-generated phase advance:

```
phase_r <= (phase_r + 1) % N   // wraps back to 0 from N-1
```

This advance is appended by Phase C after processing user-written register updates for that phase.

The output drive becomes conditional on phase — see the Output Drive Model section.

---

## Output Drive Model

`emit!()` in Copper drives the module's output port. Its position relative to tick boundaries determines when the output is valid. SHIR makes this explicit with the `SHIROutputDrive` type.

```rust
pub enum SHIROutputDrive {
    // Single-tick, emit before tick (Pattern A from ASYNC_AWAIT_SEMANTICS.md)
    // Emits: assign out = <expr>;
    // The expression may reference registers or wires from the pre-edge segment
    PreEdge(SHIRExpr),

    // Single-tick, emit after tick (Pattern B)
    // Emits: assign out = <reg_name>;  (the register just updated)
    PostEdge(SHIRExpr),

    // Multi-tick: output depends on which phase is active
    // Phase C generates: assign out = (phase_r == K) ? expr_K : ... : expr_default;
    // or equivalent always_comb with a case statement
    PhaseConditional {
        arms: Vec<SHIRPhaseOutputArm>,
        // Value to drive when no emit fired in current phase (holds last value)
        // None = undefined (should not happen if Phase B invariant 6 is met)
        default: Option<SHIRExpr>,
    },

    // Combinational module: output is always active
    Continuous(SHIRExpr),
}

pub struct SHIRPhaseOutputArm {
    pub phase_idx: usize,
    pub value: SHIRExpr,
}
```

**Why output drive is separate from `post_edge`:**

This is the core architectural insight from prior work. In Copper, `emit!(value)` in simulation writes to an `Arc<Mutex<T>>` which is read by the harness after `tick_clock` returns. This maps to a **continuous wire assignment**, not a register update. If the emitted value were placed inside `always_ff` as a non-blocking assign, the output would lag one extra cycle.

The correct Verilog is:
```verilog
assign out = count;   // continuous, not inside always_ff
```

Not:
```verilog
always_ff @(posedge clk) begin
    out <= count;     // WRONG: one cycle late
end
```

SHIR encodes this distinction by keeping `SHIROutputDrive` separate from `SHIRPhase::post_edge`. Phase E always emits the output drive as `assign out = ...` outside any always block.

---

## Statement Model

SHIR statements appear in `SHIRPhase::pre_edge`. They represent combinational logic and are emitted as `always_comb` or as intermediate `assign` statements.

```rust
pub enum SHIRStmt {
    // Combinational wire (will become a local wire or logic declaration)
    Wire {
        name: String,
        ty: CHIRType,
        value: SHIRExpr,
    },

    // Conditional branching within a timing region
    If {
        condition: SHIRExpr,
        then_stmts: Vec<SHIRStmt>,
        else_stmts: Option<Vec<SHIRStmt>>,
    },

    // Pattern match (tuple patterns preserved from CHIR)
    Match {
        scrutinee: SHIRExpr,
        arms: Vec<SHIRMatchArm>,
    },
}

pub struct SHIRMatchArm {
    pub patterns: Vec<SHIRPattern>,
    pub guard: Option<SHIRExpr>,
    pub stmts: Vec<SHIRStmt>,
}

pub enum SHIRPattern {
    Lit(SHIRLit),
    Wildcard,
    Tuple(Vec<SHIRPattern>),
    EnumVariant { name: String, inner: Option<Box<SHIRPattern>> },
}
```

Register updates are separate from statements and do not appear in `pre_edge`:

```rust
pub struct SHIRRegUpdate {
    pub target: String,
    pub next_value: SHIRExpr,
    // Conditional updates (from if/match blocks) are represented as Mux expressions
    // on next_value, not as nested SHIRStmts
}
```

**Why register updates use `Mux` rather than nested `If`:**

Having register updates be flat `target <= expr` makes Phase D much simpler — it can emit the always_ff block as a simple list without needing to re-traverse nested structures. Conditional register updates from `if/match` in source code are converted to `SHIRExpr::Mux` chains during Phase C lowering:

```rust
// Source:
if cond { x = a; } else { x = b; }

// In SHIR post_edge:
SHIRRegUpdate { target: "x", next_value: Mux(cond, a, b) }
```

For match arms over registers, Phase C generates a `SHIRExpr::Case` that selects the next value.

---

## Expression Model

SHIR reuses the CHIR expression model with one addition: `SHIRExpr::PhaseEq` for phase-conditional logic.

```rust
pub enum SHIRExpr {
    Var(String),
    Lit(SHIRLit),
    BinOp { left: Box<SHIRExpr>, op: CHIRBinOp, right: Box<SHIRExpr> },
    UnOp { op: CHIRUnOp, expr: Box<SHIRExpr> },
    Mux { cond: Box<SHIRExpr>, then_val: Box<SHIRExpr>, else_val: Box<SHIRExpr> },
    Case { scrutinee: Box<SHIRExpr>, arms: Vec<SHIRCaseArm>, default: Box<SHIRExpr> },
    Concat(Vec<SHIRExpr>),
    Slice { expr: Box<SHIRExpr>, high: usize, low: usize },

    // Used in PhaseConditional output drives to compare phase_r
    // Phase D emits this as: phase_r == <idx>
    PhaseEq(usize),
}

pub struct SHIRCaseArm {
    pub pattern: SHIRPattern,
    pub value: SHIRExpr,
}

pub struct SHIRLit {
    pub ty: CHIRType,
    pub value: u128,
}
```

---

## Worked Examples

### Example 1: Counter (single-tick, emit-before-tick)

**Source:**
```rust
async fn counter(clk: Clock<MainClk>, in_step: u8) -> u8 {
    let mut count: u8 = 0;
    loop {
        emit!(count);
        clk.tick().await;
        count = count.wrapping_add(in_step);
    }
}
```

**SHIR:**
```
SHIRSeqBody {
  clock: "clk",
  registers: [SHIRReg { name: "count", ty: UInt<8>, init: 0 }],
  phases: [
    SHIRPhase {
      phase_idx: 0,
      pre_edge: [],           // no wires before tick
      post_edge: [
        SHIRRegUpdate { target: "count", next_value: BinOp(Var("count"), Add{wrapping:true}, Var("in_step")) }
      ]
    }
  ],
  output_drive: PreEdge(Var("count"))
}
```

**Emitted Verilog (preview — Phase E output):**
```verilog
always_ff @(posedge clk) begin
    count <= count + in_step;
end
assign out = count;
```

---

### Example 2: JK Flip-Flop (single-tick, match with tuple pattern)

**Source:**
```rust
async fn jk_ff(j: Arc<Mutex<Bit>>, k: Arc<Mutex<Bit>>, clk: Clock<MainClk>) -> Bit {
    let mut q: Bit = Bit::ZERO;
    loop {
        match (*j.lock().unwrap(), *k.lock().unwrap()) {
            (Bit::ZERO, Bit::ZERO) => {}
            (Bit::ZERO, Bit::ONE)  => { q = Bit::ZERO; }
            (Bit::ONE,  Bit::ZERO) => { q = Bit::ONE; }
            (Bit::ONE,  Bit::ONE)  => { q = if q == Bit::ZERO { Bit::ONE } else { Bit::ZERO }; }
            _ => { q = Bit::X; }
        }
        emit!(q);
        clk.tick().await;
    }
}
```

**SHIR:**
```
SHIRSeqBody {
  clock: "clk",
  registers: [SHIRReg { name: "q", ty: UInt<1>, init: 0 }],
  phases: [
    SHIRPhase {
      phase_idx: 0,
      pre_edge: [],
      post_edge: [
        SHIRRegUpdate {
          target: "q",
          next_value: Case {
            scrutinee: Concat([Var("j"), Var("k")]),
            arms: [
              // (0, 0) → hold
              SHIRCaseArm { pattern: Tuple([Lit(0), Lit(0)]), value: Var("q") },
              // (0, 1) → reset
              SHIRCaseArm { pattern: Tuple([Lit(0), Lit(1)]), value: Lit(0) },
              // (1, 0) → set
              SHIRCaseArm { pattern: Tuple([Lit(1), Lit(0)]), value: Lit(1) },
              // (1, 1) → toggle
              SHIRCaseArm { pattern: Tuple([Lit(1), Lit(1)]), value: Mux(BinOp(Var("q"), Eq, Lit(0)), Lit(1), Lit(0)) },
            ],
            default: Lit(0),   // _ arm (X maps to 0 in synthesis)
          }
        }
      ]
    }
  ],
  output_drive: PreEdge(Var("q"))
}
```

**Notes:**
- The tuple match scrutinee becomes `Concat([j, k])` in SHIR — Phase D emits this as `{j, k}` in the Verilog `case` selector
- The hold arm `(0,0)` becomes `next_q = q` (self-assignment) which synthesis tools optimize away
- Phase D will emit the `Case` as a `casez` or `case` statement in `always_comb` with the concatenated selector

---

### Example 3: 2-Stage Pipeline (single-tick, multiple registers, post-edge intermediate)

**Source:**
```rust
async fn registered_pipeline(clk: Clock<MainClk>, in_data: Arc<Mutex<u8>>) -> u8 {
    let mut stage1_r: u8 = 0;
    let mut stage2_r: u8 = 0;
    loop {
        emit!(stage2_r);
        clk.tick().await;
        let stage1_data = in_data.wrapping_add(1);      // wire: not across tick
        let stage2_data = stage1_r.wrapping_add(stage1_r);
        stage1_r = stage1_data;
        stage2_r = stage2_data;
    }
}
```

**SHIR:**
```
SHIRSeqBody {
  clock: "clk",
  registers: [
    SHIRReg { name: "stage1_r", ty: UInt<8>, init: 0 },
    SHIRReg { name: "stage2_r", ty: UInt<8>, init: 0 },
  ],
  phases: [
    SHIRPhase {
      phase_idx: 0,
      pre_edge: [
        // Post-tick wires are still placed in pre_edge — they are combinational
        // values that feed the register updates
        Wire { name: "stage1_data", ty: UInt<8>, value: BinOp(Var("in_data"), Add{w}, Lit(1)) },
        Wire { name: "stage2_data", ty: UInt<8>, value: BinOp(Var("stage1_r"), Add{w}, Var("stage1_r")) },
      ],
      post_edge: [
        SHIRRegUpdate { target: "stage1_r", next_value: Var("stage1_data") },
        SHIRRegUpdate { target: "stage2_r", next_value: Var("stage2_data") },
      ]
    }
  ],
  output_drive: PreEdge(Var("stage2_r"))
}
```

**Notes on wire placement:** The wires `stage1_data` and `stage2_data` are declared in the post-tick segment of the source, but they are still combinational values — they feed register updates and do not themselves cross a tick boundary. They go in `pre_edge` because `pre_edge` is where all combinational logic lives regardless of whether the source wrote them before or after the tick.

---

### Example 4: Multi-tick module (two phases)

**Source:**
```rust
async fn two_cycle_op(clk: Clock<MainClk>, input: u8) -> u8 {
    let mut acc: u8 = 0;
    loop {
        emit!(acc);
        clk.tick().await;        // phase 0 → phase 1
        let step1 = input.wrapping_add(1);
        clk.tick().await;        // phase 1 → phase 0
        acc = step1.wrapping_add(step1);
    }
}
```

**Segment → phase mapping (n_ticks = 2):**
```
seg_0  (emit!(acc))           → phase_0  pre_edge / output
seg_1  (let step1 = ...)      → phase_1  pre_edge
seg_2  (acc = step1 + step1)  → phase_1  post_edge  [trailing: maps to phase_{N-1}]
```

`step1` is declared in seg_1 and consumed in seg_2. Both segments map to **phase_1** — the same hardware clock cycle. `step1` stays a combinational wire: it is computed in phase_1's `always_comb` and read by phase_1's `always_ff` at the same clock edge. No register promotion is needed.

**SHIR:**
```
SHIRSeqBody {
  clock: "clk",
  registers: [
    SHIRReg { name: "acc",     ty: UInt<8>, init: 0 },
    SHIRReg { name: "phase_r", ty: UInt<1>, init: 0 },  // auto-generated
    // step1 is NOT promoted — both declaration and use are in phase_1
  ],
  phases: [
    SHIRPhase {
      phase_idx: 0,
      pre_edge: [],
      post_edge: [
        // no user register updates in seg_0
        SHIRRegUpdate { target: "phase_r", next_value: Lit(1) },  // phase advance
      ]
    },
    SHIRPhase {
      phase_idx: 1,
      pre_edge: [
        Wire { name: "step1", ty: UInt<8>, value: BinOp(Var("input"), Add{w}, Lit(1)) },
      ],
      post_edge: [
        SHIRRegUpdate { target: "acc", next_value: BinOp(Var("step1"), Add{w}, Var("step1")) },
        SHIRRegUpdate { target: "phase_r", next_value: Lit(0) },  // phase wrap
      ]
    },
  ],
  output_drive: PhaseConditional {
    arms: [
      SHIRPhaseOutputArm { phase_idx: 0, value: Var("acc") },
    ],
    default: None,  // only phase 0 emits
  }
}
```

**Emitted Verilog (preview):**
```verilog
always_comb begin
    step1 = input + 1;
end
always_ff @(posedge clk) begin
    if (phase_r == 1) begin
        acc     <= step1 + step1;  // reads the combinational wire — correct
        phase_r <= 0;
    end else begin
        phase_r <= 1;
    end
end
assign out = (phase_r == 0) ? acc : acc;  // only phase 0 arm; Phase D simplifies
```

**Why `acc` uses `Var("step1")` and not a promoted `Var("step1_r")`:** Verilog non-blocking assigns evaluate all RHS expressions simultaneously at the clock edge. A wire (computed in `always_comb`) is settled and stable at that moment, so `acc <= step1 + step1` correctly reads the current-cycle `step1`. If we had instead promoted `step1` to a register and written `acc <= step1_r + step1_r`, the `step1_r` on the RHS would be the *old* value from the previous phase_1 cycle — introducing one extra cycle of phantom latency and breaking the async semantics contract.

---

## Register Promotion Rule

Phase C promotes a `Wire` to a register only when its declaration and its use fall in **different hardware phases**. Two segments that map to the same phase index can share a combinational wire without a register.

The algorithm:

1. Assign each `Wire` declaration to its source segment index `J`.
2. Compute the hardware phase for each segment: `phase(K) = min(K, N-1)` where N = n_ticks.
3. For each expression in segment K that references a wire name declared in segment J:
   - If `phase(J) == phase(K)`: no promotion needed — the wire is a combinational value valid throughout that clock cycle.
   - If `phase(J) != phase(K)`: promote the wire to a `SHIRReg` named `<name>_r`, add `SHIRRegUpdate { target: "<name>_r", next_value: Var("<name>") }` to segment J's phase post_edge, and replace references to `<name>` in segments with `phase > phase(J)` with `Var("<name>_r")`.

**Why phase-based, not segment-based:** A naïve segment-based rule (promote if J < K) would also promote wires shared between a pre-tick segment and its own trailing post-tick segment (both in the same hardware phase). That would generate unnecessary registers and — critically — would cause `acc <= step1_r + step1_r` to read the *old* step1_r due to Verilog non-blocking semantics, breaking the observable equivalence with the Copper simulation. The phase-based rule avoids this class of bug entirely.

---

## Phase C Validation

Phase C should verify the following invariants on CHIR input before processing:

1. **No tick inside branch:** `AwaitTick` appears only at the flat top level of `loop_body`. Nested ticks (inside `If` or `Match` arms) are rejected with `TickInsideBranch`.
2. **At least one tick:** Sequential modules must have at least one `AwaitTick`. A sequential module with no tick is rejected.
3. **Clock name consistency:** All `AwaitTick` nodes reference the same clock name as declared in `CHIRSeqBody::clock`. (Cross-clock awaiting is unsupported in Milestone 1.)
4. **Emit before non-output use:** An `Emit` that references a `Wire` name must reference a wire declared earlier in the same segment.

---

## Equivalence Contract with Copper Simulation

The key invariant SHIR must satisfy (from Decision 2 in `TRANSPILATION_PLAN.md`):

> For every test vector `(cycle, inputs, expected_output)` in a Copper simulation trace, the SHIR-derived Verilog module must produce `expected_output` given `inputs` at `cycle`.

The timing mapping that makes this work:

| Copper simulation event | SHIR timing region |
|---|---|
| Pre-edge `poll_tasks` — emit fires | `SHIROutputDrive::PreEdge` drives `out` |
| `clk.advance()` — clock edge | `always_ff @(posedge clk)` — `post_edge` updates fire |
| Post-edge `poll_tasks` — emit fires | `SHIROutputDrive::PostEdge` drives `out` |
| Reading `*output.lock().unwrap()` | Reading `out` port in testbench |

For multi-tick modules:
- Cycle N is when `phase_r == 0`; cycle N+1 when `phase_r == 1`, etc.
- The output on cycle N is the `PhaseConditional` arm for phase 0
- Equivalence testing must account for the startup latency of the phase counter reaching its steady state

---

## Summary: Phase C Contract

**Input:** `CHIRModule` (from Phase B semantic lowering)

**Output:** `SHIRModule` containing:
- Ports with resolved types
- Register declarations including any auto-generated `phase_r` and promoted intermediate registers
- Submodule instances carried through from CHIR, with output wires available as `Var` in all timing regions
- Per-phase timing regions with explicit pre-edge wires and post-edge register updates
- An explicit `SHIROutputDrive` with no ambiguity about when the output is driven
- All conditional register updates expressed as `Mux`/`Case` expressions on `next_value`, not as nested statement trees

**Invariants guaranteed by Phase C:**
1. Every register named in a `SHIRRegUpdate::target` is declared in `SHIRSeqBody::registers`
2. Every variable referenced in a `SHIRExpr` is either a port, a declared register, a `Wire` declared earlier in the same `pre_edge` list, or a submodule `output_wire` in `SHIRSeqBody::submodules`
3. Assignment intent is fully determined: no statement is ambiguous about blocking vs non-blocking vs continuous
4. `SHIROutputDrive` is always `Continuous` for combinational modules, and always `PreEdge`, `PostEdge`, or `PhaseConditional` for sequential modules — never absent for a module with an output port
5. `phase_r` is present if and only if `phases.len() > 1`
6. Each `SHIRPhase::post_edge` ends with a phase-advance `SHIRRegUpdate` if and only if `phases.len() > 1`
7. No `AwaitTick` or `Emit` appear in the output — these are consumed by Phase C and do not exist in SHIR
8. `SHIRSubmoduleInst` entries are passed through unchanged from CHIR — Phase C does not transform them
