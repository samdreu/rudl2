# Copper Frontend IR (FIR) Design — Phase A

## Purpose

This document defines Phase A of the Copper transpilation pipeline: Frontend Capture. Phase A is the boundary between the Rust proc-macro world and the rest of the compiler. Its sole job is to faithfully capture everything the user wrote, in source order, without interpretation.

**Status: Complete.** The implementation is in `copper-codegen/src/parser.rs` and the schema is in `copper-core/src/frontend_ir.rs`.

---

## What Phase A Is and Is Not

**Phase A is:**
- A pure capture layer — it preserves, not transforms
- Source-shaped: the output mirrors the structure of the Rust function as written
- Span-preserving: every node carries a `SourceSpan` for diagnostics
- Portable: all output types are plain Rust structs with no `syn` or proc-macro dependencies
- The translation boundary between proc-macro world and the rest of the compiler

**Phase A is not:**
- Normalizing: it does not rewrite expressions or resolve types
- Scheduling: it makes no decisions about timing or assignment style
- Width-inferring: it does not compute signal widths
- Rejecting (mostly): it captures unsupported constructs as `Other` variants rather than failing — early rejection is Phase B's job

---

## Why Phase A Exists as a Separate Layer

`syn::ItemFn` and related types are only fully meaningful inside a proc-macro invocation. `syn::Span` is an opaque handle into the Rust compiler's internal span table — it cannot be passed outside proc-macro context, serialized, tested against in unit tests, or used in regular library crates.

`FrontendModuleIR` is the portable version: all spans become `SourceSpan { start_line, start_col, end_line, end_col }`, all expression nodes become Copper-owned types, and the result can flow freely through the rest of the compiler, be unit-tested without a proc-macro harness, and be serialized or printed for debugging.

---

## Entrypoint API

```rust
pub fn capture_frontend_ir(
    design_fn: &ItemFn,
    hardware_fns: &HashSet<String>,
) -> Result<FrontendModuleIR, LowerError>
```

Called from the `#[hardware]` proc-macro in `copper-macros`. Receives a `syn::ItemFn` and a registry of known `#[hardware]`-annotated function names, and produces a `FrontendModuleIR`. The result is handed to Phase B for semantic lowering.

### The `hardware_fns` registry

Phase A needs to distinguish `#[hardware]` function calls (submodule instantiations) from plain Rust utility calls (to be inlined by Phase B). However, the attribute `#[hardware]` is on the *callee's* definition, not at the call site. By the time Phase A is processing the call site, only the function name is visible.

The proc-macro dispatcher in `copper-macros` accumulates a registry of `#[hardware]`-annotated function names as it processes each annotated item. This registry is passed into `capture_frontend_ir` and used to annotate `ExprCall` nodes with `is_hardware_module: bool`.

**Ordering constraint:** The dispatcher must process all `#[hardware]` definitions before processing any function body that calls them. Within a single crate this is guaranteed by the proc-macro expansion order (items are processed in source order). Cross-crate calls are always resolved by the time the calling crate is compiled, so the registry is always complete.

### Helper functions (all implemented)

| Function | Purpose |
|---|---|
| `capture_signature(fn)` | Extracts parameter list and return type |
| `classify_module(fn)` | Detects async (sequential) vs sync (combinational) |
| `capture_clock_metadata(fn)` | Finds `Clock<D>` parameters and extracts domain hint |
| `capture_raw_statements(fn)` | Recursively parses the function body into `RawStmt` list |
| `parse_expr_type(expr, hardware_fns)` | Converts `syn::Expr` → `ExprType` recursively, annotating hardware calls |
| `capture_source_span(node)` | Converts a `syn::Spanned` to `SourceSpan` |

---

## Schema: `FrontendModuleIR`

```rust
pub struct FrontendModuleIR {
    pub module_name: String,
    pub signature: FrontendSignature,
    pub classification: FrontendClassification,
    pub clocks: Vec<ClockParamMeta>,
    pub raw_statements: Vec<RawStmt>,
    pub span: SourceSpan,
}
```

### `FrontendClassification`

```rust
pub enum FrontendClassification {
    CombinationalFn,     // plain fn — no async
    AsyncSequentialFn,   // async fn — has clock + tick loop
}
```

Determined solely by the presence of `async` on the function signature. Phase B refines this.

### `FrontendSignature`

```rust
pub struct FrontendSignature {
    pub params: Vec<RawParam>,
    pub return_ty: Option<RawTypeRef>,
}

pub struct RawParam {
    pub name: String,
    pub ty: RawTypeRef,
    pub raw_text: String,
    pub span: SourceSpan,
}

pub struct RawTypeRef {
    pub ty_text: String,   // e.g. "Arc < Mutex < u8 > >", "Clock < MainClk >", "u8"
    pub span: SourceSpan,
}
```

