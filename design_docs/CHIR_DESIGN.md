# Copper Canonical Hardware IR (CHIR) Design

## Purpose

This document defines the design of the Canonical Hardware Intermediate Representation (CHIR) for Copper — Phase B output in the transpilation pipeline defined in `TRANSPILATION_PLAN.md`.

CHIR receives a `FrontendModuleIR` (Phase A output) and outputs a normalized, hardware-semantic representation suitable for Phase C scheduling and Phase D legalization. CHIR is the "hardware meaning" layer: source intent has been parsed, but timing has not yet been fixed. The goal is to make hardware semantics explicit while leaving scheduling decisions to Phase C.

---

## Prior Art Summary

Three systems were studied: FIRRTL, Calyx, and CIRCT. The relevant takeaways for Copper are:

**From FIRRTL:** A flat module/port/statement model is sufficient for RTL. Ports carry explicit direction and type. Registers require explicit clock binding. The `node` statement (a named combinational value) is a clean primitive that Copper needs an equivalent of. FIRRTL's `when` (conditional) maps naturally to Rust `if`.

**From Calyx:** The explicit separation between data (cells/wires) and control (seq/par/while/if) is a valuable architectural insight. Calyx makes the "scheduling intent" first-class rather than baking it into wire connectivity. For Copper, the async/await loop body is exactly what Calyx's control language is designed to represent. The key idea: **identify groups/segments of operations and express their sequencing separately from what the operations do**.

**From CIRCT:** Treating clock as a first-class value (SSA operand) rather than a hidden metadata attribute is clean and correct. The `comb`/`seq` dialect split — where combinational ops and sequential ops are categorically separated — directly informs how Copper should partition statements in its IR. The `seq.compreg` abstraction (a reset-agnostic register whose value is determined by a next-value operand) is the right conceptual model for Copper registers.

---

## What CHIR Is and Is Not

**CHIR is:**
- A hardware-semantic representation of a single Copper module
- Hardware-typed (widths are resolved; Rust types are mapped to `UInt<N>`, `Bool`, etc.)
- Clock-domain-aware (clock is explicit, domain is tagged)
- Tick-boundary-explicit (the `await` point becomes a first-class `AwaitTick` op)
- Free of Rust-runtime types (`Arc`, `Mutex`, `Box`, lifetimes)
- Normalizing for expressions (Rust method calls like `wrapping_add` → canonical CHIR ops)
- The correct level for register/wire classification

**CHIR is not:**
- Scheduled (pre-edge vs post-edge is not decided yet — that is Phase C)
- Verilog-legal (no keyword sanitization, no assignment style decisions)
- Optimized
- Multi-module (CHIR is per-module; composition is handled later)

---

## Core Data Model

The proposed Rust types below are for design purposes. They are the target of the Phase B implementation.

```rust
// Top-level module in CHIR
pub struct CHIRModule {
    pub name: String,
    pub ports: Vec<CHIRPort>,
    pub body: CHIRBody,
    pub span: SourceSpan,
}

// Ports — clock is a port kind, not a magic parameter
pub struct CHIRPort {
    pub name: String,
    pub direction: CHIRPortDir,
    pub kind: CHIRPortKind,
    pub span: SourceSpan,
}

pub enum CHIRPortDir {
    Input,
    Output,
}

pub enum CHIRPortKind {
    Clock { domain: String },
    Data { ty: CHIRType },
}

// Type system: hardware-native only
pub enum CHIRType {
    UInt { width: usize },
    SInt { width: usize },
    Bool,
}

// Two fundamental module bodies
pub enum CHIRBody {
    Combinational(CHIRCombBody),
    Sequential(CHIRSeqBody),
}

// Combinational: named wire values, submodule instances, and output drive
pub struct CHIRCombBody {
    // #[hardware] submodule instantiations, in source order
    pub submodules: Vec<CHIRSubmoduleInst>,
    // Named intermediate wire values, in computation order
    pub wires: Vec<CHIRWireDecl>,
    // Drives the output port
    pub output: CHIRExpr,
}

pub struct CHIRWireDecl {
    pub name: String,
    pub ty: CHIRType,
    pub value: CHIRExpr,
    pub span: SourceSpan,
}

// Sequential: registers + submodule instances + loop body
pub struct CHIRSeqBody {
    // Clock parameter name this module is clocked on
    pub clock: String,

    // State registers (live across tick boundary)
    pub registers: Vec<CHIRRegDecl>,

    // #[hardware] submodule instantiations used in this module
    // These are wired in combinationally — their outputs are wires, not registers
    pub submodules: Vec<CHIRSubmoduleInst>,

    // The infinite loop body — contains AwaitTick as an explicit op
    // Phase C will split this at the AwaitTick boundary
    pub loop_body: Vec<CHIRStmt>,
}

pub struct CHIRRegDecl {
    pub name: String,
    pub ty: CHIRType,
    // Initial value for simulation; not emitted in synthesis output
    pub init: Option<CHIRLit>,
    pub span: SourceSpan,
}

// A #[hardware] combinational or sequential submodule instantiated at a call site.
// The call `let result = full_adder(a, b)` becomes:
//   - a CHIRSubmoduleInst with a generated inst_name and output_wire name
//   - a CHIRStmt::Wire (or CHIRWireDecl) that aliases output_wire
//   - all references to `result` replaced with Var(output_wire)
pub struct CHIRSubmoduleInst {
    // Unique instance name within this module (e.g. "full_adder_0")
    pub inst_name: String,

    // The name of the #[hardware] module being instantiated (e.g. "full_adder")
    pub module_name: String,

    // Input port connections: (port_name, driving_expr)
    pub inputs: Vec<(String, CHIRExpr)>,

    // Name of the wire carrying this instance's output in the parent module
    pub output_wire: String,

    // Type of the output
    pub output_ty: CHIRType,

    pub span: SourceSpan,
}
```

