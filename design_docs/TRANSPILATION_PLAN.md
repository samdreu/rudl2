# Copper Transpilation Plan (First Draft)

## Purpose

Define a practical, incremental path for transpiling Copper function-typed modules into synthesizable Verilog while preserving Copper simulation semantics.

This draft is intentionally opinionated so it can be reviewed and changed quickly.

## Current State (Observed in This Repository)

- The codegen crate lives in `../copper-codegen/src/` — key files: `lib.rs`, `parser.rs`, `chir_lower.rs`, `shir_lower.rs`, `verilog.rs` (legacy emitter, not part of the new pipeline).
- Phases A and B are complete; Phase C (SHIR) is nearly complete; Phases D–F are not yet started.
- Runtime semantics for cycle boundaries and emit behavior are documented in [ASYNC_AWAIT_SEMANTICS.md](ASYNC_AWAIT_SEMANTICS.md).

## Research Summary: Prior Transpilation Codebases

## 1) CIRCT (MLIR-based hardware compiler)

- Source: https://circt.llvm.org/docs/VerilogGeneration/
- Key pattern: separate correctness from output quality.
- Export architecture has layered phases:
  - optional prettification
  - mandatory legalize/prepare-for-emission
  - final Verilog printer
- Tool-specific compatibility is handled by configurable lowering options (for Verilator, Yosys, Vivado, etc).

Takeaway for Copper:
- Keep a mandatory legalization phase before final emission.
- Treat formatting/prettiness as a separate pass.
- Make backend compatibility switches explicit and testable.

## 2) Chisel + FIRRTL/CIRCT pipeline

- Sources:
  - https://github.com/chipsalliance/chisel
  - https://github.com/chipsalliance/firrtl
- Key pattern: front-end elaboration and DSL semantics are separated from low-level transformations via an IR pipeline.
- Chisel README explicitly points to FIRRTL/CIRCT as compiler framework.
- Historical FIRRTL compiler is archived; current flow points toward CIRCT.

Takeaway for Copper:
- Preserve a stable Copper-specific IR boundary between parsing and emission.
- Avoid direct Rust-AST-to-string shortcuts for anything beyond tiny demos.

## 3) Amaranth (Python HDL)

- Sources:
  - https://github.com/amaranth-lang/amaranth
  - https://amaranth-lang.org/docs/amaranth/latest/start.html
- Key pattern: synchronous/combinational domains are explicit in source semantics; Verilog conversion is backend-driven and preserves source mapping information.
- Practical examples show integrated simulation plus conversion flow.

Takeaway for Copper:
- Keep domain/timing semantics explicit in IR.
- Maintain source locations in IR for diagnostics and generated-code traceability.

## 4) nMigen (historical predecessor to Amaranth)

- Source: https://github.com/m-labs/nmigen
- Key pattern: Python builds an HDL IR, not HLS from arbitrary software.
- Targets behavioral Verilog accepted by downstream toolchains.

Takeaway for Copper:
- Be explicit that Copper transpilation is RTL lowering, not generic software-to-hardware synthesis.

## 5) Yosys internal flow

- Source: https://yosyshq.readthedocs.io/projects/yosys/en/latest/yosys_internals/flow/overview.html
- Key pattern: many frontends, one core IR (RTLIL), many passes, many backends.

Takeaway for Copper:
- Keep one canonical internal representation for transformations and checks.
- Build reusable passes instead of embedding behavior in emitter code.

## 6) Calyx (accelerator compiler)

- Source: https://calyxir.org/
- Key pattern: explicit control representation and scheduling as first-class compilation concerns.

Takeaway for Copper:
- For async state machines, treat control extraction and scheduling as explicit lowering steps, not ad hoc code generation.

## 7) PyMTL3 pass architecture

- Source: https://pymtl3.readthedocs.io/en/latest/ref/passes.html
- Key pattern: modular pass system with metadata-driven options.

Takeaway for Copper:
- Configure transpilation behavior through pass options/config objects rather than scattered flags.

## 8) SpinalHDL

- Source: https://github.com/SpinalHDL/SpinalHDL
- Key pattern: Scala-based RTL construction that emits Verilog/VHDL and emphasizes direct hardware intent rather than event-driven simulation style.