`ty_text` is the raw token stream rendering of the type — not normalized or resolved. Phase B parses it.

### `ClockParamMeta`

```rust
pub struct ClockParamMeta {
    pub param_idx: usize,
    pub param_name: String,
    pub clock_ty: String,           // "Clock < MainClk >"
    pub domain_hint: Option<String>, // "MainClk"
    pub span: SourceSpan,
}
```

Parameters whose type begins with `Clock<` after whitespace stripping. Phase B uses this to mark the clock port and strip the clock from the data port list.

---

## Schema: Statement Nodes

### `RawStmt`

```rust
pub struct RawStmt {
    pub order: usize,       // 0-indexed position in the enclosing block
    pub kind: RawStmtKind,
    pub text: String,       // raw token stream text — for debugging/diagnostics only
    pub span: SourceSpan,
}
```

`text` is a fallback for diagnostics. Phase B uses `kind`, not `text`.

### `RawStmtKind`

```rust
pub enum RawStmtKind {
    Local(LocalStmt),   // let bindings
    Expr(ExprStmt),     // expression statements
    Item(ItemStmt),     // inline item definitions
}
```

### `LocalStmt`

```rust
pub struct LocalStmt {
    pub is_mut: bool,
    pub ty: Option<RawTypeRef>,     // explicit type annotation if present
    pub name: String,
    pub init: Option<ExprType>,     // initializer expression
    pub attrs: Vec<String>,
    pub span: SourceSpan,
}
```

Type inference chain used by Phase A to populate `ty`:
1. Explicit annotation: `let x: u8 = ...`
2. Function call hint: `let x = SomeType::from_u128(...)`
3. Cast hint: `let x = ... as u8`
4. None — left to Phase B to infer

### `ExprStmt`

```rust
pub struct ExprStmt {
    pub expr: ExprType,
    pub has_semi: bool,
    pub span: SourceSpan,
}
```

### `ItemStmt`

```rust
pub enum ItemStmt {
    Const(ItemConst),
    Enum(ItemEnum),
    Struct(ItemStruct),
    Type(ItemType),
    Macro(ItemMacro),
    Other(ItemOther),   // fallback — preserves text
}
```

Inline item definitions inside function bodies are uncommon in hardware modules but captured for completeness.

---

## Schema: Expression Nodes

`ExprType` is a tagged union with 20 variants, one per supported Rust expression form. Every variant carries a `span` field.

```rust
pub enum ExprType {
    Array(ExprArray),
    Assign(ExprAssign),
    Async(ExprAsync),
    Await(ExprAwait),
    Binary(ExprBinary),
    Call(ExprCall),
    Cast(ExprCast),
    Field(ExprField),
    If(ExprIf),
    Let(ExprLet),
    Lit(ExprLit),
    Loop(ExprLoop),
    Match(ExprMatch),
    MethodCall(ExprMethodCall),
    Range(ExprRange),
    Reference(ExprReference),
    Repeat(ExprRepeat),
    Return(ExprReturn),
    Unary(ExprUnary),
    While(ExprWhile),
    Yield(ExprYield),
}
```

Key variants for hardware modules:

| Variant | Hardware relevance |
|---|---|
| `ExprAwait` | `clk.tick().await` — the clock edge boundary |
| `ExprLoop` | The infinite hardware loop |
| `ExprMatch` | State machine / mux pattern matching |
| `ExprIf` | Conditional logic |
| `ExprAssign` | Register / wire assignments |
| `ExprMethodCall` | `wrapping_add`, `lock`, etc. — normalized in Phase B |
| `ExprCall` | Free function calls |
| `ExprLit` | Integer/bool literals |

**`ExprCall` carries `is_hardware_module`** to distinguish submodule instantiations from plain utility calls:

```rust
pub struct ExprCall {
    pub func: Box<ExprType>,
    pub args: Vec<ExprType>,
    pub is_hardware_module: bool,  // true if callee is in the hardware_fns registry
    pub span: SourceSpan,
}
```

Phase B uses `is_hardware_module` to decide between generating a `CHIRSubmoduleInst` (true) or inlining the call body (false).

**Macro calls** (`emit!(x)`, `assert!`, etc.) currently fall through to `ExprLit` with the raw token text preserved. Phase B pattern-matches on `text` to detect `emit!`. A future improvement would be a dedicated `ExprMacroCall` variant. See the "Recognized Macro Call Patterns" section below for exact text formats.

### Notable expression structs

