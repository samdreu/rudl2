# Copper Verilog-Legal IR (VLIR) Design — Phase D

## Purpose

This document defines Phase D of the Copper transpilation pipeline: Verilog Legalization. Phase D receives a `SHIRModule` (Phase C output) and produces a `VLIRModule` — a fully legalized representation that maps 1:1 onto valid SystemVerilog syntax. After Phase D, the only remaining work is mechanical text serialization (Phase E).

VLIR is the "can be written as text" layer. Everything that is semantically correct but not yet syntactically valid for a target toolchain is fixed here.

---

## What Phase D Is and Is Not

**Phase D is:**
- A mechanical transformation — it does not change semantics
- Toolchain-profile-aware (generic, Verilator, Yosys have different restrictions)
- Name-legalizing (keyword conflicts, reserved names, Verilog identifier rules)
- Structure-flattening where needed (tuple match → concatenated case selector)
- Width-explicit (all expressions carry resolved bit widths for literal formatting)
- The last place where structural changes to the IR are made

**Phase D is not:**
- Semantic: it must not change the timing, scheduling, or hardware intent from SHIR
- Optimizing: no inlining, constant folding, or dead code removal
- Re-scheduling: assignment intent was fixed in Phase C and must not be re-interpreted

---

## Toolchain Profiles

Phase D behavior is driven by a `TranspilationProfile`:

```rust
pub enum ToolchainProfile {
    Generic,      // Standard SystemVerilog, no tool-specific workarounds
    Verilator,    // Verilator-compatible SV (2-value, some feature restrictions)
    Yosys,        // Yosys-compatible SV (synthesis-oriented restrictions)
}
```

Restrictions by profile:

| Feature | Generic | Verilator | Yosys |
|---|---|---|---|
| `always_ff` / `always_comb` | ✓ | ✓ | ✓ |
| `logic` type | ✓ | ✓ | ✓ |
| `initial` blocks | No (policy) | No (policy) | No (policy) |
| `x`/`z` literals | ✓ | ✗ (2-value sim) | ✓ |
| Implicit net declarations | ✓ | ✗ | ✗ |
| Packed structs | ✓ | limited | ✗ |

The profile is passed through the pipeline and consulted in Phase D only.

---

## Core Data Model

VLIR is a thin legalization layer over SHIR. Most types are direct mappings with legalized names and concrete Verilog constructs.

```rust
pub struct VLIRModule {
    pub name: String,                   // legalized module name
    pub ports: Vec<VLIRPort>,
    pub body: VLIRBody,
}

pub struct VLIRPort {
    pub name: String,                   // legalized port name
    pub direction: VLIRPortDir,
    pub kind: VLIRPortKind,
    pub width: usize,                   // always resolved here
}

pub enum VLIRPortDir {
    Input,
    Output,
}

pub enum VLIRPortKind {
    Clock,
    Logic,
}

pub enum VLIRBody {
    Combinational(VLIRCombBody),
    Sequential(VLIRSeqBody),
}
```

### Combinational Body

```rust
pub struct VLIRCombBody {
    // #[hardware] submodule instantiations with legalized names
    pub submodules: Vec<VLIRSubmoduleInst>,
    // always_comb block contents, in order
    pub comb_stmts: Vec<VLIRStmt>,
    // assign out = <expr>;
    pub output_assign: VLIRContinuousAssign,
}
```

### Sequential Body

```rust
pub struct VLIRSeqBody {
    pub clock: String,                  // legalized clock port name

    // reg/logic declarations
    pub reg_decls: Vec<VLIRRegDecl>,

    // #[hardware] submodule instantiations with legalized names
    // Emitted as module instantiation statements before always blocks
    pub submodules: Vec<VLIRSubmoduleInst>,

    // per-phase always_comb block (pre_edge wires)
    // For single-phase modules: one entry, no phase guard needed
    // For multi-phase modules: each entry is guarded by phase_r == K
    pub comb_phases: Vec<VLIRCombPhase>,

    // single always_ff block — all register updates
    pub always_ff: VLIRAlwaysFF,

    // continuous assign for output
    pub output_assign: VLIRContinuousAssign,
}

pub struct VLIRRegDecl {
    pub name: String,       // legalized
    pub width: usize,
}

// A submodule instantiation with all names legalized for the target profile
pub struct VLIRSubmoduleInst {
    pub inst_name: String,              // legalized instance name, e.g. "full_adder_0"
    pub module_name: String,            // legalized module name, e.g. "full_adder"
    pub inputs: Vec<(String, VLIRExpr)>, // (legalized port name, driving expr)
    pub output_wire: String,            // legalized wire name for this instance's output
    pub output_width: usize,
}

pub struct VLIRCombPhase {
    pub phase_guard: Option<VLIRExpr>,  // None for single-phase; Some(phase_r == K) for multi-phase
    pub stmts: Vec<VLIRStmt>,
}

pub struct VLIRAlwaysFF {
    pub clock: String,
    pub stmts: Vec<VLIRFFStmt>,
}

pub struct VLIRContinuousAssign {
    pub target: String,
    pub value: VLIRExpr,
}
```