---

## Statement Model

The statement list in `CHIRSeqBody::loop_body` contains operations in source order. The `AwaitTick` statement is the explicit tick boundary. Phase C will walk this list and split it.

```rust
pub enum CHIRStmt {
    // Declare a wire (combinational value, does not cross tick boundary)
    Wire {
        name: String,
        ty: CHIRType,
        value: CHIRExpr,
        span: SourceSpan,
    },

    // Assign to a register (always a register — Phase B resolves this)
    Assign {
        target: String,
        value: CHIRExpr,
        span: SourceSpan,
    },

    // Drive the output port (from emit!())
    // Semantics: whatever value is emitted last before AwaitTick wins
    Emit {
        value: CHIRExpr,
        span: SourceSpan,
    },

    // The clock-edge boundary — corresponds to clk.tick().await
    // Contains the clock name it waits on (for multi-clock future support)
    AwaitTick {
        clock: String,
        span: SourceSpan,
    },

    // Conditional branching
    If {
        condition: CHIRExpr,
        then_body: Vec<CHIRStmt>,
        else_body: Option<Vec<CHIRStmt>>,
        span: SourceSpan,
    },

    // Pattern matching
    Match {
        scrutinee: CHIRExpr,
        arms: Vec<CHIRMatchArm>,
        span: SourceSpan,
    },
}

pub struct CHIRMatchArm {
    pub patterns: Vec<CHIRPattern>,
    pub guard: Option<CHIRExpr>,
    pub body: Vec<CHIRStmt>,
    pub span: SourceSpan,
}

pub enum CHIRPattern {
    Lit(CHIRLit),
    Wildcard,
    Tuple(Vec<CHIRPattern>),
    EnumVariant { name: String, inner: Option<Box<CHIRPattern>> },
}
```

---

## Expression Model

All Rust method calls and type-specific operations are normalized to canonical CHIR ops at Phase B. No method calls, closures, or trait dispatch appear in CHIR expressions.

```rust
pub enum CHIRExpr {
    // Variable reference (wire, register, or submodule output wire name)
    Var(String),

    // Literal constant
    Lit(CHIRLit),

    // Binary operation
    BinOp {
        left: Box<CHIRExpr>,
        op: CHIRBinOp,
        right: Box<CHIRExpr>,
    },

    // Unary operation
    UnOp {
        op: CHIRUnOp,
        expr: Box<CHIRExpr>,
    },

    // Multiplexer (ternary) — from if-as-expression
    Mux {
        cond: Box<CHIRExpr>,
        then_val: Box<CHIRExpr>,
        else_val: Box<CHIRExpr>,
    },

    // Match used as a value expression (let x = match { ... })
    // Distinct from CHIRStmt::Match which is a match used as a statement
    Case {
        scrutinee: Box<CHIRExpr>,
        arms: Vec<CHIRCaseArm>,
        default: Option<Box<CHIRExpr>>,
    },

    // Bit concatenation {a, b, c}
    Concat(Vec<CHIRExpr>),

    // Bit slice [high:low] — both bounds are compile-time constants at this level
    Slice {
        expr: Box<CHIRExpr>,
        high: usize,
        low: usize,
    },
}

pub struct CHIRCaseArm {
    pub pattern: CHIRPattern,
    pub guard: Option<CHIRExpr>,
    pub value: CHIRExpr,
}

pub struct CHIRLit {
    pub ty: CHIRType,
    pub value: u128,
}

pub enum CHIRBinOp {
    // Arithmetic
    Add { wrapping: bool },
    Sub { wrapping: bool },
    Mul { wrapping: bool },

    // Bitwise
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,

    // Comparison — always produce Bool
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,

    // Logical
    LogicalAnd,
    LogicalOr,
}

pub enum CHIRUnOp {
    BitNot,
    LogicalNot,
    Neg,
    ReductionAnd,
    ReductionOr,
    ReductionXor,
}
```