```rust
pub struct ExprAwait {
    pub base: Box<ExprType>,   // the future being awaited — usually clk.tick()
    pub span: SourceSpan,
}

pub struct ExprLoop {
    pub body: Vec<RawStmt>,
    pub span: SourceSpan,
}

pub struct ExprMatch {
    pub scrutinee: Box<ExprType>,
    pub arms: Vec<ExprMatchArm>,
    pub span: SourceSpan,
}

pub struct ExprMatchArm {
    pub pattern_text: String,     // raw text — Phase B parses this
    pub guard: Option<Box<ExprType>>,
    pub body: Box<ExprType>,
    pub span: SourceSpan,
}

pub struct ExprMethodCall {
    pub receiver: Box<ExprType>,
    pub method: String,
    pub args: Vec<ExprType>,
    pub span: SourceSpan,
}
```

---

## Recognized Hardware Patterns in FIR

Phase B must recognize specific FIR tree shapes to extract hardware semantics. These are the canonical forms Phase A produces for the most critical hardware constructs.

### `emit!(value)` — output drive

`emit!` is a statement-level macro. It arrives as `Stmt::Macro` and is captured as:

```rust
RawStmtKind::Expr(ExprStmt {
    expr: ExprType::Lit(ExprLit {
        text: "emit ! (count)",   // exact spacing from token stream
        ...
    }),
    has_semi: true,
    ...
})
```

Phase B detects `emit!` by checking if `ExprLit::text` (whitespace-stripped) starts with `"emit!("`  or matches the token-stream pattern `emit ! (`. The argument name is extracted from within the parentheses.

**Known fragility:** This relies on `quote!` token stream formatting. The canonical whitespace-stripped form is `"emit!(value)"`. Phase B should strip all whitespace before matching.

### `clk.tick().await` — clock edge boundary

This is a method call chain followed by `.await`. Phase A produces the following nested tree:

```rust
ExprType::Await(ExprAwait {
    base: Box::new(ExprType::Call(ExprCall {
        func: Box::new(ExprType::MethodCall(ExprMethodCall {
            receiver: Box::new(ExprType::Var("clk")),   // clock parameter name
            method: "tick",
            args: [],
            ...
        })),
        args: [],
        is_hardware_module: false,
        ...
    })),
    ...
})
```

More precisely, `clk.tick()` is a method call that returns a future, and `.await` is the await expression on it. Phase B identifies a tick boundary by matching:
- `ExprType::Await` whose `base` is
- an `ExprMethodCall` with `method == "tick"` whose `receiver` is
- a `Var` or `Path` matching a known clock parameter name from `FrontendModuleIR::clocks`

Multiple `clk.tick().await` calls in a single loop body all produce separate `ExprAwait` nodes in source order — Phase B collects all of them as `AwaitTick` boundaries, implementing the multi-tick support from SHIR_DESIGN.md Decision D5.

### `Arc<Mutex<T>>` input parameters

Sequential modules receive mutable inputs wrapped in `Arc<Mutex<T>>`. These appear in FIR as `RawParam` with `ty_text` containing the full wrapper:

```
"Arc < Mutex < u8 > >"
"Arc < Mutex < Bit > >"
```

Phase A preserves these verbatim. Phase B is responsible for stripping the wrapper and resolving `T` to a `CHIRType` data port. The rule is: if `ty_text` whitespace-stripped starts with `Arc<Mutex<`, extract the inner type. Access patterns (`*x.lock().unwrap()`) inside the body are `ExprMethodCall` chains that Phase B also normalizes away — they have no hardware meaning, only simulation meaning.

### `loop { ... }` — the hardware process loop

The top-level infinite loop in a sequential module arrives as:

```rust
RawStmtKind::Expr(ExprStmt {
    expr: ExprType::Loop(ExprLoop {
        body: Vec<RawStmt>,   // contains emit!, tick, assignments in source order
        ...
    }),
    has_semi: false,
    ...
})
```

Phase B expects exactly one `ExprLoop` at the top level of a sequential module's `raw_statements`. All `AwaitTick` boundaries are found within `ExprLoop::body`.

---

## `SourceSpan`

```rust
pub struct SourceSpan {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}
```

Stable, portable, proc-macro-independent. Carries through all IR phases for diagnostics and optional source-location comments in emitted Verilog.

**Implementation status: stub.** `capture_source_span` currently returns `SourceSpan::default()` (all zeros) for all nodes. Conversion from `syn::Span` to line/column requires the `"span-locations"` feature on `proc-macro2`. This must be implemented before Phase B diagnostics include meaningful source locations. Until then, all spans in FIR are `{0, 0, 0, 0}` and Invariant 2 below is not currently enforced.

