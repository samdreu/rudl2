# Copper Transpilation Roadmap (Actionable Plan)

**Status date:** 2026-07-14
**Author:** working plan derived from a review of the current code + design docs.
**Relationship to `TRANSPILATION_PLAN.md`:** that document holds the architecture
and the accepted decision log and is still the source of truth for *why* the
pipeline is shaped the way it is. This document is the *what-to-do-next*: it
reflects the code as it actually exists today and lays out the concrete path to
end-to-end Rust → SystemVerilog.

---

## 1. Where the code actually is (verified, not from docs)

The pipeline is `FIR → CHIR → SHIR`, three passes, all landing in `copper-codegen/src`:

| Phase | Pass | File | State | Tests |
|---|---|---|---|---|
| A | Frontend capture (Rust AST → FIR) | `parser.rs` | Implemented | ✅ |
| B | Semantic lowering (FIR → CHIR) | `chir_lower.rs` | Implemented | ✅ |
| C | Timing/state (CHIR → SHIR) | `shir_lower.rs` | Implemented | ✅ |
| D | Legalization (SHIR → VLIR) | `vlir_lower.rs` | **Single-phase + comb path implemented** (D1a/D2/D3/D4/D5 done; tuple-match D6 + conditional output deferred to M2) | ✅ |
| E | Emission (VLIR → SV text) | `emit.rs` | **Implemented** for the above; `verilog.rs` legacy retained until driver lands | ✅ |
| F | Validation (SV vs sim trace) | partial (see §7) | Harness exists; not wired to pipeline | — |

`cargo test -p copper-codegen` → **241 passing** (**391** across the workspace,
all green). A counter now transpiles
end-to-end (`FIR → CHIR → SHIR → VLIR → SystemVerilog`):

- **Library entry:** `copper_codegen::transpile_item_fn`.
- **CLI driver (G) — done:** `copper-transpile <input.rs> [-o out.sv] [--module N]
  [--profile verilator|generic|yosys] [--list]` (`copper-codegen/src/main.rs`).
  Finds hardware modules by `#[hardware]` attr or `Clock/In/Out` signature.
- **Validation (F) — done for the M1 slice:** `tests/m1_counter_equivalence.rs`
  runs the counter in the Copper simulator and Verilates the *generated* `.sv`
  against that trace — `trace: PASS`, `verilator: PASS` across all cycles. The
  DUT source (`tests/fixtures/counter_dut.rs`) is `include!`d for simulation and
  `include_str!`d for transpilation, so the two can't diverge. Output is also
  `verilator --lint-only -Wall` clean.
- **Golden + guard tests:** `counter_golden_output` pins the exact emitted text;
  `tuple_match_is_rejected_not_miscompiled` proves deferred features error out
  rather than miscompile.

**✅ Milestone 1 complete.** A single-clock, single-phase module transpiles
end-to-end and is behaviorally equivalent to the Copper simulation under
Verilator. **Known gap surfaced for M2:** combinational examples using `Logic` +
`.read()` hit `AmbiguousWidth` (CHIR width inference for `Logic` / `.read()`, see
§5.4). `u8`/`bool`/`Bits<N>`-typed modules transpile cleanly today.

### 1a. IMPORTANT — the design docs describe an obsolete port model

The code has moved to a **`In<T, D>` / `Out<T, D>` port model** where every
port carries an explicit clock domain `D`, outputs are driven with `out.write(v)`,
inputs are read with `in.read()`, and the `#[hardware]` macro takes
`sequential` / `combinational` (not `function_typed`). Confirmed in:

- `copper-macros/src/lib.rs` — validates `Clock<D>`/`In<T>`/`Out<T>` params, no return value.
- `copper-codegen/src/chir_lower.rs` — `strip_port_wrapper("In<"/"Out<")`, `.write()` targets.
- `examples/**/*.rs` — e.g. `d: In<Logic, MainClk>`, `out: Out<Bits<N>, MainClk>`, `out.write(out_n)`.

