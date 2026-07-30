# Copper Executor & Synchronous-Semantics — Implementation Plan

> **Single source of truth** for the executor/semantics work on `feat/reachability-cfg` and its
> follow-ons. Absorbed the former `EXECUTOR_CHANGE_PLAN.md` on 2026-07-29 (that doc is deleted;
> its decision, empirical status, preconditions, and staged phases are folded in here).
> Companion spec: `SYNCHRONOUS_SEMANTICS.md` (updating it is item 5). Paper cross-refs:
> `paper/threats_to_validity.md` (T1, T6), `paper/00_claims_audit.md` (LEAD #1),
> `paper/related_work.md` (coroutine prior art).
>
> **Progress:** items 1 + 1b **DONE** (commits `8e66144`, `3452d38`); item 2 **IN PROGRESS**;
> items 3–7 not started. Gates **G1 DONE** (`design_docs/TIMING_COVERAGE_MATRIX.md` +
> `tests/det_010_independent_golden.rs`), **G3 DONE** (`tests/poll_order_fuzz.rs` +
> `tests/golden_traces.rs`), **G4 DONE** (MyHDL boundary holds; Prost LATTE'26 found →
> contribution 1 re-scopes; recorded in `paper/`), **G6 DONE** (c2 feasible — new
> `copper-analysis` crate consumed by both macro + codegen, no cycle; `syn::ItemFn` input),
> **G2 DONE** (structural reg-match defined + `copper-analysis` capability built), and **G5 DONE**
> (provable-claims register below). **All gates G1–G6 cleared** — item 2 can begin.

## Architecture decision (the foundation) — c2 + "just Rust"

**Approach (c2): the default simulator always executes plain Rust; read-timing/register knowledge
moves out of the runtime heuristic (`synced_read`) into a shared, compile-time control/liveness
analysis (a CFG) consumed by *both* the sim macro and the transpiler.**

Explicitly rejected:
- **(b) IR interpreter as the sim** — having the simulator interpret CHIR/an FSM-IR instead of
  executing the Rust. Rejected because it abandons sim/transpiler independence (what makes the
  same-source equivalence non-circular — T6) and dissolves LEAD contribution #1 ("rustc's async
  transform *is* the FSM"). (A CHIR interpreter may still exist as a *validation-only* backend —
  item 7 — but never as the default sim.)
- **(c1) duplicate analysis** — the macro building its own control analysis independent of
  codegen's. Rejected because register inference (T1) and the FSM report already need one
  authoritative CFG, and two analyses that must nonetheless agree is itself a correctness hazard.

**Why the two constraints are locked:**
1. **"Sim is plain Rust" is load-bearing for the paper.** LEAD #1 = rustc's coroutine lowering is
   the FSM; contribution 2's non-circularity (T6) rests on sim and transpiler being independent
   derivations from one source. Runtime execution stays plain Rust; no runtime timing oracle.
2. **rustc over-captures (T1).** The synthesizable register set can't be read off the `Future`
   layout (a conservative, unstable superset), so Copper needs its own liveness analysis anyway —
   the same reachability/liveness CFG this branch is about. One analysis, three payoffs: register
   inference, read-timing facts, and the FSM report.

## Context / motivation

Copper's execution model is under-specified in two ways that matter for capabilities designs
should support: (1) multiple `clk.tick().await` points per loop iteration, distributed across
branches, not just today's single-trailing-tick shape; and (2) genuine multi-clock designs (e.g.
an async/dual-clock FIFO) as one coherent component, not a hand-wired bundle of separately-spawned
modules.

Investigating both surfaced that Copper's core invariant — "every path through a hardware loop
must eventually reach an await" — **is not actually checked anywhere today**; it holds only as an
accident of the single-tick-per-iteration construction. That accident silently masks real bugs (a
zero-tick branch gets a phantom cycle it never asked for) and stops holding once
branch-tick-distribution generalizes. So formalizing the semantics and making the invariant a
real, checkable analysis is required groundwork regardless of multi-clock.

For multi-clock, the investigation reframed the problem: the missing piece isn't "let one
coroutine tick multiple clocks" (needs new concurrent-loop syntax, and creates a multiple-driver
problem) — it's **hierarchical instantiation of a clocked submodule**, which doesn't exist for
*any* case yet. Real hardware idiom already assumes this: a parent with no `always_ff` of its own
that purely instantiates children, each on its own clock.