---

## What Phase A Does Not Capture

These are explicit non-goals:

- **Generics and const generics** (`Bits<N>` has `N` preserved only as text in `ty_text`)
- **Trait method dispatch** — a method call is captured as `ExprMethodCall`; which trait it comes from is not resolved
- **Lifetimes** — not relevant to hardware, ignored
- **`use` statements at module top level** — outside the function body, not captured
- **Attributes on the function itself** (`#[hardware]`, `#[hardware(function_typed)]`) — consumed by the macro dispatcher, not in FIR. The dispatcher uses the attribute to decide whether to call `capture_frontend_ir` at all; the presence of the attribute is implicit in the fact that FIR was produced. Phase B infers hardware role from `async` (sequential) vs non-async (combinational) — it does not need to re-read the attribute.
- **`Arc<Mutex<T>>` wrapper semantics** — Phase A preserves `Arc<Mutex<T>>` verbatim as `ty_text`. It does not strip the wrapper or classify the parameter as a data port. This is Phase B's responsibility during type resolution. The inner type `T` is extracted by Phase B by parsing the `ty_text` string.
- **`*x.lock().unwrap()` access patterns** — the simulation-side access pattern for reading `Arc<Mutex<T>>` inputs appears in FIR as a chain of `ExprMethodCall` nodes (`lock`, `unwrap`, then deref). Phase A captures these faithfully. Phase B recognizes and removes them, replacing the entire chain with a direct reference to the underlying port signal.

---

## Phase A Invariants

These hold for all output produced by `capture_frontend_ir`:

1. `raw_statements` is in source order (`order` field is 0-indexed position)
2. Every `SourceSpan` has `start_line <= end_line` and valid column values — **currently not enforced**: `capture_source_span` is a stub returning all zeros; this invariant becomes active once span locations are implemented
3. `classification` is `AsyncSequentialFn` if and only if `design_fn.sig.asyncness.is_some()`
4. `clocks` contains exactly the parameters whose type text (whitespace-stripped) begins with `Clock<`
5. No `syn` types appear anywhere in the output
6. `ExprCall::is_hardware_module` is true if and only if the callee name appears in the `hardware_fns` registry passed to `capture_frontend_ir`
7. `RawStmt::text` and `ExprLit::text` fields are raw token stream strings for debugging only — Phase B must not use them as the primary source of semantic information, with one exception: `emit!()` detection requires matching on `ExprLit::text` until a dedicated `ExprMacroCall` variant is added (see "Recognized Hardware Patterns")
8. `pattern_text` in `ExprMatchArm` is a structured raw string produced by `to_token_stream()` — Phase B must parse it to classify patterns (tuple, literal, wildcard, enum variant)

---

## Test Coverage

Phase A has 103 unit tests covering:

- Signature capture: simple params, no-return, tuple-return
- Classification: async vs sync detection
- Clock metadata: domain hint extraction, multiple clock params
- Statement ordering and kind classification
- Local statement: explicit type, inferred type, cast, no type
- Item statement: all 6 variants
- Expression types: all 20+ variants
- Nested recursive expressions: method chains, if-else-if, match guards
- Complex multi-level nesting: calls in matches in if conditions
- Block extraction for compound expressions

Tests live in `copper-codegen/src/parser.rs` in the `#[cfg(test)]` module.

---

## Interface with Phase B

Phase B receives `FrontendModuleIR` and its entry contract is:

- It may assume all Phase A invariants hold
- `ExprCall::is_hardware_module == true` → generate `CHIRSubmoduleInst`; `false` → inline or reject
- `emit!()` detection: find `ExprType::Lit` nodes whose `text` whitespace-stripped starts with `"emit!("` — extract the argument name from within the parentheses
- `clk.tick().await` detection: find `ExprType::Await` whose `base` is an `ExprMethodCall` with `method == "tick"` on a receiver matching a clock parameter name; multiple such nodes in a loop body produce multiple `CHIRStmt::AwaitTick` entries (multi-tick support per SHIR_DESIGN.md D5)
- `pattern_text` in `ExprMatchArm` is a raw token-stream string; Phase B must parse it to identify tuple patterns `(A, B)`, literal patterns, wildcards `_`, and enum variant patterns
- `ty_text` in `RawTypeRef` is a raw string; Phase B must parse it to resolve `CHIRType` — including stripping `Arc<Mutex<T>>` wrappers on input parameters
- `*x.lock().unwrap()` method call chains on clock-excluded parameters must be recognized and collapsed to a direct `CHIRExpr::Var` reference to the underlying port