---

## The Async/Await Timing Model

This is the most critical design constraint in CHIR and the primary reason CHIR exists as a separate phase.

### Why the tick boundary must be explicit in CHIR

The FrontendIR preserves `ExprAwait` as a raw expression. CHIR promotes the tick boundary to a first-class statement, `AwaitTick`. This is the right representation for Phase B because:

1. Phase C (timing/scheduling) needs to find the boundary without re-parsing expression trees
2. CHIR statements before `AwaitTick` are classified as pre-edge candidates; statements after are post-edge candidates — but this classification is Phase C's job, not Phase B's
3. Nested `AwaitTick` inside `If` or `Match` arms is explicitly illegal in CHIR (Phase B rejects it) — a tick boundary must occur at the flat top level of the loop body only

### How the loop body is structured

For Milestone 1, CHIR assumes the canonical Copper sequential module structure:

```
loop {
    [pre-tick statements: Wire, Assign, Emit, If, Match]
    AwaitTick { clock }
    [post-tick statements: Wire, Assign, If, Match]
}
```

The single `AwaitTick` in `loop_body` is the clock edge boundary. Everything before it in the list is "pre-edge source order"; everything after is "post-edge source order". Phase C consumes this list and produces explicit pre/post timing regions (SHIR).

### What happens to `emit!()` in this model

`emit!()` maps to `CHIRStmt::Emit`. The semantics preserved from the runtime:
- `emit!()` before `AwaitTick` → drives the output in the pre-edge phase (Pattern A from `ASYNC_AWAIT_SEMANTICS.md`)
- `emit!()` after `AwaitTick` → drives the output in the post-edge phase (Pattern B)
- Multiple `emit!()` calls before the tick → last one wins (Phase B emits a warning; this is unusual)

In Phase D/E emission, `Emit` becomes a continuous `assign out = value` wire from the emitted expression, NOT a non-blocking assign inside the always_ff block. This was the core semantic finding from prior work.

### Wire vs Register classification in Phase B

Phase B must resolve which local variables are **registers** (cross the tick boundary) vs **wires** (computed within a single phase):

- A variable is a **register** if it is declared before the `loop` and assigned inside the loop
- A variable is a **wire** if it is declared inside the loop and never assigned across a tick boundary
- A variable that is declared inside the loop but read before it is assigned in the same segment is a compile error

This classification happens in Phase B and is encoded in CHIR: register names appear in `CHIRSeqBody::registers`; wire names appear in `CHIRStmt::Wire`.

---

## `#[hardware]` Attribute Semantics

The `#[hardware]` attribute is the explicit boundary between Rust utility code and synthesizable hardware modules. Phase B uses this to decide whether a function call site becomes an inlined expression or a submodule instantiation.

| Function | Attribute | Phase B treatment |
|---|---|---|
| `fn foo(...) -> T` | none | Inlined into the parent expression tree |
| `fn foo(...) -> T` | `#[hardware]` | Combinational submodule — instantiated as `CHIRSubmoduleInst` |
| `async fn foo(...) -> T` | `#[hardware]` | Sequential submodule — instantiated as `CHIRSubmoduleInst` |

**Inlining (no attribute):**
Plain Rust utility functions with no `#[hardware]` attribute are inlined by Phase B. Their body is substituted into the parent expression tree with arguments replaced. This is only valid if the body contains no hardware-only constructs (`emit!`, `tick().await`, `Arc<Mutex<T>>`). If it does, Phase B rejects it with a diagnostic suggesting `#[hardware]` be added.

**Submodule instantiation (`#[hardware]`):**
A call to a `#[hardware]` function becomes a `CHIRSubmoduleInst`. The call site is replaced with a `Var` referencing the instance's generated output wire. Phase B generates a unique instance name (e.g. `full_adder_0`, `full_adder_1` for multiple calls to the same module).

