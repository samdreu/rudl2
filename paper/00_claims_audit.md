# Copper — Code-Grounded Claims Audit & Positioning

**Purpose:** single source of truth for what Copper *actually does* (verified against the
crates, not the design docs, which are out of sync in several places) and how each claim
should be framed against prior work. Read this before writing any paper prose.

Last verified against source: **2026-08-27** (see §Re-verification 2026-08-27 — the
equivalence scope changed materially; earlier sections stand as the dated record).

---

## What the code actually does (verified)

| Feature | Mechanism in source | Evidence |
|---|---|---|
| Clock domains | Phantom type `Clock<Domain: ClockDomain>` with `PhantomData<Domain>` | `copper-core/src/types.rs:865` |
| Clock-domain-crossing safety | **Phantom-type**, not ownership: `In<T,D>`/`Out<T,D>` carry `PhantomData<D>`; cross-domain pass = `E0308` type mismatch | `copper-core/src/port.rs:82,111`; `copper-core/src/cdc.rs` (compile_fail doctests ARE the spec) |
| Single-driver guarantee | `Out<T,D>` is **non-`Clone`** (move-only); `In<T,D>` is `Clone`. One writer per wire, by Rust move semantics | `copper-core/src/port.rs:64-118` |
| Async/await FSMs | `#[hardware]` macro only validates + injects a sim barrier; **rustc's own async→state-machine lowering is the FSM the *simulator* runs**. Vars live across `.await` are the design's registers — but the *synthesizable* set is Copper's own liveness result, **not** rustc's over-captured `Future` fields; the emitted netlist is an independent transpiler lowering. See §Claim-scope correction (2026-07-30) | `copper-macros/src/lib.rs:118-138`; `examples/sequential/traffic_light_fsm.rs:26-28` |
| Same-source sim = synth | DUT source included once as Rust (rustc sim) and once via `include_str!` → `copper_codegen::transpile_source` → SV; traces compared under Verilator. **As of 2026-08-27 every example module transpiles (34/34) and the corpus sweep covers 32/34 — see §Re-verification** | `tests/lfsr_equivalence.rs`; `build.rs` corpus sweep (G-D); `tests/rv32i_pipelined_verilator.rs` |
| Transpiler pipeline | FIR → control_extract → CHIR → SHIR → VLIR → SV; entry `copper_codegen::transpile_source`; CLI `copper-transpile` | `copper-codegen/src/{parser,control_extract,chir_lower,shir_lower,vlir_lower,emit}.rs`, `main.rs` |

## Doc/code mismatches to fix before publication — ALL RESOLVED (2026-08-27)
- ~~README says `#[hardware(function_typed)]`~~ — gone; the README documents the
  real modes (`sequential`/`combinational`/`synchronizer`/`structural`).
- ~~Several examples use bare `async fn` + `HardwareExecutor` with no macro~~ —
  every example file spawning modules now carries `#[hardware]` modules
  (re-checked mechanically: no `HardwareExecutor`-using example lacks the attribute).
- ~~README claims "first HDL to use ownership semantics for compile-time CDC"~~ —
  the README now states CDC as phantom-typed, with no priority claim.

---

## Positioning decisions