But `TRANSPILATION_PLAN.md`, `FIR_DESIGN.md`, `CHIR_DESIGN.md`, and the worked
examples in several docs still show the **old `function_typed` model** with
return-value outputs and `emit!(value)`. **These docs are stale and will mislead.**
Refreshing them (§5, task D0) is a prerequisite for anyone else picking up the work.

---

## 2. The single biggest gap: nothing produces Verilog yet

SHIR is the current terminus. To get a `.sv` file out of a `.rs` module we need,
in order:

1. **Phase D — VLIR legalization** (`SHIRModule → VLIRModule`)
2. **Phase E — Emission** (`VLIRModule → String`)
3. **A driver** that reads a source module, runs A→E, and writes the `.sv`
4. **Phase F wiring** — feed the generated `.sv` into the existing Verilator harness

Items 1–2 are fully designed already (`VLIR_DESIGN.md`, `EMISSION_DESIGN.md`) and
those designs are current with the In/Out model (they operate on SHIR, which is
already port-model-agnostic). They are the fastest, highest-leverage work.

---

## 3. Milestone 1 — "counter to Verilog, verified" (thin vertical slice)

Goal: pick the **simplest sequential example** (single clock, single phase, one
register, no for-loops, no bit-indexing — e.g. a plain enable counter) and drive
it all the way from `.rs` to Verilator-verified `.sv`. This proves the whole
spine before we widen feature coverage.

