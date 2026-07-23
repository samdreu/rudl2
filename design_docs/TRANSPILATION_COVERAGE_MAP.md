# Copper Transpilation — Coverage Map

**Status date:** 2026-07-14. Companion to
[TRANSPILATION_ROADMAP.md](TRANSPILATION_ROADMAP.md) (the roadmap is the *why/what-next*;
this map is *which examples force which features, and in what order*).

## How to read this

Every example module was run through `copper-transpile` to find exactly where it
stops today. From those failure points, the required work is grouped into
**feature classes**, each tagged:

- **Size** — `S` (small/local), `M` (medium), `L` (large/cross-cutting).
- **Approach** — *example-first* (just let the failing example drive the fix; cheap,
  self-revealing) or *design-first* (changes an IR type or adds a lowering path —
  decide the representation before coding, à la the `Width` seam). The tell:
  **does closing it change an IR type or add a new lowering path? If yes → design-first.**

The scope is anchored on the **RV32I CPU capstone**; the intermediate examples are
the stepping stones that force each feature, and each becomes a Verilator
equivalence test the moment it goes green.

---

## 1. Status snapshot (all 16 modules)

| Module | Status today | First blocker | Feature class |
|---|---|---|---|
| `combinational/one_bit_comparator` | ✅ transpiles | — | (done) |
| `sequential/lfsr` | ✅ verified | — | (done) |
| `sequential/pattern_detector` | ❌ | enum state `let mut state = State::IDLE` | Enums, Tuple-match |
| `sequential/traffic_light_fsm` | ❌ | enum + `(phase, timer) = match …` | Enums, Tuple-match, Tuple-assign |
| `cdc/two_domain_counter` | ❌ | `Bits::from_lit::<1>()` (0-arg ctor) | Const constructors, Multi-clock |
| `sequential/shift_register` | ❌ | symbolic `Bits<N>` | Const generics, For-loops, LHS bit-assign |
| `combinational/rotate_right` | ❌ | not detected (function-typed) | **Migration**, Const generics, For-loops |
| `combinational/priority_encode` | ❌ | not detected (function-typed) | **Migration**, Const generics, For-loops |
| `combinational/ripple_carry_adder` | ❌ | not detected (function-typed) | **Migration**, Submodule hierarchy |
| `combinational/mux` | ❌ | array port `[Bits<W>; ELS]` | Array ports, Const generics |
| `sequential/pipeline_mac` | ❌ | ambiguous width (FSM regs) | Enums / width inference; multi-module |
| `memory/dual_port_ram` | ❌ | `Memory::<…>` as a wire | Memory RTL lowering |
| `uart/rx` | ❌ | `while` loop | **Migration** (rewrite to `loop`+tick) |
| `uart/system` | ❌ | 3 modules + submodule wiring | Submodule hierarchy |
| `cpu/rv32i_cpu` | ❌ | `Vec<Bits<32>>` (register file) | Enums+methods, Arrays/regfile, Memory |
| `cpu/rv32i_cpu_pipelined` | ❌ | `Vec<Bits<32>>` | (capstone, pipelined) |

Two green today (counter — via M1 — and lfsr). The rest cluster into a handful of
feature classes below.

---

## 2. Feature-class catalog