**LEAD (implemented + differentiated):**
1. **async/await as the FSM surface, realized by the host compiler's coroutine lowering.**
   Among the *typed-HDL / RTL-generator* competitors, none does this (Arch = declarative state
   blocks; Spade = pipeline stages; Clash = Mealy functions; Anvil = `wait`/events; RHDL = SSA
   lowering + explicit registers). Framing: *we don't build an FSM compiler for the **simulation** —
   rustc's async transform runs as the reference FSM; the synthesizable register set is Copper's own
   liveness result and the emitted netlist is an independent transpiler lowering* (see §Claim-scope
   correction 2026-07-30 — do **not** phrase this as "rustc's fields **are** the [synthesized]
   registers"; that overclaims and contradicts §Threats T1).
   **PRIOR-ART CORRECTION (2026-07-29, verified via web + RHDL LATTE'25 + Prost LATTE'26 + MyHDL
   0.11 manual): drop any blanket "no competitor uses coroutines for hardware" claim — it is
   false, and the coroutine-as-synthesizable-FSM *idea itself* is no longer uniquely ours.**
   MyHDL (Python generators, `yield`=resume-condition; simulate-by-running + convert to Verilog)
   and cocotb (`async`/`await` + `await RisingEdge(clk)`, testbench-only) precede us on the raw
   coroutine mechanism. **More significantly, Prost (LATTE '26) independently proposes the exact
   thesis** — coroutines as the fundamental synchronous-circuit abstraction, locals=registers,
   suspension=cycle boundary, procedural multi-cycle algorithm synthesized to Verilog next-state
   logic, Rust-`async`-syntax-inspired, even the same "each loop must contain ≥1 wait"
   well-formedness rule. **Contribution 1 therefore re-scopes: the novelty is NOT "coroutine as a
   synthesizable multi-cycle FSM" (Prost states that too) but the specific realization.** The
   *defensible* conjunction (see `related_work.md`): (a) **embedded in Rust, reusing `rustc`'s own
   `async` lowering as the FSM** — no bespoke coroutine compiler — vs Prost's bespoke language +
   compiler that only borrows Rust's syntax; (b) registers = the **general-purpose compiler's
   captured live-across-suspension state**, not explicit `Signal`s + an edge decorator (MyHDL) nor
   a bespoke compiler's state values (Prost); (c) `async` used for **design, not verification**
   (vs cocotb); (d) same source **run by `rustc` and transpiled, verified equivalent + anchored to
   third-party BaseJump hardware** — no equivalence/anchoring story exists in Prost or MyHDL. Also
   note MyHDL's *synthesizable/RTL* subset excludes multi-`yield` cycle-slicing (its convertible
   subset is broader than the RTL-synthesis subset; synthesizable sequential = single-edge
   `always_seq` + explicit enum-state FSM). Also **RETIRE "it's
   just Rust"/"simulate by running the host language" as standalone novelty** — RHDL uses "it's
   just Rust" verbatim, and Clash/RHDL both simulate-by-running; those are supporting properties
   of claim (a)–(d), not headline claims.
2. **Same literal source → cycle-accurate sim AND SystemVerilog, verified equivalent.**
   Frame as a *correctness* property (backed by the equivalence harness), NOT as "unified
   sim/synth" (table stakes — Chisel/PyMTL/Clash/Bluespec all claim that).

**SUPPORT (real but not novel / narrower than claimed):**
3. Type-driven safety = phantom clock domains (**cite Clash & Arch as prior art — we match,
   not beat**) + **ownership-based single-driver** (move-only `Out` — the honest "Rust
   ownership" contribution, distinct from Arch's dependency-graph single-driver rule).

**RETIRE:** all "first HDL" / "ownership-based CDC" phrasing.

## Current status caveat (C2) — as of 2026-07-16 on branch `transpilation/fir-chir-shir` — RESOLVED
**(Resolved long since: both tests are tracked and green, and the equivalence
evidence now spans the whole example corpus — §Re-verification 2026-08-27.
Kept as the dated record of when the claim was NOT yet demonstrable.)**
The `counter` + `lfsr` equivalence tests (`tests/m1_counter_equivalence.rs`,
`tests/lfsr_equivalence.rs`) are **new, untracked, and currently FAILING**: the in-progress
parser refactor emits `ExprType::Path` for a bare-identifier `port.write()` receiver, but
`chir_lower.rs` still matches only `ExprType::Lit` (3 sites: `:696`, `:744`, `:804`), so no
`.write()`-using module transpiles. The README's "verified equivalent" wording describes the
intended/last-known-good state, not the current tree. **Transpiler fix is planned by the
author.** Any paper claim of demonstrated equivalence must wait until these tests are green.

## Executor-convention decision (2026-07-25) — anchors C2, adds a semantics sub-result
The simulator's cross-tick **output timing** was investigated against independent hand-written
Verilog (not the transpiler). Finding: the executor's tick-phase choice is an **irreducible
dual** — pre-edge continuation makes write-before-tick outputs (Moore FSM, e.g. `mac_fsm`)
match hardware but write-after-tick outputs (flip-flop, enabled register, sync-RAM read) off by
one cycle; post-edge continuation does the exact reverse. No single global convention is correct
for both. See `design_docs/EXECUTOR_CONVENTION_EXPERIMENT.md` (probes:
`tests/timing_probe_investigation.rs`, `tests/mem_latency_probe.rs`; refs:
`tests/fixtures/timing_probe_sv/*.sv`).

**Decision: adopt the post-edge base (the more hardware-standard convention — a register clocked
at edge N is observable in cycle N) + an explicit `RegOut` annotation for held/registered output
ports.** This makes the primitive constructs match hand-written Verilog, which is what lets C2
("cycle-accurate", "semantic reference") be a *hardware* claim rather than a self-consistency
check. Two paper consequences:
- **C2/C4 reframed** to "sim anchored to independent hand-written references," defusing the
  circularity attack ("you only proved your sim agrees with your transpiler").
- **New contribution 5**: output-timing inference is provably insufficient at exactly one point;
  `RegOut` is the minimal, characterized annotation. This is a partial answer to the
  "no formal semantics" weakness below.
- **Guardrail added**: do NOT claim "zero timing annotations."

## Claim-scope correction (2026-07-30) — contribution 1 does not annex the synthesized FSM or the register set

**The conflict.** The intro's contribution-1 prose said *"that generated state machine **is** the
FSM, and its fields **are** the registers"* — which contradicts §Threats **T1**, whose whole point
is that rustc's `Future` layout is a *conservative superset*, so "we must **not** claim rustc's
`Future` layout **is** the register set." Two different FSMs were being conflated:

- the **simulation** FSM — genuinely rustc's coroutine, which the executor *runs* (this is the true,
  load-bearing, novel-conjunction part); and
- the **synthesized** FSM — Copper's *independent* reconstruction (`control_extract` → `match pc` →
  SV), which does **not** use rustc's lowering at all.