---

## Statement Models

VLIR has two separate statement types: one for `always_comb` (blocking / combinational) and one for `always_ff` (non-blocking / sequential). They are separate types to make it impossible to accidentally mix them.

```rust
// Statements that appear in always_comb blocks
pub enum VLIRStmt {
    // wire/logic declaration with immediate drive
    WireAssign {
        name: String,
        width: usize,
        value: VLIRExpr,
    },
    If {
        condition: VLIRExpr,
        then_stmts: Vec<VLIRStmt>,
        else_stmts: Option<Vec<VLIRStmt>>,
    },
    // case statement — used for match and tuple match
    Case {
        selector: VLIRExpr,        // may be a concat for tuple patterns
        arms: Vec<VLIRCaseArm>,
        default: Option<Vec<VLIRStmt>>,
    },
}

// Statements that appear in always_ff blocks (non-blocking assignments only)
pub enum VLIRFFStmt {
    // target <= value
    NonBlockingAssign {
        target: String,
        value: VLIRExpr,
    },
    If {
        condition: VLIRExpr,
        then_stmts: Vec<VLIRFFStmt>,
        else_stmts: Option<Vec<VLIRFFStmt>>,
    },
    Case {
        selector: VLIRExpr,
        arms: Vec<VLIRFFCaseArm>,
        default: Option<Vec<VLIRFFStmt>>,
    },
}

pub struct VLIRCaseArm {
    pub selector_value: VLIRExpr,  // concrete literal or concat
    pub stmts: Vec<VLIRStmt>,
}

pub struct VLIRFFCaseArm {
    pub selector_value: VLIRExpr,
    pub stmts: Vec<VLIRFFStmt>,
}
```

**Why two statement types?** Having `VLIRStmt` and `VLIRFFStmt` as separate types means Phase E cannot accidentally emit a blocking assign inside `always_ff` or a non-blocking assign inside `always_comb`. The type system enforces the assignment policy from `VERILOG_OUTPUT_STANDARDS.md` §4.

---

## Expression Model

```rust
pub enum VLIRExpr {
    // Named signal reference — always legalized
    Var(String),

    // Width-explicit literal: e.g. 8'd5, 1'b0, 2'd3
    Lit { width: usize, value: u128 },

    // Binary operation
    BinOp {
        left: Box<VLIRExpr>,
        op: VLIRBinOp,
        right: Box<VLIRExpr>,
    },

    // Unary operation
    UnOp {
        op: VLIRUnOp,
        expr: Box<VLIRExpr>,
    },

    // Ternary: cond ? a : b
    Ternary {
        cond: Box<VLIRExpr>,
        then_val: Box<VLIRExpr>,
        else_val: Box<VLIRExpr>,
    },

    // Bit concatenation: {a, b, c}
    Concat(Vec<VLIRExpr>),

    // Bit slice: expr[high:low]
    Slice {
        expr: Box<VLIRExpr>,
        high: usize,
        low: usize,
    },
}

pub enum VLIRBinOp {
    Add, Sub, Mul,
    BitAnd, BitOr, BitXor,
    Shl, Shr,
    Eq, Neq, Lt, Lte, Gt, Gte,
    LogicalAnd, LogicalOr,
}

pub enum VLIRUnOp {
    BitNot, LogicalNot, Neg,
    ReductionAnd, ReductionOr, ReductionXor,
}
```

Note: `SHIRExpr::Mux` becomes `VLIRExpr::Ternary`. `SHIRExpr::Case` becomes a `VLIRStmt::Case` or `VLIRFFStmt::Case`. There is no `Case` expression in VLIR — cases are always statements.

