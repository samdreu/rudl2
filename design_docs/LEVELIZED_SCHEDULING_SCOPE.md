# Item 6 — Levelized dependency-graph scheduling: as-built design

> Scope for impl-plan item 6 (`SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md`), written
> 2026-07-30 before implementation and kept as the design record. **All five phases
> landed in `56bfe5e` (2026-08-03).** `SchedulerMode::Levelized` is the compiled
> default (`SchedulerMode::env_default` in `copper-sim/src/executor.rs`;
> `COPPER_SCHEDULER=fixpoint|levelized` overrides it); the fixpoint scheduler is
> retained permanently as the differential oracle. Graph acquisition is the
> additive spawn API (approach A below).

## Goal

Replace the naive iterate-to-fixpoint settle (`poll_tasks` polls every task repeatedly until no
dirty flag) with a **static inter-module dependency graph**, topo-sorted so each task is polled
**once per phase in dependency order** — the levelized/compiled model of modern cycle-based
simulators (Verilator), vs the dynamic event-queue model of classic kernels.

**The *feature's purpose* is performance + robustness, not a correctness fix** — the fixpoint model
already produced correct, hardware-anchored results (BaseJump equivalence, the full example suite).
The payoffs:

- cost drops ≈`O(tasks × logic-depth)` → ≈`O(tasks)` per phase;
- **poll-order independence becomes structural** — a canonical order replaces the
  discipline-maintained invariant. The poll-order fuzzer (`tests/poll_order_fuzz.rs`) is *not*
  deleted: it is pinned to `SchedulerMode::Fixpoint` and guards the oracle;
- **static combinational-loop detection** falls out (`HardwareExecutor::comb_cycles`) — its
  designated home (moved here from item 2: intra-module comb loops are unexpressible, so the only
  real comb loops are cross-module, visible only in this inter-module wiring graph).

**But the *migration itself* was held to a correctness-first bar.** The new scheduler had to be
behavior-identical to the old one, validated differentially, so a regression could not slip in to
be debugged later. "Not urgent" referred to *timing*, never to rigor: this is an architecture
investment whose whole point is to make the invariant *structural*, which is worthless if the
migration itself introduces divergence.

## Correctness discipline

The fixpoint scheduler is the correctness oracle for this change; the levelized scheduler was
only allowed to ship once it was indistinguishable from it. The mechanism, as built:

- **Differential equivalence harness (the spine) — `tests/levelized_differential.rs`.** Runs the
  *same* design + stimulus under **both** `SchedulerMode`s and asserts identical settled values on
  **every wire after every phase of every cycle** (not just end-of-run), so a divergence is
  localized to the exact design, cycle, and phase that breaks. What runs under both schedulers
  *continuously* is: the six hand-built topologies in that file (`diff_comb_chain`,
  `diff_diamond_fanout_fanin`, `diff_register_then_comb`, `diff_regout_pipeline`,
  `diff_convergent_comb_loop`, `diff_multi_domain_independent`) and the six frozen golden-trace
  designs in `tests/golden_traces.rs`, each checked under `Fixpoint` and `Levelized`. The
  **corpus-wide** run (full `cargo test --workspace` and every example under
  `COPPER_SCHEDULER=fixpoint`, with the levelized default) was a one-off at the default flip, not
  a standing gate; the env override remains so it can be repeated.
- **Both schedulers stay in the tree, permanently.** Like the `PollOrder` knob, the fixpoint
  scheduler was not deleted when levelized became default — it remains as the differential oracle
  and a permanent regression guard.
- **Never reject a design the fixpoint model accepts.** Static comb-loop detection (phase 5)
  rejects **exactly** the SCCs the runtime `OSCILLATION_THRESHOLD` already catches — no false
  positives on passing designs. When edge classification is uncertain, **SCC-iteration wins over
  panicking**: correctness (reproduce the fixpoint result) beats the performance of a single pass.
  A loop is reported only when the settle actually fails to converge.
- **Behavior-neutral phases first.** Phase 1 only *recorded* the graph; scheduling was unchanged,
  so it could not regress. Phase 2 landed the scheduler **opt-in** (default stayed fixpoint) — it
  changed nothing in production until the differential harness was green.