Likewise the **register set**: rustc *over-captures* (T1), so the synthesizable set is computed by
Copper's own liveness analysis, **not** read off the `Future` fields.

**Corrected scoping (now reflected in `intro_contributions.md` and the two spots above).** rustc's
async lowering is the mechanism for the **reference simulation** and the **semantic correspondence**
— not for the emitted netlist and not for the register set. State it as: *the async surface faithfully
denotes a synchronous FSM; we demonstrate faithfulness by running rustc's own lowering as a
hardware-anchored cycle-accurate sim, and we transpile the same source to RTL independently, verified
equivalent; the synthesizable register set is Copper's liveness result.* This is exactly the G4
re-scope ("reuse of rustc's async lowering" = the sim running it) and now has evidence:
`copper-analysis` liveness CFG (the register analysis), `register_reconciliation.rs` (codegen ≡ shared
inference — 97 clocked modules as of 2026-08-27), and the G2 structural reg-match vs 4 independent hand-written SVs.

**Is the corrected claim still strong? Yes — arguably stronger, but its *character* changes.**
- **Stronger:** (a) it stops contradicting T1 (an internal contradiction a reviewer would exploit);
  (b) it *protects* contribution 2 — had the synthesized FSM literally been rustc's coroutine, sim ≡
  transpiler would be near-tautological; independence is what makes the equivalence a real cross-check;
  (c) the register-set work moves from a false "for free" claim to a real, verified analysis, which is
  a genuine result, not a retreat.