**Standing priority:** correct simulation semantics first, transpilation follows — verified
empirically against **independent hand-written Verilog** (BaseJump STL), never against the
transpiler's own output (circular). Overarching: **novel claims that are proven/evidence-backed.**

## Empirical status of the read-timing problem (measured 2026-07-29)

Against **independent hardware**, the sim has **no known read-timing failure**:
- `examples/basejump/sipo_block.rs` — the mid-phase-read question with a **third-party BaseJump
  hardware golden** — **PASSES** cycle-by-cycle under Verilator (uses `RegOut` for the output
  axis). So the mid-phase *read* timing is correct in the current sim.
- The still-ignored cases are **not** hardware-proven read-timing bugs: `accum_2`
  (`tests/read_timing_equivalence.rs`) and `probe_timing_investigation` are sim-vs-**transpiler**
  comparisons whose residual is the output register-vs-combinational (RegOut) axis, not read
  timing; `det_010_variants` (`examples/sequential/pattern_detector_2.rs`) is a **heuristic
  fragility** case that only checks two Copper codings against each other, not hardware.

**Consequence:** the read-timing part of this work (item 3) is a **robustness + capability**
investment — retire a fragile runtime heuristic; gain register inference + the FSM report — **not**
a fix for a live hardware-divergence bug. This lowers its urgency/risk and should be stated
honestly.

## Preconditions (gates) before the c2/CFG-timing work proceeds

The two priorities — **proven/evidence-backed novelty** and **correctness** — make these gates to
clear before the c2 refactor + read-timing retirement (items 3/6 and the c2-sharing of item 2)
proceed. They do *not* block continuing the item-2 CFG scaffolding already in progress on the
branch. G1/G3/G4/G6 are do-first tasks (**all four DONE**); G2/G5 are decisions (**both now
recorded — DONE**).