### A. Enums as state — ✅ **DONE**
`enum State { … }` as a register (`let mut state = State::IDLE`) and matched.
**Landed:** an `EnumRegistry` (name → {width, variant→value}) built per module and
threaded through CHIR. Explicit discriminants are honored, unannotated variants
continue sequentially (Rust's own rule), and width is `bits_for(max_value)`.
Enum paths resolve in width **inference**, expression **lowering**, **register init**
(reset value), and **match patterns** (→ `case` selector values). File-scope enums
are not reachable from an `ItemFn`, so they are captured by
`parser::capture_file_enums` and injected into `FrontendModuleIR::enums` by
`transpile_source` (hence the new `transpile_fir` entry point).

**Also landed to make enum FSMs usable:** *conditional (Moore) output drives* —
a port driven inside `if`/`match` now lowers to `VLIRStmt::PortAssign` inside
`always_comb` (assigned on every path, so no latch), instead of the previous
`ConditionalOutputUnsupported` error. `match`-statement lowering to `VLIRStmt::Case`
came with it.

Verified by `enum_state_machine_golden_output` (3-variant FSM → `logic [1:0] state`,
`case`, Moore output) — Verilator-lint-clean. `pattern_detector` and
`traffic_light_fsm` now advance past enums and block only on class B.

### B. Tuple match + tuple assignment — ✅ **DONE**
**Landed:**
- **Tuple values** — `ExprType::Tuple` lowers to `CHIRExpr::Concat` (first element
  most-significant), so `match (state, in)` gets a `{state, in_i}` selector.
  Inference sums element widths.
- **Tuple patterns** — `flatten_tuple_pattern` folds a (possibly nested) tuple of
  literals into one `(width, value)` selector literal, per VLIR_DESIGN §Pass 2.
  A wildcard *inside* a tuple has no single selector value and is rejected rather
  than silently mis-matched.
- **Tuple destructuring assignment** — `(a, b) = rhs` splits into one assignment
  per element via `project_tuple_element`, which pushes the index through
  `match`/`if`/block-tail so each element gets its own conditional expression.
- **Nested case-expressions** — `if … else { match … }` flattens to
  `Mux(cond, x, Case{…})`; a `Case` in expression position now lowers to a
  **ternary chain** (with guard support) instead of the old `NestedCaseExpr` error.
  A `Case` at the top of a register update still lifts to a `case` statement.

`pattern_detector` is fully green, Verilator-lint-clean, and **behaviorally
verified** (`tests/pattern_detector_equivalence.rs` — `trace: PASS`,
`verilator: PASS`), making it the third verified module after `counter` and `lfsr`.

### B2. Pattern bindings + guards + partial wildcards — ✅ **DONE**
**Landed:** a match whose arms are not all fully-literal now lowers to a
**per-element condition chain** instead of a concatenated-selector `case`
(`match_expr_is_case_compatible` picks between the two, so fully-literal matches
keep the nicer `case` form). Each arm's condition is built only from its
*literal* positions — wildcards constrain nothing — combined with the arm guard.
A **binder** contributes no condition but names its scrutinee element via a
binding scope on `LowerCtx`, so `t` resolves to `timer` in both the guard and the
body. Or-patterns OR their alternatives' conditions (rejected when combined with
a binder).

**Exhaustiveness:** an enum-exhaustive match with no `_` arm (traffic_light has
four `Phase` arms) works because rustc has already proven exhaustiveness — the
final arm's condition is implied and it becomes the fallback. A *guarded* final
arm is rejected, since then no branch is guaranteed to produce a value.

**Two correctness fixes this surfaced:**
1. **Tuple assignment must be simultaneous.** Splitting `(phase, timer) = …` into
   sequential assignments let Phase C's register forwarding feed the *new* `phase`
   into `timer`'s expression. Each projection is now evaluated into a wire first
   (`phase_next_val`, `timer_next_val`), then the registers are assigned from
   those wires. This also removed a massive expression blow-up.
2. **Trailing-segment wires were dropped.** Phase C only extracted register
   updates from the post-tick segment, discarding `Wire` statements — so those
   next-state wires vanished. They are now hoisted into the same phase's pre-edge
   (only wires; port drives keep their timing).

**Width propagation (fixes long-standing `WIDTHEXPAND` noise):** an untyped
literal now takes the width of the other binop operand (`timer < 1` → `timer <
8'd1`), and an assignment target's width propagates into untyped literals in
*value* positions of its RHS (conditions and selectors are deliberately left
alone). This also improved `lfsr` (`state >> 64'd1` → `>> 32'd1`).

`traffic_light_fsm` is fully green, Verilator-lint-clean, and **behaviorally
verified** (`tests/traffic_light_equivalence.rs` — `trace: PASS`,
`verilator: PASS`). **The FSM cluster is complete.**

### C. Const generics / symbolic `Bits<N>` — **L, design-first**
`fn m<const N: usize>(… Bits<N> …)`, `Bits<N>`, `const { assert!(…) }`.
This is decision #4's `Width::Param` payoff — the seam exists (`Width` enum), the
machinery does not: symbolic width arithmetic + equality, and either SV
`parameter` emission or driver-supplied monomorphization.
**Forces:** `shift_register`, `rotate_right`, `priority_encode`, `ripple_carry_adder`, `mux`.
*Design note:* decide **parameter-emit vs monomorphize** first (roadmap #4 leans
"monomorphize now, parameters later"). That choice gates the rest.