```rust
// Source:
let sum = full_adder(a, b);
let doubled = full_adder(sum, sum);

// CHIR submodules:
CHIRSubmoduleInst { inst_name: "full_adder_0", module_name: "full_adder",
    inputs: [("a", Var("a")), ("b", Var("b"))], output_wire: "full_adder_0_out", ... }
CHIRSubmoduleInst { inst_name: "full_adder_1", module_name: "full_adder",
    inputs: [("a", Var("full_adder_0_out")), ("b", Var("full_adder_0_out"))],
    output_wire: "full_adder_1_out", ... }

// `sum` and `doubled` become Wire decls aliasing the output wires
```

**`function_typed` is legacy:**
`#[hardware(function_typed)]` on an async fn is treated identically to `#[hardware]` on an async fn. The distinction is no longer meaningful — `async` alone signals sequential. `function_typed` will be removed in a future cleanup once all examples are migrated.

---

## Mapping from FrontendModuleIR to CHIR

| FrontendIR concept | CHIR concept |
|---|---|
| `FrontendClassification::CombinationalFn` | `CHIRBody::Combinational` |
| `FrontendClassification::AsyncSequentialFn` | `CHIRBody::Sequential` |
| `ClockParamMeta` | `CHIRPort { kind: Clock }` + `CHIRSeqBody::clock` |
| `RawParam` (non-clock) | `CHIRPort { kind: Data }` |
| `return_ty` | Implicit output port `out` with resolved `CHIRType` |
| `LocalStmt` (before loop, `is_mut: true`) | `CHIRRegDecl` |
| `LocalStmt` (inside loop) | `CHIRStmt::Wire` |
| `ExprLoop` | `CHIRSeqBody::loop_body` |
| `ExprAwait` on a clock | `CHIRStmt::AwaitTick` |
| Macro call `emit!(x)` | `CHIRStmt::Emit` |
| `ExprAssign` to a register | `CHIRStmt::Assign` |
| `ExprIf` | `CHIRStmt::If` |
| `ExprMatch` | `CHIRStmt::Match` |
| `ExprMethodCall` on a Copper type | Normalized to `CHIRExpr::BinOp` or `UnOp` |
| `ExprMethodCall::wrapping_add` | `CHIRBinOp::Add { wrapping: true }` |

---

## Type Resolution in Phase B

Phase B is responsible for mapping Copper source types to `CHIRType`. The rules:

| Copper source type | CHIRType |
|---|---|
| `u8`, `u16`, `u32`, `u64`, `u128` | `UInt { width: 8/16/32/64/128 }` |
| `i8`, `i16`, `i32`, `i64`, `i128` | `SInt { width: 8/16/32/64/128 }` |
| `bool` | `Bool` |
| `Logic` | `UInt { width: 1 }` |
| `Bits<N>` | `UInt { width: N }` |
| `Logic` | `UInt { width: 1 }` (X becomes simulation-only) |
| `Arc<Mutex<T>>` | Resolved to the `T` type (Arc/Mutex stripped) |
| `Clock<D>` | Not a data type — becomes `CHIRPortKind::Clock` |

Ambiguous widths (e.g., a bare integer literal without type annotation) are a Phase B compile error per Decision 6 in `TRANSPILATION_PLAN.md`.

---

## Error Model

Phase B errors (lowering from FIR to CHIR) should be structured:

```rust
pub enum CHIRLowerError {
    // A Rust construct that cannot be mapped to hardware
    UnsupportedConstruct {
        description: String,
        span: SourceSpan,
        suggested_rewrite: Option<String>,
    },

    // A type that cannot be resolved to a CHIRType
    UnresolvableType {
        ty_text: String,
        span: SourceSpan,
    },

    // A variable used as a register and a wire in the same context
    RegisterWireConflict {
        name: String,
        span: SourceSpan,
    },

    // AwaitTick found inside a conditional branch (not supported in Milestone 1)
    TickInsideBranch {
        span: SourceSpan,
    },

    // emit!() used in a module with no output port
    EmitWithoutOutput {
        span: SourceSpan,
    },

    // Module has an output port but never calls emit!()
    OutputWithoutEmit {
        span: SourceSpan,
    },

    // Width inference failure
    AmbiguousWidth {
        span: SourceSpan,
    },
}
```

---

## Design Decisions

These questions were resolved in design review. Decisions are recorded here as normative.

---

### D1: `while` loops in sequential modules — REJECTED with rewrite suggestion

Phase B rejects any async sequential module that does not use `loop` as its top-level structure. A `while` loop in sequential position produces a `UnsupportedConstruct` error with a diagnostic suggesting rewrite to `loop { if !cond { break; } ... }` or equivalent. This keeps Phase C analysis simple for Milestone 1.