- **The G3 guardrails were the ship gate.** Frozen golden traces (bit-exact, no re-bless), the
  poll-order fuzzer, and independent BaseJump equivalence all stayed green with the levelized
  scheduler before the default flipped.

## Gate (item 3) — confirmed clear

Item 6 was gated on item 3 because the old `synced_read` deferred via `Poll::Pending`, requiring
re-polls. Verified clear: there is **no within-phase "poll again" requirement**. `PreEdgeBarrier`
(`copper-sim/src/lib.rs`) is phase-*constant* within a settle — `Pending` for all of post-edge,
`Ready` for all of pre-edge — and `clk.tick()` resolves only at the pre→post transition (a
separate `poll_tasks` call). So the **only** reason `poll_tasks` iterated was combinational
settling order, which a topo pass eliminates. `delta_yield` likewise runs a combinational body once
per phase. The gate holds.

## The model that was replaced

`HardwareExecutor::poll_tasks` ran a delta-cycle loop (now `poll_tasks_fixpoint`, dispatched to
under `SchedulerMode::Fixpoint`): each delta polls every task in `visit_order` (the `PollOrder`
knob), takes each task's **output** `DirtyHandle`s, and repeats until a full pass is clean;
`OSCILLATION_THRESHOLD` (20) / `MAX_DELTA_CYCLES` (1000) guard non-convergence. Both bounds are
shared with the levelized SCC iteration.

The blocking fact: **the executor had no producer→consumer edges.** A `TaskEntry` held only
`future` + output `port_dirties`. A wire is a shared `Arc<Mutex<T>>` cell (`copper-core/src/port.rs`)
cloned into both the `Out` (cell + dirty flag) and the `In` (cell only); the `In` is **moved into
the module future** and is invisible to the executor. There was no record of which task reads
which wire.

## Decision — how the executor learns the wiring graph

Three approaches were weighed:

| Approach | Blast radius | Robustness |
|---|---|---|
| **A. Additive spawn API** (callers pass input wire-ids at spawn) | every spawn site (~85 across the tree at the time) | precise, explicit |
| B. Port instrumentation (`In::read`/`Out::write` self-register via thread-local current-task) | zero | fragile: conditional reads undiscovered until their branch runs |
| C. Hybrid (instrument + warmup + validate vs fixpoint) | low | robust but most complex |