---

## Legalization Passes

Phase D applies these passes in order:

### Pass 1: Name Legalization

Every name in the IR (module name, port names, register names, wire names, submodule instance names, submodule output wire names) is run through the legalizer:

```
legalize(name) → legal_name
```

Rules:
1. **Reserved keyword substitution:** If `name` is a Verilog/SystemVerilog reserved keyword, append `_sig`. This is the full keyword list from `VERILOG_OUTPUT_STANDARDS.md` plus SV additions (`logic`, `interface`, `always_ff`, `always_comb`, etc.)
2. **Leading digit:** If `name` starts with a digit, prepend `sig_`
3. **Invalid characters:** Replace any character not in `[a-zA-Z0-9_]` with `_`
4. **Duplicate names after legalization:** If two names collide after legalization, append `_0`, `_1`, etc. to disambiguate — the first occurrence keeps the base name

A legalization map `HashMap<original, legalized>` is built and applied consistently across all expressions, so all references to a signal remain consistent after renaming.

Reserved keywords to check (includes SV keywords not in plain Verilog):

```
always, always_comb, always_ff, always_latch, and, assign, automatic,
begin, bit, buf, byte, case, casex, casez, cell, clocking, config,
default, defparam, disable, do, edge, else, end, endcase, endconfig,
endfunction, endgenerate, endgroup, endinterface, endmodule, endpackage,
endprimitive, endprogram, endproperty, endspecify, endsequence,
endtable, endtask, enum, export, extends, extern, final, first_match,
for, force, foreach, forever, fork, fork_join, forkjoin, function,
generate, genvar, highz0, highz1, if, iff, ifnone, ignore_bins,
illegal_bins, import, incdir, include, initial, inout, input, inside,
instance, int, integer, interface, intersect, join, join_any, join_none,
large, liblist, library, local, localparam, logic, longint, macromodule,
matches, medium, modport, module, nand, negedge, new, nmos, nor,
noshowcancelled, not, notif0, notif1, null, or, output, package, packed,
parameter, pmos, posedge, primitive, priority, program, property,
protected, pull0, pull1, pulldown, pullup, pulsestyle_onevent,
pulsestyle_ondetect, pure, rand, randc, randcase, randsequence, rcmos,
real, realtime, ref, reg, release, repeat, return, rnmos, rpmos, rtran,
rtranif0, rtranif1, scalared, sequence, shortint, shortreal,
showcancelled, signed, small, solve, specify, specparam, static,
string, strong0, strong1, struct, super, supply0, supply1, table, task,
this, throughout, time, timeprecision, timeunit, tran, tranif0, tranif1,
tri, tri0, tri1, triand, trior, trireg, type, typedef, union, unique,
unique0, unsigned, use, uwire, var, vectored, virtual, void, wait,
wait_order, wand, weak0, weak1, while, wildcard, wire, with, within, wor,
xnor, xor
```

### Pass 2: Tuple Pattern Lowering

`SHIRPattern::Tuple` cannot be directly emitted in Verilog — there is no native tuple match. Phase D converts tuple match into a `case` on a concatenated selector:

**Input (SHIR):**
```
Match {
    scrutinee: (Var("j"), Var("k")),
    arms: [
        Tuple([Lit(0), Lit(0)]) => stmts_00,
        Tuple([Lit(0), Lit(1)]) => stmts_01,
        Tuple([Lit(1), Lit(0)]) => stmts_10,
        Tuple([Lit(1), Lit(1)]) => stmts_11,
        Wildcard => stmts_default,
    ]
}
```

**Output (VLIR):**
```
Case {
    selector: Concat([Var("j"), Var("k")]),   // {j, k}
    arms: [
        selector_value: Lit { width: 2, value: 0b00 } → stmts_00,
        selector_value: Lit { width: 2, value: 0b01 } → stmts_01,
        selector_value: Lit { width: 2, value: 0b10 } → stmts_10,
        selector_value: Lit { width: 2, value: 0b11 } → stmts_11,
    ],
    default: stmts_default,
}
```

The concatenated literal values are computed by Phase D: for a tuple `(p0, p1, ..., pN)` where each `pi` is a literal of width `wi`, the concatenated value is `p0 << (w1 + w2 + ... + wN) | p1 << (w2 + ... + wN) | ... | pN`. The total selector width is `w0 + w1 + ... + wN`.