- **G1 — Timing-pattern coverage matrix + fill the `det_010` gap (correctness). DONE — see
  `design_docs/TIMING_COVERAGE_MATRIX.md`.** c2 makes sim-vs-transpiler *timing* agree by
  construction, so an independent hardware golden becomes the *only* thing that can catch a timing
  bug. Matrix built: 5 of 6 patterns (multi-tick FSM, mid-phase read, Moore-`RegOut` vs Mealy-`Out`,
  memory read latency) are anchored to independent hardware; **the `det_010` variable-iteration-loop
  gap is now filled** by a hand-written independent golden `examples/sequential/sv/pattern_detector_010.sv`
  + `tests/det_010_independent_golden.rs` (canonical `det_010` coding LIVE-passes it;
  `det_010_awaits` is `#[ignore]`d and empirically diverges at the *repeat* detections — the golden
  sides with the canonical Moore semantics, making item 3's claim provable, not self-asserted).
  **One gap remains — CDC/synchronizer latency (pattern 5)** — deferred by design to **item 4**'s
  multi-clock verification (which already calls for an independent async-FIFO Verilog reference); it
  is *not* on item 3's critical path.
- **G2 — Register-inference correctness is defined *structurally* (decision). DONE.** A behavioral
  pass doesn't prove it (an over-approximated register set can still simulate correctly), so
  correctness is defined as a **structural match of the inferred register set against the sequential
  (flip-flop) registers of an independent hand-written reference SV**, with the reference side
  computed by the convention that **nonblocking `<=` targets are the flip-flops** (excluding
  combinational `next_*` blocking-`=` regs and `output reg` — the `RegOut` axis). Two forms,
  empirically calibrated on the actual references:
  - **Name-exact** — valid only when the reference is a *faithful translation* mirroring the design's
    own names (e.g. `mac_fsm.sv` → {stage, product, c_latch, result}).
  - **Storage-equivalent** (count now; count + per-register bit-width once item 2 carries widths from
    resolved Rust types) — the honest bar for a *truly independent* reference whose author chose
    different names/encoding (e.g. `pattern_detector_010.sv`'s two-process Moore `cur_state` vs
    Copper's `state`: names cannot match, count does).
  **Capability built & exercised** in `copper-analysis`: `reference_sv_registers`, `RegMatch`, and
  `assert_registers_match_reference_sv` (unit-tested on `mac_fsm` name-exact + `det_010`
  storage-equivalent). **Hook for item 2:** `tests/common::EquivalenceTest` calls
  `assert_registers_match_reference_sv` once item 2 wires inference into the transpile pipeline
  (today it asserts behavior + Verilator, not the reg set). This is the artifact behind item 2's G5
  claim.
- **G3 — Guardrails land before the refactor (correctness). DONE.** Both artifacts landed: (1) a
  **poll-order fuzzer** — `HardwareExecutor` gained a test-only `PollOrder` knob
  (`Insertion` default = unchanged production behavior; `Reversed`; `Seeded(u64)` reshuffles every
  delta cycle via a dependency-free SplitMix64), driven by an executor unit test
  (`poll_order_does_not_change_cross_task_settle`) and `tests/poll_order_fuzz.rs` (combinational
  settle chain + multi-domain interleave, each asserted identical across reversed/seeded orders); (2)
  **frozen golden traces** — `tests/golden_traces.rs` + committed `tests/golden_traces/*.trace`
  bit-exact snapshots for counter/lfsr/shift_register/det_110101/det_010/mac_pipeline (spanning the
  matrix), with a `BLESS_GOLDEN=1` re-bless path and a verified tamper-detection tripwire. Both
  superseded-by-design once item 6 makes poll order canonical.
- **G4 — Resolve the MyHDL prior-art boundary (novelty). DONE (2026-07-29) — with a bigger find.**
  MyHDL boundary **verified and holds**: its convertible subset is broader than its RTL-synthesis
  subset, and its synthesizable sequential/FSM idiom is single-edge `always_seq` + explicit
  enum-state case; multi-`yield` cycle-slicing is convertible-only, not RTL-synthesizable. **But the
  companion check ("no async-based synthesizable Rust HDL since") surfaced Prost (LATTE '26)** — a
  coroutine-based HDL that independently states contribution 1's core thesis (locals=registers,
  suspension=cycle, procedural multi-cycle algorithm → synthesizable Verilog, Rust-`async`-inspired,
  same loop-must-wait rule). **Consequence: contribution 1 must re-scope** — novelty is the embedded
  *reuse of rustc's own async lowering* + verified same-source sim/synth equivalence + third-party
  hardware anchoring, none of which Prost has (bespoke language+compiler, 3-page vision paper, no
  eval). Recorded in `paper/related_work.md`, `paper/00_claims_audit.md` (LEAD #1),
  `paper/intro_contributions.md`. **Open decision: exact contribution-1 re-wording (user's call).**
- **G5 — Write the exact provable claim per item before coding (novelty; decision). DONE.** Every
  item's claim is now pinned to a *named artifact that proves it* — see the **Provable claims
  register** below. No item ships a novelty claim without its proof artifact named up front.
- **G6 — Prove c2's dependency structure on a vertical slice first (feasibility). DONE — c2 is
  feasible; c1 stays closed.** The open unknown ("can the `copper-macros` proc-macro depend on the
  shared analysis crate without circular/compile-time problems?") is answered **yes**. New crate
  `copper-analysis` (deps: **`copper-core` + `syn` only** — both already macro deps, so ~zero new
  transitive cost) is consumed by **both** `copper-macros` (`hardware` sequential arm) and
  `copper-codegen` (`transpile_source`); the full workspace builds with **no cycle** (`copper-core`
  is a leaf). Register inference driven end-to-end on `mac_fsm` and **structurally reg-matched
  against the independent `tests/fixtures/timing_probe_sv/mac_fsm.sv`** ({stage, product, c_latch,
  result}) — also a live demonstration of G2's structural-match method.
  **Sub-decisions settled by the slice:**
  - *Where the shared analysis lives* → a **new light crate `copper-analysis`**, not an extension of
    heavy `copper-codegen` (which cannot be a proc-macro dependency without bloating every build).
  - *Analysis input* → **`syn::ItemFn`** — the representation BOTH front-ends already hold (the macro
    receives it; `transpile_source` builds it via `parse_file`, and `capture_frontend_ir` already
    keys off `&syn::ItemFn`). So there is **no front-end-unification problem for the analysis**; FIR
    stays codegen's downstream lowering IR (a FIR-based entry `registers_from_fir` is stubbed for
    item 2 if a richer CFG is wanted).
  - *Extend `control_extract` vs new pass* → **new pass in `copper-analysis` that codegen consumes**;
    `control_extract` can later be refactored to consume the shared CFG rather than owning its own.
  NOTE: `infer_registers` here is the **minimal** control-flow criterion (pre-loop state reassigned
  inside the loop); item 2 generalizes it to full backward liveness (registers born inside the loop
  and live across an interior await, e.g. `mac_pipeline`). The slice's calls are read-only (log
  only); item 2/3 route real facts through them. **The three sub-decisions above are USER-APPROVED
  (2026-07-29) as the item-2 foundation.**

## Provable claims register (G5)

Each item's novelty/correctness claim, paired with the **named artifact** that proves it and the
paper contribution it supports. The rule (G5): no item ships a claim without its proof artifact
existing and green — this is what keeps the claims *evidence-backed* rather than asserted.

| Item | Provable claim | Proof artifact (the thing that must be green) | Paper | Status |
|---|---|---|---|---|
| 2 | The synthesizable **register set is inferred from control flow** (not read off rustc's over-capturing `Future` layout) — *and it is correct* | **Structural reg-match vs independent hand-written SV** (G2): `copper-analysis::assert_registers_match_reference_sv` — name-exact for faithful refs (`mac_fsm.sv`), storage-equivalent for independent refs; wired into `tests/common::EquivalenceTest` | C1 | artifact built (G2); claim pending item-2 general liveness |
| 2 | Every path through a hardware loop **reaches a tick** (reachability well-formedness), enforced not accidental | A **constructed malformed loop is rejected** with a spanned compile error + regression tests that uneven-per-branch-tick designs still pass | C1 | pending item 2 |
| 3 | **No runtime timing oracle** — read-timing is compile-time-static; timing is correct against *hardware* | Sim trace ≡ **expanded independent hardware anchor set** (G1 matrix); specifically un-ignoring `det_010_awaits_matches_independent_verilog` vs `pattern_detector_010.sv` | C1/C2 | anchor in place (G1); claim pending item 3 |
| 4 | A dual-clock design is **one coherent hierarchical component**, correct across clock interleavings | Trace/transpile/Verilator equivalence vs an **independent hand-written async-FIFO SV** + a **clock-interleave fuzzer** (≥2 relative tick rates ⇒ equal results) — fills the G1 pattern-5 (CDC) gap | C1/C4 | pending item 4 |
| 5 | The formal semantics (CFG model, liveness rule, reachability, cross-domain interleave independence) are *stated*, construction-independent | `SYNCHRONOUS_SEMANTICS.md` rewrite with the `control_extract.rs:208-210` finding as a worked example | — | pending item 5 |
| 6 | Levelized (topo-once) scheduling gives the **same settled values** as iterate-to-fixpoint, and makes poll-order independence *structural* | Suite green under levelized scheduler + the **poll-order fuzzer becomes moot** (canonical order) | C4 | pending item 6 |
| cross | **Poll-order independence** — a well-formed design simulates identically under any poll order | **Poll-order fuzzer** (G3): `tests/poll_order_fuzz.rs` (insertion ≡ reversed ≡ seeded) | C1/C2 | DONE |
| cross | **No silent behavioral drift** across the refactor | **Frozen golden traces** (G3): `tests/golden_traces.rs` + committed `*.trace` snapshots | — | DONE |
| cross | Sim/synth same-source correspondence is **non-circular** — anchored to third-party hardware | Sim ≡ **BaseJump STL** Verilog (`examples/basejump/`), independent of the transpiler | C2 | DONE (standing) |

Paper key: C1 = `async`-as-FSM (re-scoped for Prost, see G4); C2 = verified same-source correspondence;
C4 = staged transpilation pipeline. See `paper/00_claims_audit.md` and `paper/intro_contributions.md`.

## Implementation items (sequenced)

### 1. Per-domain state keying (DONE — commit `8e66144`)

`PollPhase`/`POLL_PHASE` (`copper-sim/src/lib.rs:24-41`), `TICK_RESOLVING`
(`copper-core/src/types.rs:934-953`), and `CALL_ID` (`copper-sim/src/synced_read.rs`) were
process-global `thread_local`s — ticking one clock domain flipped a flag every other domain's
futures consulted. Now keyed per clock-domain instance. Fixed a latent bug in
`examples/cdc/two_domain_counter.rs` and is a prerequisite for items 2–4.

### 1b. Executor/macro hygiene (DONE — commit `3452d38`)

Panic if a `#[hardware]` future returns `Ready(())` (a hardware coroutine is a `loop {}` that
should never complete); made untracked `spawn()` explicit/test-restricted; removed dead waker
registration. **Still to investigate** (not yet planned concretely): "restore phase after tick
resolution" — confirm whether it's a live bug or a superseded note against current
`tick_clock`/`ClockTick::poll`.

### 2. Reachability/liveness CFG + register inference + FSM report (IN PROGRESS — the heart)

Replace shape-restriction soundness with a real analysis. **Under c2 this CFG must be factored as
the shared analysis both the transpiler and the sim macro consume (see G6).**

- Model each loop as a CFG (`E_comb` vs `E_tick` edges, tick edges labeled by actual clock
  receiver identity — `is_tick_await`/`is_tick_stmt` in `control_extract.rs:286-293` match by
  method name only, not receiver; fix as groundwork for item 4's clock tagging too).
- Well-formedness: deleting all `E_tick` edges must leave the reachable subgraph acyclic (every
  cycle crosses a tick) — a DFS back-edge check, recursive per nested loop. Real pass (new function
  in `control_extract.rs`), a hard, spanned compile error.
- **Replaces** the silent fallthrough at `control_extract.rs:208-210` (currently emits
  `pc_assign(0, ...)` unconditionally) — a zero-tick branch must be rejected, not given a free
  phantom cycle.
- Generalize `as_if_with_tick`/`lower_into`'s branch duplication (`control_extract.rs:168-211`)
  from `If` only to `Match` arms (N arms instead of 2; same duplication-cost caveat).
- Generalize register promotion from `shir_lower.rs`'s linear `split_at_ticks`/segment-index
  bookkeeping (226-257, 395-411) to a real **backward liveness** over the state graph (a var is a
  register iff every path connecting a def and a use crosses a tick). This is the T1 answer — the
  synthesizable register set, computed independently of rustc's over-capture.
- Same CFG also drives **definite-assignment checking** (every combinational output assigned on all
  paths) and **structural combinational-loop detection** (IR-level dependency graph rejecting a
  combinational cycle that doesn't pass through a register/memory-latency/synchronizer). Build
  alongside, not separately.
- Produce a **per-module FSM report** (states, inferred registers, transitions, output logic) as a
  byproduct — falls out of the CFG data structures essentially for free.
- **v1 scope:** full multi-await FSM lowering (not narrowed to one-await-per-loop). Match-arm
  generalization ships in v1; **nested-loop CFG construction** (a genuine basic-block builder,
  since AST-duplication doesn't terminate on back-edges) is the one follow-on phase after v1 lands
  and is verified.

### 3. Retire `synced_read` via CFG-derived compile-time timing facts (gated on item 2 + G1)

The shared CFG classifies each read site statically (which edge registers its result / tick
distance), and the macro bakes that classification into the generated *plain-Rust* sim code,
replacing the runtime `block = wrapped_since && (same_call || …)` predicate. At runtime there is
then no timing oracle — just Rust. Also removes the `det_010`-class fragility (static dataflow the
heuristic structurally lacks). **Prerequisite:** because c2 makes sim-vs-transpiler *timing*
agreement true-by-construction, timing correctness rests entirely on the hardware anchor — **expand
the anchor set (G1) before this item.** Datapath equivalence (sim-vs-transpiler) is unaffected and
stays a genuine cross-check.

### 4. Hierarchical clocked submodule instantiation (the multi-clock enabler)

- `chir_lower.rs::lower_hardware_call` (2556-2616): stop filtering out `Clock<...>` args (2573-2578)
  — thread the clock argument into `CHIRSubmoduleInst`.
- `CHIRSubmoduleInst`/`SHIRSubmoduleInst`/`VLIRSubmoduleInst`
  (`chir.rs`/`shir_lower.rs:194-198`/`vlir_lower.rs:438-441`): add a clock/domain field.
- `emit.rs::submodules()` (216-236): emit an actual `.clk(...)` port connection (today only wires
  data ports + a conventional `.out(...)`; clock wiring flagged unbuilt).
- `copper-macros/src/lib.rs::check_cdc` (111-157): extend the port-signature-only foreign-domain
  check to submodule-instantiation call sites — a child receiving a foreign-domain clock/signal
  must go through a `#[hardware(synchronizer)]` child, same discipline at the call site.
- Verify a "pure hierarchy, no native clock, no loop" parent is legal under
  `validate_hardware_fn`/`has_top_level_loop` (`copper-macros/src/lib.rs:348-357, 722-792`); relax
  if they hard-require a loop/clock.

Resulting dual-clock shape (async FIFO): a parent with no ticks, instantiating `wr_side` (on
`wr_clk`) and `rd_side` (on `rd_clk`); each child instantiates its own `sync_2ff` on its native
domain to pull in the other side's pointer — no new syntax, no multiple-driver problem, reuses the
synchronizer exemption at a new call site.

### 5. Update `SYNCHRONOUS_SEMANTICS.md` with the formal semantics

Replace the draft bullet notes with: the CFG model; cycle-boundary/FSM-state definitions (stated
independent of any construction); the generalized liveness rule; the reachability well-formedness
condition (with the `control_extract.rs:208-210` finding as a worked example of why
shape-restriction soundness isn't enough); and poll-order independence generalized to cross-domain
interleave independence — **a well-formed multi-clock design must simulate identically under any
relative tick-interleaving/rate of independently-ticking domains, provided every crossing goes
through a synchronizer** (real independent clocks have no defined phase relationship). Also record
the c2 + just-Rust decision as the execution model.

### 6. Levelized dependency-graph scheduling (gated on item 3)

Replace the naive iterate-to-fixpoint settle (`poll_tasks` polls every module repeatedly until no
dirty flag) with a **static inter-module dependency graph**, topo-sorted so each module is polled
once per phase in dependency order — the compiled/levelized model of modern fast simulators
(Verilator; cycle-based), vs the dynamic event-queue model of classic kernels (VCS/Questa/Xcelium).

- **A *scheduler* change, not option (b).** It decides the order and count of polls; each module is
  still executed as plain Rust. Orthogonal to the run-Rust-vs-interpret-IR axis.
- **Performance + robustness, not correctness.** Acyclic combinational graph → topo-order-once gives
  the same settled values as iterate-to-fixpoint; cost drops ≈O(tasks × logic-depth) → ≈O(tasks) per
  phase. Ranks after read-timing/register work.
- **Gated on item 3.** `synced_read` defers via `Poll::Pending`, which *requires* polling again —
  it relies on the iterate loop. A single-pass topo schedule has no "poll again." Only after item 3
  removes the runtime deferral is levelized scheduling clean to adopt.
- **Makes poll-order independence structural** (a canonical order replaces the discipline-maintained
  invariant; the poll-order fuzzer becomes moot once the order is canonical).
- **Costs:** new plumbing — the executor holds only `DirtyHandle`s, not producer→consumer edges; the
  `wire()`/port layer must record which `In` reads which `Out`. Cycles don't vanish — SCCs still
  need the fixpoint machinery + oscillation detection ("levelize the DAG, iterate only within SCCs").
- **Distinct from the item-2 CFG:** this is the *inter-module wiring DAG* (scheduling); item 2 is
  *intra-module control flow* (liveness across ticks). Same family, different graphs — don't
  conflate.

### 7. CHIR interpreter as a validation-only backend (OPTIONAL — never the default sim)

Downgraded 2026-07-29 from the former "FSM-IR interpreter migration" (which was option b — see the
architecture decision). What survives: a CHIR interpreter as a **second, optional,
independently-selectable backend that NEVER becomes the default.** In this role it is compatible
with c2 and *strengthens* evidence — a third, cross-checking view (interpret-CHIR vs run-Rust) that
validates the CHIR lowering against the Rust execution semantics, while the raw-Rust executor stays
primary and preserves independence. After items 1–4 (item 2's CFG is the CHIR this interprets).
Build only if the extra cross-check earns its cost; not on the critical path.

Standing preferences if built: **CHIR** as the interpretation authority (closed 10-variant
expression set, no `Call` nodes, explicit `CHIRRegDecl` registers, explicit `AwaitTick`
boundaries); **validation backend, never a cutover** — require it to reproduce the raw-Rust
executor's results on the full suite and agree with independent hand-written Verilog; the raw-Rust
executor **remains the default**. Reusing rustc's own async-fn coroutine as the interpreter target
was investigated and **rejected** (unstable `rustc_private`/MIR-internal; post-borrowck,
monomorphized, no port/type semantics left — the wrong abstraction level; CHIR read off the surface
syntax is what's needed). Real costs: substantial copper-sim work (register file, dataflow
evaluator, hierarchical submodule interpretation, memories); likely slower per-cycle than compiled
native Rust; loses the ability to drop arbitrary host-side Rust in a hardware body during sim. Does
not remove the need for independent BaseJump verification.

## What does NOT change

The executor's phase machinery — pre/post-edge settle, the delta-cycle poll-to-fixpoint loop (until
item 6), the post-edge continuation convention, `RegOut` for the output-timing axis — is validated
and stays. "The executor change" is the read-timing heuristic → static-facts migration (item 3),
not a rewrite of `tick_clock`.

## Verification

- **Discipline:** never treat the transpiler as a correctness oracle for simulator semantics; anchor
  new claims to independent hand-written Verilog (`examples/basejump/`).
- **Register inference (G2):** structural reg-for-reg match of emitted SV against the independent
  hand-written reference, not behavior alone.
- **Reachability check:** a constructed malformed loop (a branch that structurally never ticks) must
  be rejected with a clear compile error; regression tests confirm legitimate designs (uneven
  per-branch tick counts) still pass, now *verified* rather than accidentally sound.
- **Poll-order fuzzer (G3):** randomized/reversed `poll_tasks` order asserts identical results —
  a pre-item-6 regression guard, superseded once item 6 makes the order canonical.
- **Frozen golden traces (G3):** bit-exact snapshots of currently-passing examples guard against
  silent behavioral drift across the refactor.
- **Multi-clock:** a dual-clock example (async FIFO, or `two_domain_counter` extended into one
  hierarchical component) — verify trace, transpile, Verilator equivalence, and agreement with an
  independent hand-written async-FIFO Verilog reference. Plus a **clock-interleave fuzzer**: run the
  design under ≥2 relative `tick_clock` interleavings/rates and assert equal observable results
  (operationalizes item 5's generalized invariant).
- `.claude/skills/run-copper/smoke.sh --test` and the full workspace suite stay green throughout;
  per-step regressions triaged against the `#[ignore]` conventions (each ignore cites a design doc).

## Out of scope / resolved

- **Mid-phase read-timing is no longer a standing open bug.** The old "cocotb-vs-Esterel startup
  semantics" question (archived `design_docs/OUTDATED/SESSION_HANDOFF_READ_TIMING.md`) is
  effectively resolved against independent hardware (see *Empirical status* — `sipo_block` passes).
  What remains is the `det_010`-class heuristic *fragility*, addressed by item 3's static timing.
- **Reusing rustc's async-fn coroutine as an FSM-IR target** — investigated and rejected (item 7).

## Open sub-decisions

- ~~Where the shared analysis crate physically lives and how `copper-macros` depends on it without a
  heavy compile-time cost~~ **SETTLED by G6:** new light crate `copper-analysis` (`copper-core` +
  `syn` only); both `copper-macros` and `copper-codegen` depend on it; no cycle (`copper-core` is a
  leaf); analysis input is `syn::ItemFn` (both front-ends already hold it).
- ~~Whether item 2's CFG extends codegen's existing `control_extract` or is a new pass it consumes~~
  **SETTLED by G6:** a **new pass in `copper-analysis`** that codegen consumes; `control_extract`
  (in heavy `copper-codegen`) cannot be a proc-macro dependency, so the authoritative CFG lives in
  the light shared crate and `control_extract` is later refactored to consume it.
- Exact form of the per-read timing fact the CFG emits (tick-distance integer vs edge-phase tag). *(open)*
- Sequencing of item 4 (multi-clock) relative to item 3 (retire `synced_read`) — largely
  independent; order by whichever capability is wanted first. *(open)*