Takeaway for Copper:
- Maintain a strict mapping from language constructs to hardware intent; avoid surprising implicit behavior in emission.

## Copper Transpilation Architecture (Proposed)

## Phase A: Frontend Capture ✅ COMPLETE

Input:
- Rust function AST for modules marked with Copper attributes.

Output:
- Frontend IR (FIR) containing:
  - module signature and typed ports
  - clock/reset metadata
  - structured statements/expressions with full expression AST
  - source spans

Rules:
- Distinguish combinational functions from sequential async modules up front.
- Reject unsupported Rust constructs early with actionable diagnostics.
- Preserve source shape without normalization.

### Completed Implementation

**Entrypoint API:**
```rust
pub fn capture_frontend_ir(design_fn: &ItemFn) -> Result<FrontendModuleIR, LowerError>
```

**Helper Functions (all implemented):**
- `capture_signature(design_fn: &ItemFn) -> FrontendSignature`
- `classify_module(design_fn: &ItemFn) -> FrontendClassification`
- `capture_clock_metadata(design_fn: &ItemFn) -> Vec<ClockParamMeta>`
- `capture_raw_statements(design_fn: &ItemFn) -> Vec<RawStmt>`
- `parse_expr_type(expr: &Expr) -> ExprType` (full recursive expression parsing)
- `capture_source_span(node: &impl Spanned) -> SourceSpan`

**IR Schema (copper-core/src/frontend_ir.rs):**

1. **FrontendModuleIR**: Top-level module IR with signature, classification, clocks, statements
2. **FrontendSignature**: Parameter list in declared order, return type, raw text preservation
3. **RawStmtKind** (Tagged Union):
   - `Local(LocalStmt)` - let statements with type inference
   - `Expr(ExprStmt)` - expressions with semicolon tracking
   - `Item(ItemStmt)` - structured item definitions
4. **LocalStmt**: Mutability, optional type, name, optional init expression, attributes
5. **ItemStmt** (Tagged Union with 6 variants):
   - `Const(ItemConst)` - name, type, value_text, attributes
   - `Enum(ItemEnum)` - name, variants with discriminants, attributes
   - `Struct(ItemStruct)` - name, fields with types/names, attributes
   - `Type(ItemType)` - name, target_ty, attributes
   - `Macro(ItemMacro)` - name, body_text, attributes
   - `Other(ItemOther)` - fallback for unknown items
6. **ExprType** (Tagged Union with 20 expression variants):
   - Literals, arrays, assignments, async/await
   - Binary/unary operations, casts, field access
   - Control flow: if/else, loop, while, match
   - Method calls, function calls, references
   - Return, yield expressions

**Parser Features:**
- Full recursive expression AST parsing for all 20 expression types
- Type extraction with fallback chain (explicit annotation → function call → cast → none)
- Mutability detection for both simple and typed patterns
- Enum variant and discriminant capture
- Struct field handling (named, tuple, unit)
- Type alias and macro body preservation
- Attribute capture for all item types
- Synthetic field names for tuple structs (field_0, field_1, etc.)

**Test Coverage:**
- 103 unit tests, all passing
- Signature capture (simple, no-return, tuple-return)
- Module classification (async vs sync)
- Clock metadata extraction with domain hints
- Raw statement ordering and classification
- Local statement type inference (explicit, inferred, cast, none)
- Item statement variant classification
- Expression type validation for all 20+ variants
- Nested/recursive expression validation (method chains, if-else-if, match guards, etc.)
- Complex nested multi-level expressions (calls in matches in if conditions)
- Statement block extraction for compound expressions (async, if, loop, while)
- Content validation: operands, predecessors, condition types, guard expressions

**Philosophy:**
- Preserves source syntax without normalization
- Captures full structural information for Phase B
- No scheduling or assignment-style decisions yet
- No width inference or type resolution
- Ready for downstream Phase B semantic lowering

## Phase B: Semantic Lowering ✅ COMPLETE

FIR to Canonical Hardware IR (CHIR) — fully implemented in `copper-codegen/src/chir_lower.rs`.

### Completed Implementation

**Entrypoint API:**
```rust
pub fn lower_to_chir(
    fir: &FrontendModuleIR,
    hardware_fns: &HashSet<String>,
    registry: &ModuleRegistry,
) -> Result<CHIRModule, CHIRLowerError>

pub type ModuleRegistry = HashMap<String, FrontendModuleIR>;
```