---

### D2: Tuple patterns in `match` — PRESERVED in CHIR

CHIR preserves tuple patterns rather than desugaring them. The `CHIRPattern` enum includes:

```rust
pub enum CHIRPattern {
    Lit(CHIRLit),
    Wildcard,
    Tuple(Vec<CHIRPattern>),
    EnumVariant { name: String, inner: Option<Box<CHIRPattern>> },
}
```

**Rationale:** Tuple patterns (e.g. `jk_ff`'s `(J, K)` match) should emit as a Verilog `case` on a concatenated selector for readability and synthesis quality. Desugaring in Phase B would produce an `if/else` chain that loses this structure. Phase D is responsible for concatenating the tuple scrutinee components and emitting a `casez` or `case` statement.

---

### D3: Overflow behavior — wrapping annotated, saturating/checked unsupported

`wrapping_add` maps to `CHIRBinOp::Add { wrapping: true }`. `saturating_add` and `checked_add` are unsupported in Phase B and produce a `UnsupportedConstruct` error. A plain `+` operator on hardware-typed integers in CHIR always means modular-at-width. The `wrapping` annotation is informational — the hardware behavior is identical either way.

---

### D4: `Arc<Mutex<T>>` stripping — Phase B type resolution

Phase B strips `Arc<Mutex<T>>` wrappers during type resolution. A parameter of type `Arc<Mutex<T>>` in the FrontendIR becomes a plain `CHIRPort { kind: Data { ty: resolve(T) } }` in CHIR. No `Arc`/`Mutex` representation exists anywhere in CHIR. Phase A preserves the raw type text as-is.

---

### D5: Multiple `clk.tick().await` per loop — SUPPORTED

CHIR supports multiple `AwaitTick` nodes in `loop_body`. A loop body with N tick boundaries takes exactly N clock cycles per iteration and maps to a multi-phase state machine.

**Semantics of multiple ticks:**

Each `clk.tick().await` is a full clock cycle boundary. A loop body with two tick calls executes over 2 cycles per iteration. Phase C segments the loop body at each `AwaitTick`, numbers the segments 0..N, and generates an implicit `phase` register. The resulting hardware advances one phase per clock edge:

```
phase 0: execute segment 0 statements, advance to phase 1
phase 1: execute segment 1 statements, advance to phase 0
```

In generated Verilog this becomes `case (phase_r)` inside `always_ff`. The implicit `phase_r` register is introduced by Phase C — it does not appear in CHIR. CHIR only records the flat `AwaitTick`-delimited statement sequence.

An `Emit` statement in segment K drives the output only during cycle phase K. Phase C must generate the output drive conditional on `phase_r == K`.

---

## Relationship to Existing IR

The existing `copper-core/src/ir.rs` (`ModuleIR`, `Statement`, `Expression`) is currently marked as potentially deprecated. CHIR replaces it for the transpilation pipeline. The existing `ir.rs` types may be retired once CHIR covers the full feature set, or kept for backward compatibility with other tooling.

The existing `copper-core/src/frontend_ir.rs` is Phase A output and is CHIR's input — these two are coordinated and should not be conflated.

---

## Summary: Phase B Contract

**Input:** `FrontendModuleIR` (from Phase A capture)

**Output:** `CHIRModule` containing:
- Resolved port types and directions
- Register declarations with widths and init values
- Loop body as a flat `Vec<CHIRStmt>` with one or more `AwaitTick` boundaries
- Tuple match patterns preserved as `CHIRPattern::Tuple`
- All expressions normalized to `CHIRExpr` with no Rust-runtime types
- Structured errors with source spans for any unsupported constructs

**Invariants guaranteed by Phase B:**
1. No `Arc`, `Mutex`, `Box`, lifetimes, or other Rust-runtime types appear anywhere in the output
2. All variable widths are resolved — no ambiguous types
3. Every name in an expression is either in `registers`, in a `Wire` declared earlier in the same segment, or is a port name
4. At least one `AwaitTick` appears at the flat top level of `loop_body` for sequential modules
5. `AwaitTick` does not appear nested inside `If` or `Match` arms — tick boundaries are always at the loop-body top level
6. `Emit` appears at least once somewhere in `loop_body` for any sequential module with an output port
7. All `ExprMethodCall` nodes from FIR are normalized to CHIR ops or rejected with diagnostics
8. `while` loops in sequential position are rejected with a rewrite diagnostic
9. `saturating_add`, `checked_add`, and other unsupported arithmetic variants are rejected with diagnostics