### Pass 3: Mux-to-Ternary Lowering

`SHIRExpr::Mux` → `VLIRExpr::Ternary`. This is structural — no logic changes.

### Pass 4: Case-Expression Lifting

`SHIRExpr::Case` (case used as an expression, e.g. in a `SHIRRegUpdate::next_value`) is lifted into a `VLIRFFStmt::Case` block in the always_ff body. Each arm becomes a non-blocking assign to the target register.

**Input (SHIR):**
```
SHIRRegUpdate {
    target: "q",
    next_value: Case {
        scrutinee: Concat([j, k]),
        arms: [ (0,0) → q, (0,1) → 0, (1,0) → 1, (1,1) → Mux(q==0, 1, 0) ],
        default: 0
    }
}
```

**Output (VLIR always_ff):**
```
VLIRFFStmt::Case {
    selector: Concat([j, k]),
    arms: [
        2'b00: q <= q,
        2'b01: q <= 0,
        2'b10: q <= 1,
        2'b11: q <= (q == 0) ? 1 : 0,
    ],
    default: [ q <= 0 ]
}
```

### Pass 5: Literal Width Annotation

Every `SHIRLit { ty, value }` becomes `VLIRExpr::Lit { width, value }` with a concrete bit width.

- `CHIRType::UInt { width: N }` → `width = N`
- `CHIRType::SInt { width: N }` → `width = N`
- `CHIRType::Bool` → `width = 1`

This ensures Phase E can emit `N'd<value>` (decimal) or `N'b<bits>` (binary) without needing type information.

### Pass 6: Multi-Phase Guard Injection

For multi-phase modules, each `SHIRPhase` with `phase_idx = K` generates a `VLIRCombPhase` with:
```
phase_guard: Some(BinOp(Var("phase_r"), Eq, Lit { width: phase_width, value: K }))
```

The always_comb block becomes:
```verilog
always_comb begin
    if (phase_r == 2'd0) begin
        // segment 0 combinational logic
    end
    if (phase_r == 2'd1) begin
        // segment 1 combinational logic
    end
end
```

For single-phase modules, `phase_guard` is `None` and no guard is emitted.

---

## VLIR Invariants

After all Phase D passes, the following must hold:

1. All names are legal Verilog identifiers with no keyword conflicts — this includes submodule instance names and output wire names
2. No `SHIRPattern::Tuple` appears in the output — all tuples are lowered to `Case` on `Concat`
3. No `SHIRExpr::Mux` appears — all muxes are `VLIRExpr::Ternary`
4. No `SHIRExpr::Case` (used as expression) appears — all cases are lifted to statements
5. Every `VLIRExpr::Lit` has a concrete `width` field
6. `VLIRStmt` nodes appear only in `always_comb` contexts
7. `VLIRFFStmt` nodes appear only in `always_ff` contexts — these types are structurally separate
8. A register name legalized in `reg_decls` is consistently legalized in all `VLIRFFStmt` and `VLIRExpr::Var` references
9. A submodule output wire legalized in `VLIRSubmoduleInst::output_wire` is consistently legalized in all `VLIRExpr::Var` references to it
10. Module name is a legal Verilog identifier

---

## VLIR Does Not Change

The following are explicitly preserved unchanged from SHIR into VLIR:

- Assignment intent: non-blocking stays non-blocking; continuous stays continuous
- Timing regions: pre-edge combinational logic and post-edge register updates remain separate
- Output drive: `assign out = ...` remains a continuous assignment, never moved into always_ff
- Register set: no registers are added or removed (except the already-added `phase_r` from Phase C)
- Structural equivalence with the Copper simulation trace

---

## Phase D Contract

**Input:** `SHIRModule` (from Phase C timing and state construction)

**Output:** `VLIRModule` containing:
- All names legalized for the target toolchain profile
- Tuple patterns lowered to case/concat form
- Case expressions lifted to case statements
- Literals width-annotated
- Structurally typed always_comb / always_ff statement separation
- Multi-phase guards injected where needed

**Semantic invariant:** For every Copper simulation trace vector, the VLIR-derived Verilog must produce the same outputs as the SHIR-derived Verilog. Phase D is a mechanical transformation — any semantic change is a bug.