**Chosen and built: A.** The honest, explicit model: a wire's identity is its cell pointer
(`WireId`, exposed by `In::wire_id()` / `Out::wire_id()` in `copper-core/src/port.rs`);
`spawn_wired` / `spawn_untracked` / `spawn_child` take the task's **input** wire-ids as `reads:
Vec<WireId>` (they already took output `DirtyHandle`s). The executor builds `producer(writes wire
W) → consumer(reads wire W)` edges by matching wire ids (`compute_scc_plan`). Correctness of the
graph does not depend on execution reaching every branch (unlike B). The generated corpus cases
pass `reads` too (`build.rs`'s `emit_test`).

## Key technical risk — combinational vs sequential edges (false cycles)

A naive "every input → every output" edge set **over-approximates** and creates **false cycles** for
*legal* sequential feedback: module A reads B's registered output while B reads A's — fine across a
tick, but a cycle in the read→write graph, which topo-sort cannot order and a comb-loop check would
wrongly reject.

Resolution: edges are **combinational only**. A `RegOut` commits via the `ClockEdgeListener` at
the edge (not during settle — `copper-core/src/port.rs`), so a `RegOut` is a phase **source**, never
a settle-phase sink; only plain-`Out` writes *during the poll* are combinational sinks. Port *kind*
is the discriminator, carried as `WireKind` (`Comb` / `Registered`) on every `DirtyHandle`.

**Residual edge case:** a plain `Out` legitimately *holding* a registered value (the enabled-register
idiom — `sim ≡ BaseJump` on `bsg_dff_en`) could appear in a feedback path and be classified as
combinational. Per convention (CLAUDE.md) write-before-tick Moore outputs use `RegOut` and plain
`Out` reflects combinational logic, so mutual plain-`Out` feedback is *genuinely* a comb loop —
and because a convergent SCC is iterated rather than rejected (below), a Moore design on plain
`Out` inside a cross-module loop still simulates correctly; it is merely not single-passed.

## The phases, all landed

1. **Wire identity + read/write registration.** `WireId` = cell pointer; `In::wire_id()` /
   `Out::wire_id()`; the spawn API takes input wire-ids; every spawn site swept. Behavior
   unchanged (still iterate-to-fixpoint) — this phase only *recorded* the graph.
2. **DAG + topo order behind an opt-in scheduler mode** mirroring the `PollOrder` knob. Levelized
   pass = one topo-ordered poll per phase (`poll_tasks_levelized`). The differential harness
   (`tests/levelized_differential.rs`) landed in the same phase, so the scheduler was developed
   *against* it. `WireKind` on `DirtyHandle` is the comb-only edge discriminator.
3. **SCCs.** Tarjan over comb edges (`tarjan_scc`); single-pass for the acyclic part,
   iterate-to-fixpoint **only within** an SCC (`iterate_scc`). *Finding:* a register-broken
   "cycle" is a `RegOut` back-edge → no comb edge → acyclic, so the only comb-graph SCCs are
   genuine plain-`Out` loops; convergent ones stay legal iterated SCCs.
4. **Validate + flip default.** Gated on the differential harness, the corpus green under
   levelized via the `COPPER_SCHEDULER` override (full `cargo test --workspace` **and** every
   example — today `tools/regression.sh` bare), bit-for-bit G3 golden traces (no re-bless, run
   under **both** schedulers), the poll-order fuzzer, and BaseJump equivalence. Levelized is the
   compiled default (`SchedulerMode::env_default`); the fixpoint scheduler stays as the permanent
   oracle (`COPPER_SCHEDULER=fixpoint`, and pinned in the differential/golden/poll-order tests).
   The poll-order fuzzer is *retired as a production guardrail* — pinned to Fixpoint, it guards
   only the oracle (poll-order independence is structural under levelized).
5. **Static comb-loop detection.** A multi-task comb-edge SCC is a combinational cycle with no
   register / memory-latency / synchronizer to break it (those commit at the edge → no comb edge),
   *identified structurally* from the dependency graph and exposed by
   `HardwareExecutor::comb_cycles()` (detect without running). Per the correctness constraint
   (reject **exactly** what `OSCILLATION_THRESHOLD` catches — convergence is function-dependent
   and undecidable statically), a *convergent* cycle (e.g. a set-dominant latch) is **not**
   rejected: it is legal and iterated to a fixpoint. Only a *non-convergent* cycle fails, and
   only when the settle does not converge — it is a **panic** from `iterate_scc` that names the
   **whole SCC** and how to break it (`RegOut` or a synchronizer), replacing the old vague
   single-task `OSCILLATION_THRESHOLD` panic. Regressions in `copper-sim/src/executor.rs`:
   `levelized_oscillating_scc_trips_threshold`, `levelized_three_module_cycle_reports_whole_scc`
   (assert the structural message), `comb_cycles_reports_convergent_latch_without_rejecting_it`,
   `comb_cycles_empty_for_acyclic_design`; and `tests/simulator_corner_cases.rs`
   `non_convergent_combinational_loop_is_detected`. `comb_cycles()` is detection, not rejection —
   nothing calls it to refuse a design.

## Validation strategy

Anchored to the existing guardrails, which is exactly why they were built (G3): **frozen golden
traces** (bit-exact, no re-bless), the **poll-order fuzzer** (green through the flip, then pinned to
the oracle), and **independent BaseJump equivalence**. The levelized scheduler was correct iff it
reproduced all three unchanged, and it did.

## Sub-questions raised at scoping, and how they resolved

- **Multi-domain graph.** `tick_clock` polls all tasks each settle; cross-domain comb paths do not
  exist (crossings are registered synchronizers), so the comb DAG is within-domain and other-domain
  tasks are quiescent sources under the topo pass. Pinned by `diff_multi_domain_independent`.
- **Dynamic spawn.** `spawn_child` (structural hierarchy) and `RegOut` / `Memory`
  `ClockEdgeListener` registrations add nodes and sources. Implemented as a lazily-built plan:
  `ensure_graph` rebuilds `levelized_plan` when `graph_dirty` is set by a spawn, so a run of spawns
  costs one build.
- **Comb-module `delta_yield`.** A `#[hardware(combinational)]` body runs once per phase under a
  single topo pass. The gate analysis said no design relies on repeated `delta_yield` passes within
  one phase, and the corpus under levelized confirmed it.