- **Weaker as a headline:** the deflation a reviewer will try is *"if the netlist and the register set
  are both Copper's own work, what does rustc's coroutine buy synthesis?"* Honest answer: **for
  synthesis, nothing directly** — it buys the free, faithful, hardware-anchored *reference simulator*
  and the *evidence* that the async surface is a faithful FSM description. So contribution 1 is a
  **systems + verification** claim ("stock rustc `async`, run unmodified as the semantic reference,
  plus an independent verified transpiler, plus correct liveness-based registers"), **not** a
  novel-mechanism claim ("async-as-FSM" — prior art: Prost/MyHDL/cocotb, per G4). Billed as the
  former, it holds; billed as the latter, it was never ours to claim and the "is/are" wording made the
  overreach look like the thesis. The novelty remains the *conjunction* + third-party hardware
  anchoring, which the correction leaves intact and cleaner.

## Known weaknesses a reviewer will attack (re-scoped 2026-08-27)
- ~~Transpiler coverage is thin~~ — RESOLVED: 34/34 example modules transpile, 131
  corpus differential cases green, and the pipelined RV32I CPU is itself an
  equivalence result. The residual version of this attack: the **language is
  narrowed by design** — five pre-tick alignment rules, the mid-phase read seam,
  and one write port per array register refuse shapes rather than support them.
  Pre-empt by framing restriction as the mechanism (the cycle-dataflow model
  refuses what it cannot give one meaning), with the guardrail doc's five
  REJECTED fixes as evidence the boundary is measured, not convenient.
- **No formal semantics / soundness argument yet.** Filament (PLDI'23), Anvil (ASPLOS'26),
  Spade all carry one. Leading with a "correctness guarantee" invites demand for a proof
  sketch of the async-lowering ↔ transpiler correspondence, or a much larger verified set.
  Partial mitigation now on file: `design_docs/CYCLE_DATAFLOW_SEMANTICS.md` is a
  normative denotation (one value per signal per cycle, commit/forwarding rules)
  from which the guard rules are derived — a semantics document, not a proof.
- **Third-party anchoring is 7 designs.** The sweep (131 cases) proves sim ≡
  emitted-SV CONSISTENCY; only the BaseJump anchors and the hand-written
  references adjudicate SEMANTICS. The CPU lane strengthens this (13
  architectural programs are an ISA-level oracle), but "the two agree" and "the
  two are right" remain distinct claims — keep them distinct in prose.

---

## Scope of the equivalence claim (added 2026-08-22) — SUPERSEDED by §Re-verification 2026-08-27

**(Kept as the dated record. Every row of the table below has since landed, and
the boxed CPU trap is obsolete in the opposite direction — the CPU is now an
equivalence result. Do not cite this section; cite the re-verification.)**


The same-source claim is real but **bounded**, and the bound is not a rough edge. State it as
*"the same source simulates and synthesises, provably in agreement, for the subset the transpiler
supports."* Outside that subset there is no transpiled artifact at all, so there is nothing to
agree with.

Outside the subset today, each pinned by a test that flips loudly if support lands
(`copper-codegen/tests/unsupported_constructs.rs`, `transpile_inference_gaps.rs`):

| construct | status |
|---|---|
| **`Memory`** | does not transpile at all — see Threats T7 |
| array-typed ports (`mux`) | unsupported |
| `/` (note `%` transpiles) | unsupported |
| `arithmetic_shift_right` | unsupported |
| `[Logic; N]` array locals, tuple-returning helpers | bit-width inference gap |
| generic modules | monomorphised by the macro at example-run time, not by the standalone CLI |

**The trap to avoid in prose:** the RISC-V CPU is the most impressive example in the repo and is
*not* covered by the equivalence claim — it uses `Memory` and a `Vec` port, and does not transpile.
`tests/rv32i_integration.rs` is a simulator self-check against known program results, not a
sim≡synth check. Any figure or sentence that puts the CPU next to the equivalence claim needs to
say which one it is demonstrating.

**Two further limits on the evidence itself**, both in Threats: X-propagation cannot be checked
against Verilator at all (T8 — Verilator is 2-state and the X initialiser is dropped in
transpilation), and the harness producing all of this evidence had five defects that could make a
check pass or vanish silently (T9). Neither invalidates the claim; both bound how much weight it
carries, and both are worth pre-empting rather than having a reviewer find.

---

## Re-verification 2026-08-27 — the equivalence scope is now the whole example corpus

Everything in the 2026-08-22 scope table landed within five days of being
written down, so the claim's shape changes. Verified against source and the
current regression (`REGRESSION OK`: 1006 tests / 0 failures across 122
binaries; 26/26 examples; 131 corpus differential cases green, 12
ignored-with-reason):

| 2026-08-22 said | 2026-08-27 reality | evidence |
|---|---|---|
| `Memory` does not transpile | transpiles — declared, preloaded, multi-port, WriteFirst, and RECEIVED as a parameter (bus ABI) | `examples/memory/dual_port_ram.rs`; `design_docs/RECEIVED_MEMORY_ABI.md`; `tests/received_memory_abi.rs` |
| `arithmetic_shift_right` unsupported | lowers to `$signed(a) >>> n` | `unsupported_constructs.rs::arithmetic_shift_right_is_supported`; `fx_struct_pipeline_dut::sra_method` (swept) |
| tuple-returning helpers = inference gap | tuple/struct/block bindings all lower | `tests/fixtures/struct_pipeline_dut.rs` (5 modules, swept); `ripple_carry_adder` swept |
| the CPU "does not transpile … no sentence may place it beside the equivalence claim" | **the CPU IS an equivalence result**: transpiles (892 SV SLOC per `paper/stats/loc.csv`, from 208 SLOC of Rust), lints under `-Wall`, and matches the simulator **cycle-for-cycle on all 13 architectural programs** under a Verilated owner | `tests/rv32i_pipelined_verilator.rs` |

**Transpiler coverage is 34/34** (`tools/transpile_coverage.sh`; the per-cause
history is in the repo `TODO`, all entries retired). The corpus sweep covers
32/34 — the two exceptions are the structural multi-clock parent (no
simulatable body by design) and the pipelined CPU (its `Memory` parameter
cannot be supplied by the sweep harness; the dedicated lane above is its
behavioural gate, and is *stronger* than a sweep case: seeded-random stimulus
becomes 13 ISA-level programs).

**How to state the claim now.** Not "for the subset the transpiler supports" —
that bound is gone. The honest bound moved from *capability* to *language
design*: Copper **refuses** constructs it cannot give one meaning in simulation
and silicon (the five pre-tick alignment rules, the mid-phase read seam, one
write port per array register, the refused wait orderings and memory shapes),
each with a spanned diagnostic and a measured counterexample on file. Suggested
sentence: *"every example design — including a 5-stage pipelined RV32I CPU —
simulates and synthesises from one source, verified cycle-equivalent under
Verilator; constructs that cannot carry one meaning across both are compile
errors, not lowered approximations."*

**What the CPU lane is evidence FOR** (be precise; it earned it): the aggregate
surface (struct pipeline latches, tuple/block bindings), const match patterns,
word-indexed array registers with WB→ID write-through, the received-memory bus
ABI under a WriteFirst owner, control extraction of a halt state, and the
edge-form staging rules — each of which was FOUND WRONG by this lane and fixed
with a pinned fixture before the traces matched. It is a discovery instrument,
not a demo.

**Still true and still worth pre-empting** (unchanged from 2026-08-22): the
X-propagation limit (T8 — Verilator is 2-state), the harness-defect history
(T9 — why the wiring guards G-A…G-D exist), and the generic-module CLI note
(monomorphisation happens at example-run time; the CLI emits parametric SV and
concrete widths come from the harness). The `%`-yes-`/`-no division asymmetry
remains the one pure capability gap in the operator surface.