**Type Resolution:**
- All primitives: `u8`..`u128`, `i8`..`i128`, `bool`, `Logic`, `Bits<N>`
- `Arc<Mutex<T>>` → strips wrapper, resolves inner type
- Type inference from init expressions: typed literals (`0u8`), cast expressions (`x as u8`), booleans
- Hard error (`AmbiguousWidth`) for unresolvable types — no silent coercions

**Port Extraction:**
- `Clock<Domain>` params → `CHIRPortKind::Clock { domain }`
- Data params → `CHIRPortKind::Data { ty }` (with Arc<Mutex<T>> stripping)
- Return type → implicit `out` output port

**Combinational Body Lowering:**
- `let` declarations → `CHIRWireDecl` (type inferred if no annotation)
- Final expression-without-semicolon → `CHIRCombBody::output`
- Hardware calls → `CHIRSubmoduleInst` (named ports from registry)

**Sequential Body Lowering:**
- Pre-loop `let mut` declarations → `CHIRRegDecl` with type + optional init literal
- Top-level `loop { }` → `CHIRSeqBody::loop_body`
- `while` loops → hard error with rewrite suggestion
- `clk.tick().await` → `CHIRStmt::AwaitTick`
- `emit!(value)` → `CHIRStmt::Emit` (error if no output port)
- `x = expr` → `CHIRStmt::Assign`
- `if`/`else` → `CHIRStmt::If` (ticks inside branches rejected)
- `match` → `CHIRStmt::Match` with or-pattern expansion

**Expression Lowering:**
- All binary/unary operators
- `wrapping_add/sub/mul` → `BinOp { wrapping: true }`
- `saturating_*`, `checked_*` → hard error with rewrite
- `lock()`, `unwrap()` → stripped (simulation artifact)
- `if`-as-expression → `CHIRExpr::Mux`
- `match`-as-expression → `CHIRExpr::Case`
- Hardware calls → `CHIRExpr::Var(output_wire)` + submodule registered in ctx
- Cast expressions → strip cast, lower inner expr

**Hardware Call Lowering (with ModuleRegistry):**
- Looks up callee in registry for named port connections (skips Clock params)
- Resolves output type from callee's return type
- Falls back to positional names (`arg0`, `arg1`) if callee not in registry
- Generates unique instance names (`full_adder_0`) and output wires (`full_adder_0_out`)

**Pattern Parsing:**
- `_` → `Wildcard`
- Integer literals and booleans → `Lit`
- Tuple patterns → `Tuple(Vec<CHIRPattern>)`
- Uppercase identifiers → `EnumVariant`
- Or-patterns (`1 | 2 | 3`) → fully expanded to all alternatives via `parse_or_patterns`

**Post-Lowering Validation:**
- Scope check: all `CHIRExpr::Var` references validated against declared port/register/wire/submodule-output names
- `emit!()` used without output port → `EmitWithoutOutput` error

**Test Coverage:** 88 Phase B tests (208 total with Phase A)
- Type resolution, inference, register init, port extraction
- Pattern parsing, or-pattern expansion
- Expression lowering, method call normalization
- Sequential/combinational body structure
- Scope validation, emit validation, while-loop rejection
- Hardware call with registry (port names, output type, fallback)
- 4 end-to-end tests: counter, combinational adder, hardware call, sequential with conditional

## Phase C: Timing and State Construction — IN PROGRESS

CHIR to Scheduled IR (SHIR) — implemented in `copper-codegen/src/shir_lower.rs`.

### Implemented
- `lower_to_shir(chir) -> Result<SHIRModule, SHIRLowerError>` entry point
- Segment splitting at `AwaitTick` boundaries; phase mapping (`seg_k → phase_k`, trailing `→ phase_{N-1}`)
- Single-tick and multi-tick phase building; `phase_r` auto-register for multi-tick
- Pre-edge wire lowering (`SHIRStmt::Wire/If/Match`); register update extraction
- Conditional flattening: `If → Mux`, `Match → Case` on `next_value` (one-sided if holds current value)
- **Sequential register forwarding**: assigns within a segment propagate new values to later assigns,
  matching Rust sequential execution semantics and avoiding Verilog non-blocking old-value pitfall