### D. Phase D — VLIR legalization  *(implement `copper-core/src/vlir.rs` + `copper-codegen/src/vlir_lower.rs`)*
Per `VLIR_DESIGN.md`. Ship the passes in this order, each with unit tests:
- **D0 (prereq):** refresh stale docs to the In/Out model (§1a).
- **D1** VLIR data types (`VLIRModule`, split `VLIRStmt` / `VLIRFFStmt`, `VLIRExpr`).
- **D1a — parameter seam (do now, so params are additive later; see decision #4):**
  - Replace `usize` width in `CHIRType::UInt/SInt` with a `Width` enum; M1 only ever
    constructs `Width::Concrete(usize)`. Propagate the type change through SHIR/VLIR.
  - Add `params: Vec<ModuleParam>` to the module IR (empty in M1).
  - In the emitter, route every width→text conversion (`[W-1:0]`, literal widths)
    through one helper that can later print a symbolic `N-1`.
  - *Not in scope here:* symbolic arithmetic, `Width::Param`, generate/genvar loops —
    those land with the first parametric module in M2.
- **D2** Name legalization (reserved-keyword map, identifier rules, collision suffixing).
- **D3** Literal width annotation (`SHIRLit{ty} → VLIRExpr::Lit{width}`).
- **D4** Mux→Ternary, Case-expr→case-statement lifting.
- **D5** Multi-phase guard injection (defer if M1 example is single-phase).
- **D6** Tuple-pattern → concat/case lowering (defer if M1 example has no tuple match).
- Assert the VLIR invariants (`VLIR_DESIGN.md` §"VLIR Invariants") in `debug_assert`s.

### E. Phase E — emitter  *(implement `copper-codegen/src/emit.rs`)*
Per `EMISSION_DESIGN.md`. `emit_verilog(&VLIRModule, &EmitConfig) -> String`.
- Port list, reg/wire decls, `always_comb`, `always_ff @(posedge clk)`, continuous `assign`.
- Fully parenthesized expressions; deterministic ordering; `\n` only.
- **Golden-file tests**: snapshot the emitted string per example under `copper-codegen/tests/golden/`.

### G. Driver / entry point  *(new — see §4 for the open question)*
- Read a Rust source file, locate the `async fn` (or comb fn), `syn::parse` it to `ItemFn`.
- Build the `ModuleRegistry` (needed for submodule port names) by scanning all `#[hardware]` fns in the file/crate.
- Run `capture_frontend_ir → lower_to_chir → lower_to_shir → lower_to_vlir → emit_verilog`.
- Write `<module>.sv`.

### F. Validation wiring
The example harness already compares Copper simulation against a hand-written
`.sv` via `.with_verilog("…/sv/foo.sv")` + Verilator (`VERILATOR_VERIFICATION.md`).
- M1 acceptance = **generated** `.sv` for the chosen example passes that same
  harness (byte-diff against the hand-written reference is a bonus, behavioral
  equivalence is the real gate).

**M1 exit criteria:** one sequential + one combinational example transpile
end-to-end and pass Verilator equivalence against their simulation traces.

---

## 4. Open design question — how is transpilation invoked?

There is **no driver today** and the macro does not hook codegen (the old
`Module::get_design_ast` / `to_verilog` path in `verilog.rs` is legacy and
should be retired). Options:

- **(A) Standalone CLI bin** (`copper-codegen` gets `src/main.rs`):
  `copper-transpile path/to/module.rs -o out.sv --profile verilator`. Simplest,
  most testable, no macro coupling. **Recommended for M1.**
- **(B) `build.rs` codegen** that transpiles annotated modules at build time.
- **(C) Macro-driven** — `#[hardware]` also emits the `.sv`. Highest coupling,
  worst for iteration. Not recommended early.

This is the main decision I need from you (see questions at the end).

---

> **See also [TRANSPILATION_COVERAGE_MAP.md](TRANSPILATION_COVERAGE_MAP.md)** — every
> example run through the CLI, grouped into feature classes (sized S/M/L, tagged
> design-first vs example-first) with a dependency-ordered order of attack toward
> the RV32I capstone. That map operationalizes the milestones below.

## 5. Milestone 2 — widen the feature surface

The current examples already use constructs CHIR/SHIR do **not** yet lower.
These are the real content of M2, roughly in dependency order:

1. **Bit indexing & slicing** — `x[i]`, `x[hi:lo]`, `shifted[0] = d.read()`.
   - ✅ **Read-side constant bit index done.** Added an `Index` variant to the FIR
     (`state[0]` used to fall through to a bogus `Lit`); CHIR lowers `base[const]`
     to a 1-bit `Slice` (variable indices are rejected pending loop unrolling) and
     infers it as 1-bit. Also lowered `Logic::One`/`Logic::Zero` constants. Proven
     end-to-end: `d[7] == Logic::One` → `((d[7] == 1'b1) ? … : …)`, Verilator-clean
     (golden + unit tests).
   - **Still open:** range slicing `x[hi:lo]`, and **LHS bit-assignment**
     `shifted[0] = d.read()` (partial register writes — needed by `shift_register`,
     pairs with for-loop unrolling).
2. **`for` loops with constant ranges** — `for i in 1..N { … }`. Must be
   **unrolled** at CHIR time (static bounds only). Currently unsupported.
3. **Const-generic elaboration** — `const N: usize`, `const_assert!`. Requires
   monomorphizing the module at a concrete `N` before lowering. Decide whether
   the driver takes `N` as a parameter or reads it from the example's `main`.
4. **✅ `.read()` / `.as_bool()` / `Logic` / logic-operator width inference — done.**
   (`.as_bool()` now lowers as a value passthrough, like `.read()`.) CHIR's
   `infer_type_from_expr` is now recursive and context-aware (a `SymbolTable` on
   `LowerCtx`): `port.read()` and bare signal refs resolve via declared
   port/wire/register types; comparisons/logical ops → 1-bit `Bool`;
   arithmetic/bitwise/shift → operand width; `if`/`match`-exprs → a branch's type.
   Also fixed an emitter gap this surfaced: intermediate `always_comb` wires are
   now declared as `logic`. `one_bit_comparator` transpiles to Verilator-clean SV
   (golden + inference unit tests added). **Still open here:** unary `!` on a
   multi-bit value currently emits SV logical-`!` (1-bit reduce) instead of
   bitwise `~` — correct for 1-bit `Logic`, wrong for `Bits<N>`; needs operand-type
   dispatch in `lower_unop`.
5. **Multi-phase FSMs end-to-end** (D5) — pattern_detector, uart_fsm.
6. **Tuple-pattern match** (D6) — mux, jk-style.

Each new construct gets: a CHIR/SHIR lowering + unit tests + one example promoted
to generated-`.sv` + Verilator equivalence.

### Next gaps surfaced by running the real examples through the CLI

After the width-inference fix, the remaining `Logic`/`Bits` examples fail on
distinct, separately-scoped issues (not width inference):
- `shift_register` → `Bits<N>` symbolic width (the const-generic / `Width::Param`
  work, decision #4 / item 3 above).
- `mux` → array-typed ports `[Bits<W>; ELS]` (array ports — new).
- ✅ `Bits::from_u32(..)` value constructors — **fixed**. The parser no longer
  mis-extracts the callee path (`Bits::from_u32`) as a type; CHIR now infers the
  width from the constructor (`from_uNN` name or `Bits::<N>::…` turbofish) and
  lowers the call to its argument, retyping a literal to the constructor width.
  With `.as_bool()` (§5.4), constant bit-indexing (item 1), and `Expr::Paren`
  (parser passthrough — `(state >> 1)` no longer becomes a raw `Lit`) now done,
  **✅ `lfsr` now fully transpiles and is behaviorally verified.** The remaining
  pieces landed: `Block`-as-branch-value (`lower_expr`/`infer_type_from_expr`
  handle `ExprType::Block` via the block-tail extractor — also closing the
  `let`-bound `if`/`match` inference gap); pre-loop non-`mut` `let`s lowered to
  combinational wires visible in the loop body (`xor_mask`); and constructor
  arguments retyped to the constructor width (`Bits::from_u32(1 << 31 | …)` now
  emits 32-bit literals, no `WIDTHTRUNC`). Result is Verilator-lint-clean and
  passes `tests/lfsr_equivalence.rs` (`trace: PASS`, `verilator: PASS`, 17 cycles)
  — the second module, after the counter, verified equivalent sim-vs-SV.
  Next example targets need LHS bit-assignment (`shifted[0] = …`) + for-loop
  unrolling (`shift_register`), array ports (`mux`), and symbolic `Bits<N>`
  (const generics, decision #4).
- `pattern_detector` → a still-`AmbiguousWidth` expression form to investigate.
- `rotate_right` / `priority_encode` → CLI "no hardware modules found" (the
  `is_hardware_fn` signature detector misses their shape — investigate).

## 6. Milestone 3 — hierarchy, memory, multi-clock, profiles

Deferred per the decision log; listed here so they aren't forgotten:
- **Submodule hierarchy** — SHIR already carries `SHIRSubmoduleInst`; emit named
  instantiations (EMISSION_DESIGN covers the format). Verify with `ripple_carry_adder`
  / `hierarchical_pipeline`.
- **Memory** — `MEMORY_DESIGN.md`; sim side done, RTL lowering not.
- **Multi-clock / CDC** — `two_domain_counter`; ports already carry domain `D`.
- **Toolchain profiles** — Generic / Verilator / Yosys switch in Phase D/E.
- **RV32I CPU** — the capstone integration test.

---

## 7. Cross-cutting: the Verilator regression suite (Phase F) — ✅ **DONE**

Equivalence is now a `cargo test`-able suite sharing one harness
(`tests/common/mod.rs`): each entry transpiles → Verilates the *generated* `.sv`
→ checks it against the Copper simulator's own trace, and separately checks the
simulator against a reference model. See
[TRANSPILATION_COVERAGE_MAP.md](TRANSPILATION_COVERAGE_MAP.md) §5 for the API and
for how to read a `trace:` vs `verilator:` failure.

**Four modules verified:** `counter`, `lfsr`, `pattern_detector`,
`traffic_light_fsm`. Golden-file tests (§3.E) catch *unintended output churn*;
this suite catches *semantic* regressions — both are in place.

---

## 8. Recommended near-term order of work

1. **D0** refresh stale docs to the In/Out model (unblocks collaborators).
2. **D1 + D1a–D4 + E** the VLIR types, the parameter seam (`Width` enum + `params`
   plumbing + emitter width-routing), core legalization, and emitter for the
   single-phase path.
3. **G** the CLI driver (option A).
4. **F** wire one generated `.sv` through the existing Verilator harness → **M1 done**.
5. Golden tests + equivalence suite (§7).
6. **M2** bit-indexing → for-loop unrolling → const generics (in that order).

---

## 9. Decisions (settled 2026-07-14)

1. **Driver form → standalone CLI bin.** `copper-codegen` gets a `src/main.rs`
   (`copper-transpile module.rs -o out.sv --profile verilator`). No macro coupling;
   retire the legacy `verilog.rs` / `Module::get_design_ast` path.
2. **Stale docs → update them.** As part of D0, refresh `TRANSPILATION_PLAN.md`,
   `FIR_DESIGN.md`, and `CHIR_DESIGN.md` from the `function_typed`/`emit!` model to
   the `In<T,D>`/`Out<T,D>`/`.write()` model so docs match code.
3. **Default profile → Verilator.** It is the only verification path today, so
   emitting Verilator-clean SV guarantees the output passes the existing harness,
   and the Verilator lint subset is also portable to other tools. `Generic`/`Yosys`
   profiles are added later as switches in Phase D/E, not needed for M1.
4. **SV `parameter` is a first-class target — build the seam now, the machinery later.**
   Parameters split into a cheap-to-retrofit-early part and an expensive part that
   costs the same whenever it lands and that M1 never exercises:
   - **Cheap seam — do it in M1 (task D1a):** width lives almost entirely in one
     type (`CHIRType::UInt/SInt { width: usize }`), so change it to a `Width` enum
     (`Concrete(usize)` the only variant used at first), add an empty
     `params: Vec<ModuleParam>` to the module IR, and route width→text through a
     single emitter helper. This makes parameters *additive* later instead of a
     wide `usize → Width` refactor across CHIR/SHIR/VLIR + ~400 tests.
   - **Expensive machinery — defer to the first parametric module (`shift_register`,
     early M2):** the `Width::Param` arm, symbolic width arithmetic/equality (harder
     under the strict "ambiguous = hard error" policy), and `generate`/`genvar`
     loop lowering with symbolic bit-indexing (can't unroll `for i in 1..N` when `N`
     is symbolic). M1's counter has no generics, loops, or slices, so none of this
     blocks the vertical slice.
   - **Driver:** for a module with unbound generics, the driver may still supply a
     concrete `N` to *monomorphize* (unroll) as a fallback; the default path emits a
     parametric module. Both are supported once the M2 machinery lands.

### Still open (non-blocking)

- **M1 target example** — pick the plainest single-phase enable counter unless you
  have a preferred simplest sequential module.

---

## 10. Progress log

Chronological summary of what has actually landed (all with tests; workspace stays
green — currently **241** codegen / **391** total).

**Pipeline build-out (M1):**
- **Phase D — VLIR** (`copper-core/src/vlir.rs`, `copper-codegen/src/vlir_lower.rs`):
  data model, name legalization, literal-width annotation, mux→ternary,
  case-expr lifting, multi-phase guard injection. Deferred forms (tuple match,
  conditional output, nested case-expr) return typed errors, never wrong Verilog.
- **Phase D1a — parameter seam:** `CHIRType` width is a `Width` enum (`Concrete`
  only today; `Param`/`Sub` reserved), module IR carries an empty `params` — so SV
  parameters are additive later, not a pipeline-wide retype. ~88 sites migrated.
- **Phase E — emitter** (`copper-codegen/src/emit.rs`): deterministic, fully
  parenthesized SV; single width→text route; `always_comb`/`always_ff`/continuous
  assigns; combinational wire declarations.
- **CLI driver** (`copper-codegen/src/main.rs`): `copper-transpile`; shared
  `transpile_source()` entry used by CLI + tests.
- **M1 verified:** `tests/m1_counter_equivalence.rs` — counter transpiles and is
  behaviorally equivalent to the Copper sim under Verilator (single-source fixture
  via `include!` + `include_str!`).

**Stale-test cleanup (green workspace):**
- `module_composition_hybrid.rs` — mechanical migration `Arc<Mutex<T>>` →
  `In`/`Out` wire model.
- `verilog_fifo_memory_new.rs` — full rewrite to the pipelined-`Memory`
  (synchronous ReadFirst) model on both the Rust and reference-Verilog sides;
  matches cycle-by-cycle.
- 3 stale `copper-sim` doctests modernized to the current API.

**M2 feature coverage (each: CHIR/SHIR lowering + tests + Verilator-clean output):**
- Context-aware width inference (`SymbolTable` on `LowerCtx`): `port.read()`,
  signal refs, logic/arithmetic operators, `if`/`match`/`Block` tails.
- `Bits::from_uNN(..)` / `Bits::<N>::…` value constructors — width from the name
  or turbofish; argument literals retyped to the constructor width.
- `.as_bool()` value passthrough; `Logic::One` / `Logic::Zero` constants.
- Constant bit-indexing `x[i]` → 1-bit `Slice` (variable indices rejected pending
  unrolling); new FIR `Index` variant.
- `Expr::Paren` passthrough; `ExprType::Block` as an expression value.
- Pre-loop non-`mut` `let`s → combinational wires visible in the loop body.
- **lfsr verified:** `tests/lfsr_equivalence.rs` — a 32-bit LFSR (reset, enable,
  bit-index, shift/xor, pre-loop constant) transpiles Verilator-lint-clean and
  passes sim-vs-SV equivalence (17 cycles). Second verified module after the counter.

---

## 11. Notes & lessons learned (for whoever continues this)

- **Run the real examples through the CLI — it is the best gap-finder.** Each fix
  tends to reveal the next construct the example needs; `lfsr` took ~7 small
  incremental fixes (width inference → constructors → `.as_bool()` → bit-index →
  `Logic::One` → `Paren` → `Block` branches → pre-loop `let`s → constructor-width
  retype) before it went green. Prefer this loop over trying to spec everything up
  front.
- **The Copper simulation is the golden reference, and the single-source fixture
  makes that rigorous.** `include!`-for-sim + `include_str!`-for-transpile means the
  simulated and transpiled DUT are byte-identical, so `finish_with_expected` Verilates
  the *generated* `.sv` against the sim's own trace. This is what would catch a
  pre/post-edge off-by-one — don't replace it with a hand-written reference model.
- **Deferred features must error, not miscompile.** Tuple match, variable bit index,
  and conditional output drives all return typed errors with rewrite hints. There
  are regression tests asserting they error rather than emit wrong Verilog.
- **New front-end features surface latent emitter gaps.** Combinational wire
  declarations were missing but never triggered until width inference let
  combinational `Logic` modules reach the emitter for the first time. Expect more of
  these as coverage widens; lint every new example shape with `verilator --lint-only
  -Wall`.
- **Constructor width = Rust's own inference.** `Bits::from_u32(1 << 31 | …)` — the
  literals *are* `u32` in Rust, so retyping the argument's literals to the
  constructor width is correct, not a coercion, and removes `WIDTHTRUNC`.
- **Triage stale tests by kind.** `module_composition` was a mechanical port-model
  migration; the fifo test was a *semantic* redesign (the `Memory` subsystem moved
  from combinational to pipelined reads) that needed both sides rewritten. Don't
  treat them the same.

### Known latent issues / cleanup backlog

- **Unary `!` on a multi-bit value emits SV logical-`!` (1-bit reduce) instead of
  bitwise `~`.** Correct for 1-bit `Logic`, wrong for `Bits<N>`. Fix: dispatch on
  operand type in `lower_unop`. Not triggered by anything passing today.
- **Untyped integer literals default to 64-bit**, producing width noise like
  `state >> 64'd1`. Harmless (lint-clean) but ugly; a context/expected-width pass
  would clean it up (and is a natural companion to the eventual `Width::Param` work).
- **`let`-bound `if`/`match` inference descends into `Block` tails now**, but general
  expected-type (bidirectional) inference still doesn't exist — widths flow
  bottom-up only.
- **The legacy `copper-codegen/src/verilog.rs`** is dead relative to this pipeline;
  retire it once nothing references the old `Module::get_design_ast` path.
- **Stale design docs** (`TRANSPILATION_PLAN.md`, `FIR_DESIGN.md`, `CHIR_DESIGN.md`)
  still describe the obsolete `function_typed`/`emit!` model — see §1a / task D0.