### D. For-loop unrolling + LHS bit-assignment — **M/L, design-first**
`for i in 1..N { shifted[i] = out_n[i-1]; }`. Needs a CHIR unroll pass over
**constant** ranges (post-monomorphization, so `N` is concrete) plus **LHS bit
assignment** (`x[i] = …` → partial register/wire update, distinct from the RHS
bit-read that already works). Depends on C for the bounds.
**Forces:** `shift_register`, `rotate_right`, `priority_encode`.

### E. Array / `Vec` ports & register files — **L, design-first**
`[Bits<W>; ELS]` (mux) and `Vec<Bits<32>>` (rv32i register file). Needs an array
signal type in the IR and indexed access lowering to packed arrays or a memory.
**Forces:** `mux`, `rv32i`. *Design note:* a small fixed `Vec` register file may
be better modeled as Memory (class F) than as a packed array — decide per use.

### F. Memory RTL lowering — **L, design-first (MEMORY_DESIGN.md)**
`Memory::<T, R, W, D, RLAT, WLAT>` → synthesizable RAM. Simulation side is done;
RTL emission is not. The synchronous/ReadFirst semantics are now pinned down (see
the fifo equivalence rewrite) which de-risks this.
**Forces:** `dual_port_ram`, `rv32i` (instruction/data memory).

### G. Submodule hierarchy emission — **M, design-first (EMISSION_DESIGN covers format)**
SHIR already carries `SHIRSubmoduleInst`; the emitter has a placeholder for the
callee output-port name. Needs the real port name threaded from the registry, then
named-port instantiation.
**Forces:** `ripple_carry_adder`, `uart/system`, `rv32i`.

### H. Custom methods / const constructors — ✅ **DONE**
**Landed:**
- **`ValueCtor` classification.** The turbofish *position* matters: a **trailing**
  one is a method const-parameter (a **value** — `Bits::from_lit::<1>`), while one
  **earlier** in the path is the type's **width** (`Bits::<8>::from_u8`). The old
  code read `from_lit::<1>` as width-1. Now `FromInt { width }` (value from the
  argument) is distinguished from `Const { value }` (fixed value, width from
  context — `from_lit::<V>()`, `zero()`), the latter emitted at the default
  literal width so the surrounding assignment/operand sets the real width.
- **Conversion methods as value passthroughs** (like `.read()`): `as_u8`…`as_u128`,
  `as_usize`, `as_bits`, and `clone` — in both lowering and inference.

**Now transpiling + Verilator-lint-clean:** all three `two_domain_counter` modules
(`fast_counter`, `sync_2ff`, `slow_consumer`) and `mac_pipeline`.

### H2. Latch inference is now rejected — ✅ **DONE** (surfaced by H)
Transpiling `pipeline_mac` revealed that class A's conditional-`always_comb`
lowering could emit a **latch** when a signal is assigned on only some control
paths (`mac_fsm` writes `out` only in its `Out` arm). That is precisely the bug
Copper's README lists first among the Verilog pitfalls the language exists to
prevent — so emitting it silently was unacceptable.

Phase D now runs a path analysis (`assigned_on_any_path` − `assigned_on_all_paths`)
over every combinational block and rejects the difference with an actionable
`LatchInferred` diagnostic. Crucially it recognises **exhaustive cases**: a
`default`-less `case` whose arm labels cover all `2^width` selector values is
complete (the traffic-light Moore output — four arms over a 2-bit enum), so there
are no false positives on the verified modules. Both directions are regression-tested.

*Remaining design question:* Copper semantics say an unwritten `Out` port **holds**
its value, which is a *register*, not a latch. Lowering a conditionally-written
port to a registered hold would make `mac_fsm` work rather than error. Tracked as
a follow-up; erroring is the correct interim behaviour.

### I. Multi-clock / CDC — **L, design-first (somewhat orthogonal)**
Two domains + a synchronizer (`two_domain_counter`). Ports already carry domain
`D`; needs per-domain `always_ff` and CDC-aware emission. Can be deferred — nothing
on the RV32I path needs it.