- **Phase-based wire promotion**: wires referenced in a later hardware phase are promoted to `_r` registers;
  expressions in later phases rewritten to `Var("x_r")` — phase-based (not segment-based) to avoid phantom latency
- Output drive model: `PreEdge`, `PostEdge`, `PhaseConditional`
- Emit validation: rejects `emit!()` inside conditional branch
- 229 tests passing

### Remaining
- Output port / no-output validation (`EmitWithoutOutput`, `OutputWithoutEmit`)
- Submodule output wire visibility enforcement across phases

## Worked Example: Difference Between Phases A, B, and C

Given this module:

```rust
#[hardware(function_typed)]
async fn counter(clk: Clock<MainClk>, in_step: u8) -> u8 {
    let mut count = 0u8;
    loop {
        emit!(count);
        clk.tick().await;
        count = count.wrapping_add(in_step);
    }
}
```

### Phase A (Frontend Capture)

What is preserved:
- The original function shape: async fn, params, return type.
- Source-level statements in source order:
  1. `let mut count = 0u8`
  2. `emit!(count)`
  3. `clk.tick().await`
  4. `count = count.wrapping_add(in_step)`
- Source spans for diagnostics.

What is not decided yet:
- Exact scheduling phase for each statement.
- Final Verilog assignment style.
- Final normalized expression/operator set.

### Phase B (Semantic Lowering)

Typical normalized CHIR shape (illustrative):

```text
module counter
  inputs: clk, in_step:u8
  output: out:u8
  state: count:u8 init 0

  loop_body:
    op emit(value = count)
    op await_tick(clk)
    op assign(count, add_wrap_u8(count, in_step))
```

What changed from A:
- `wrapping_add` is normalized into a canonical add-wrap op.
- `count` is identified as persistent state.
- `emit` and `await_tick` become explicit semantic ops.

What is still not decided:
- Pre-edge vs post-edge bucket placement for each op.
- Whether a concrete Verilog assignment is `<=` or `=`.

### Phase C (Timing and State Construction)

Typical scheduled SHIR shape (illustrative):

```text
clock_domain: clk

pre_edge:
  out_next <- count

edge_event:
  // await_tick boundary

post_edge:
  count_next <- add_wrap_u8(count, in_step)
```

What changed from B:
- Operations are assigned to explicit timing regions.
- Update sets are explicit (`out_next`, `count_next`).
- Edge semantics are fixed and now comparable against simulator behavior.

Resulting intuition:
- Phase A captures source intent.
- Phase B captures hardware meaning.
- Phase C captures cycle timing.

## Phase D: Verilog Legalization

SHIR to Verilog-Legal IR (VLIR):
- Eliminate unsupported constructs for target mode.
- Normalize names and resolve keyword collisions.
- Flatten or preserve hierarchy per configuration.
- Insert explicit wires/temporaries as needed for backend compatibility.

## Phase E: Emission

VLIR to Verilog text:
- Deterministic stable output ordering.
- Source-location comments (optional mode).
- Readability pass (optional) separate from correctness.

## Phase F: Validation

Required checks:
- Parse/lint generated Verilog.
- Simulate generated Verilog test vectors and compare against Copper simulation traces.
- For async modules, include edge-sensitive ordering tests.

## Minimal Feature Set for Milestone 1

Support first:
- Single clock domain.
- Flat modules (no deep hierarchy transforms yet).
- Scalar and fixed-width integer-like types already used in examples.
- if/else, match subsets with finite constant patterns.
- loop with explicit tick-await structure.
- emit and register update patterns used by current examples.

Defer initially:
- Complex pattern matching forms.
- Advanced memories and inferred RAM forms.
- Multi-clock lowering.
- Aggressive optimization passes.

## Diagnostics Strategy

For every unsupported construct:
- Include file span.
- Include short reason.
- Include nearest supported rewrite pattern.

Example style:
- Unsupported: method call in hardware expression.
- Suggested rewrite: precompute into a let-bound value using supported ops.

## Verification Strategy

Three levels:
- Unit tests at IR pass boundaries.
- Golden-file tests for emitted Verilog.
- Behavioral equivalence tests against Copper runtime traces and Verilator.