---

## 3. Not features — migration / cleanup (do opportunistically)

These aren't transpiler gaps; the examples are stale or mis-shaped:

- ✅ **Function-typed examples migrated.** `rotate_right`, `priority_encode`,
  `ripple_carry_adder` now use `#[hardware(combinational)]` + `In`/`Out` (const
  generics kept); `full_adder` stays a plain combinational helper. They simulate
  and are detected, but still need class C (const generics) + class D (for-loops,
  dynamic bit select) before they transpile.
- **`uart/rx`** uses a `while` loop; rewrite to `loop { … clk.tick().await; }` (the
  error already suggests the rewrite).
- **Multi-module files** (`two_domain`, `pipeline_mac`, `uart/system`) — the CLI needs
  `--module`, which already works. Not a gap; the equivalence suite just names the module.

---

## 4. Dependency-ordered order of attack (toward RV32I)

Each step turns ≥1 example green and is gated only by earlier steps. Verify each
with a Verilator equivalence test before moving on.

1. ~~**Enums as state (A)**~~ ✅ **done** — plus conditional Moore outputs.
2. ~~**Tuple match + assign (B)**~~ ✅ **done** — `pattern_detector` green + verified.
2b. ~~**Pattern bindings/guards (B2)**~~ ✅ **done** — `traffic_light_fsm` green +
   verified. **The FSM cluster is complete** (4 modules now behaviorally verified:
   `counter`, `lfsr`, `pattern_detector`, `traffic_light_fsm`).
3. ~~**Const-constructor / method rules (H)**~~ ✅ **done** — cleared all three
   `two_domain` modules and `mac_pipeline`; also added latch rejection (H2).
4. **Const generics decision + monomorphize (C)** → the gating design choice for the
   combinational-generics cluster.
5. **For-loop unroll + LHS bit-assign (D)** → `shift_register`; with the migration in
   §3, also `rotate_right`, `priority_encode`.
6. **Submodule hierarchy (G)** → `ripple_carry_adder`, then `uart/system`.
7. **Arrays / register file (E)** and **Memory RTL (F)** → `mux`, `dual_port_ram`,
   and the rv32i register file + memories.
8. **RV32I capstone** → integrate A–H; `rv32i_cpu` then `rv32i_cpu_pipelined`.
9. **Multi-clock/CDC (I)** → `two_domain_counter`, whenever convenient (off the
   critical path).

Rough milestone grouping: **M2** = steps 1–5 (FSMs + generics/loops), **M3** =
steps 6–8 (hierarchy, memory, CPU), with I as an M3 side-quest.

---

## 5. Cross-cutting: the equivalence suite — ✅ **DONE**

The four equivalence tests now share one harness, `tests/common/mod.rs`:

- `EquivalenceTest::new(module_name, DUT_SRC)` transpiles the fixture and wires up
  `HardwareTest` against the generated `.sv`.
- `.record(inputs, actual, expected)` accumulates the simulator's actual trace and
  the reference model's expected trace in one call (cycle numbering is automatic).
- `.finish()` asserts the simulator matched the reference **and** that the
  generated SystemVerilog matches the simulator under Verilator.
- `logic(bool)` / `transpile_fixture(..)` helpers; `for_module(..)` selects one
  module from a multi-module fixture.

Deliberately *not* generic: port wiring, stimulus, and the reference model stay in
each test, because DUT signatures genuinely differ — abstracting those would cost
more than it saves. Only the boilerplate and the assertion are shared.

**Reading a failure** (documented in the harness, and worth internalizing):
- `verilator: FAIL` → the transpiler disagrees with the simulator — a real
  transpiler bug, since the simulator is the semantic source of truth.
- `trace: FAIL` with `verilator: PASS` → the transpiler is fine; the test's own
  reference model is wrong. (This is exactly how the traffic-light reference
  off-by-one was diagnosed.)

Verified the harness still fails correctly by perturbing a reference model
(`trace: FAIL`, `verilator: PASS`, with the mismatching cycle and bit vectors
reported) before restoring it.

Adding a newly-green example is now: drop a fixture in `tests/fixtures/`, wire its
ports, write a reference model, call `record`/`finish`.