Initial regression set should include:
- counter
- pipeline
- uart_fsm
- fifo

## Configuration Model

Adopt explicit transpilation options object, for example:
- target_tool_profile: generic, verilator, yosys
- naming_style: compact, readable
- include_source_locations: true/false
- flatten_hierarchy: true/false

## Decision Log (Accepted)

1. Target language and emission baseline
- Decision: SystemVerilog-first transpilation.
- Rationale: Cleaner and safer sequential/combinational constructs (`always_ff`, `always_comb`) and better readability for early development.
- Note: Output must follow [VERILOG_OUTPUT_STANDARDS.md](VERILOG_OUTPUT_STANDARDS.md).

2. Semantic source of truth
- Decision: Copper simulator behavior is canonical.
- Rule: Any mismatch between Copper simulation traces and emitted SystemVerilog behavior is treated as a transpiler bug until proven otherwise.

3. Async timing representation
- Decision: Keep explicit pre-edge and post-edge timing regions in IR.
- Impact: Timing intent is explicit before emission and directly testable against simulator traces.

4. Assignment policy (normative)
- Decision: Assignment intent is fixed in Phase C (SHIR), not inferred in the text emitter.
- Rules:
  - Sequential architectural updates use non-blocking assignment intent.
  - Combinational logic uses blocking assignment intent (or continuous assignment where appropriate).
  - The same architectural signal must not be driven by both combinational and sequential contexts.
  - Do not mix blocking and non-blocking assignment intent for the same architectural signal.
  - Emission phase is mechanical: it must preserve SHIR intent and must not reinterpret scheduling.

5. Initialization and reset policy
- Decision: Do not emit `initial` blocks in production transpilation output.
- Decision: Reset-based initialization of state is allowed and preferred for deterministic startup.
- Testing exception: simulation/testing workflows may use test-only initialization paths.

6. Width and signedness policy
- Decision: Strict inferred widths/signs.
- Rule: Ambiguous width/sign inference is a hard compile error (no silent coercions).

7. Unsupported Rust constructs policy
- Decision: Fail fast with actionable diagnostics.
- Rules:
  - Unsupported constructs are compile errors, not silent degradation.
  - Diagnostics include source span, reason, and suggested rewrite pattern.
  - Unsupported feature classes are tracked explicitly (supported / recognized-but-not-supported / forbidden-in-hardware-subset).

8. Hierarchy strategy
- Decision: Support all three modes via configuration:
  - preserve hierarchy
  - full flatten
  - hybrid/selective flatten
- Default direction: hybrid, with preserve-first behavior unless tool/profile constraints require flattening.

9. Memory strategy
- Decision: Keep all three strategy modes available by configuration and roadmap stage:
  - no-memory lowering mode
  - minimal memory subset mode
  - explicit memory IR lowering mode
- Milestone guidance: start with no/minimal subset, then expand to explicit memory IR.

10. Determinism and reproducibility
- Decision: Transpilation output should be deterministic for identical inputs and config.
- Rules:
  - Stable ordering of modules/ports/declarations.
  - Stable temporary naming.
  - No time/host-specific nondeterministic text in emitted RTL.

11. Extensibility boundary
- Decision: Build a private internal pass manager first.
- Rationale: modular compiler structure without committing to a public plugin ABI too early.
- Future path: expose public extension points only after IR/pass contracts stabilize.

## Proposed Near-Term Delivery Plan

1. Freeze FIR and CHIR schema.
2. Implement unsupported-construct diagnostics in frontend lowering.
3. Implement async-to-scheduled lowering for current examples.
4. Implement legalization pass for verilog-safe subset.
5. Add deterministic emitter and golden tests.
6. Add equivalence harness against Copper simulation traces.

## Remaining Decisions For Review

1. Tool profile defaults for SystemVerilog emission
- Choose exact default profile behavior for `generic`, `verilator`, and `yosys` targets.

2. Milestone 1 memory subset scope
- Confirm whether Milestone 1 includes no memory lowering or a small explicitly listed memory subset.

3. Optimization phase timing
- Confirm whether to defer optimization passes entirely to downstream tools for Milestone 1, or include a minimal safe optimization set.

## Notes

This document is a first draft intended for rapid iteration with design review.
