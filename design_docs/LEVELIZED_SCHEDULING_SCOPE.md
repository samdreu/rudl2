# Item 6 — Levelized dependency-graph scheduling: scope & design

> Scope for impl-plan item 6 (`SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md`). Written before
> implementation. Records the current model, the chosen approach, the key technical risk, a
> phased plan, and the validation strategy. **Graph-acquisition decision: additive spawn API
> (user-approved 2026-07-30).**

## Goal

Replace the naive iterate-to-fixpoint settle (`poll_tasks` polls every task repeatedly until no
dirty flag) with a **static inter-module dependency graph**, topo-sorted so each task is polled
**once per phase in dependency order** — the levelized/compiled model of modern cycle-based
simulators (Verilator), vs the dynamic event-queue model of classic kernels.

**This is performance + robustness, not correctness.** The current model already produces correct,
hardware-anchored results (BaseJump equivalence, the full example suite). The payoffs:

- cost drops ≈`O(tasks × logic-depth)` → ≈`O(tasks)` per phase;
- **poll-order independence becomes structural** — a canonical order replaces the
  discipline-maintained invariant, so the poll-order fuzzer (`tests/poll_order_fuzz.rs`) is retired;
- **static combinational-loop detection** falls out (replacing the runtime `OSCILLATION_THRESHOLD`
  panic) — its designated home (moved here from item 2: intra-module comb loops are unexpressible,
  so the only real comb loops are cross-module, visible only in this inter-module wiring graph).

Not urgent at current example sizes; an architecture investment. The graph-acquisition plumbing
dominates the cost.

## Gate (item 3) — confirmed clear

Item 6 was gated on item 3 because the old `synced_read` deferred via `Poll::Pending`, requiring
re-polls. Verified clear: there is **no remaining within-phase "poll again" requirement**.
`PreEdgeBarrier` (`copper-sim/src/lib.rs`) is phase-*constant* within a settle — `Pending` for all of
post-edge, `Ready` for all of pre-edge — and `clk.tick()` resolves only at the pre→post transition
(a separate `poll_tasks` call). So the **only** reason `poll_tasks` iterates today is combinational
settling order, which a topo pass eliminates. `delta_yield` likewise runs a combinational body once
per phase. The gate holds.

## Current model (what changes)

`HardwareExecutor::poll_tasks` (`copper-sim/src/executor.rs:294`) runs a delta-cycle loop: each
delta polls every task in `visit_order` (the `PollOrder` knob), takes each task's **output**
`DirtyHandle`s, and repeats until a full pass is clean; `OSCILLATION_THRESHOLD` (20) /
`MAX_DELTA_CYCLES` (1000) guard non-convergence.

The blocking fact: **the executor has no producer→consumer edges.** A `TaskEntry`
(`executor.rs:102`) holds only `future` + output `port_dirties`. A wire is a shared
`Arc<Mutex<T>>` cell (`copper-core/src/port.rs:53`) cloned into both the `Out` (cell + dirty flag)
and the `In` (cell only); the `In` is **moved into the module future** and is invisible to the
executor. There is no record of which task reads which wire.

## Decision — how the executor learns the wiring graph

Three approaches were weighed:

| Approach | Blast radius | Robustness |
|---|---|---|
| **A. Additive spawn API** (callers pass input wire-ids at spawn) | ~49 files / 85 spawn sites | precise, explicit |
| B. Port instrumentation (`In::read`/`Out::write` self-register via thread-local current-task) | zero | fragile: conditional reads undiscovered until their branch runs |
| C. Hybrid (instrument + warmup + validate vs fixpoint) | low | robust but most complex |

**Chosen: A — additive spawn API (user-approved 2026-07-30).** The honest, explicit model: a wire's
identity is its cell pointer (`Arc::as_ptr` as a `WireId(usize)`), exposed by `In::wire_id()` /
`Out::wire_id()`; `spawn_wired`/`spawn_untracked`/`spawn_child` gain the task's **input** wire-ids
(they already take output `DirtyHandle`s). The executor then builds `producer(writes wire W) →
consumer(reads wire W)` edges by matching wire ids. Cost is a mechanical sweep of ~49 spawn sites;
correctness of the graph does not depend on execution reaching every branch (unlike B).

## Key technical risk — combinational vs sequential edges (false cycles)

A naive "every input → every output" edge set **over-approximates** and creates **false cycles** for
*legal* sequential feedback: module A reads B's registered output while B reads A's — fine across a
tick, but a cycle in the read→write graph, which topo-sort cannot order and a comb-loop check would
wrongly reject.

Resolution: edges must be **combinational only**. A `RegOut` commits via the `ClockEdgeListener` at
the edge (not during settle — `copper-core/src/port.rs`), so a `RegOut` is a phase **source**, never
a settle-phase sink; only plain-`Out` writes *during the poll* are combinational sinks. Port *kind*
(Out vs RegOut) is therefore the discriminator, and it is information the executor can carry per
registered output.

**Residual edge case:** a plain `Out` legitimately *holding* a registered value (the enabled-register
idiom — `sim ≡ BaseJump` on `bsg_dff_en`) could appear in a feedback path and be misclassified as
combinational. Per convention (CLAUDE.md) write-before-tick Moore outputs use `RegOut` and plain
`Out` reflects combinational logic, so mutual plain-`Out` feedback is *genuinely* a comb loop — but
this residual must be tested (a Moore design on plain `Out` inside a cross-module loop). The item-2/
item-3 analysis already classifies comb-vs-registered per port and can supply this if needed.

## Phased plan (each phase independently validatable)

1. **Wire identity + read/write registration.** `WireId` = cell `Arc::as_ptr`; `In::wire_id()` /
   `Out::wire_id()`; extend the spawn API with input wire-ids; sweep the ~49 spawn sites. Behavior
   unchanged (still iterate-to-fixpoint) — this phase only *records* the graph.
2. **Build DAG + topo order, behind an opt-in scheduler mode** mirroring the existing `PollOrder`
   knob (default stays iterate-to-fixpoint). Levelized pass = one topo-ordered poll per phase.
3. **SCCs.** Tarjan over comb edges; single-pass for the acyclic part, iterate-to-fixpoint **only
   within** an SCC (registers / memory latency / synchronizer break cycles).
4. **Validate + flip default.** The levelized order must reproduce, bit-for-bit: the G3 golden
   traces (`tests/golden_traces.rs`, no re-bless), the poll-order fuzzer (`tests/poll_order_fuzz.rs`),
   and BaseJump equivalence. Then it becomes the default and the fuzzer is retired (poll-order
   independence is now structural).
5. **Static comb-loop detection.** Reject a comb-edge SCC not broken by a register / memory-latency /
   synchronizer, with a clear error — replacing the runtime `OSCILLATION_THRESHOLD` panic
   (`executor.rs`). Add a constructed cross-module comb-loop regression.

## Validation strategy

Anchored to the existing guardrails, which is exactly why they were built (G3): **frozen golden
traces** (bit-exact, no re-bless), the **poll-order fuzzer** (must stay green until step 4 retires
it), and **independent BaseJump equivalence**. The levelized scheduler is correct iff it reproduces
all three unchanged.

## Open sub-questions (resolve during implementation)

- **Multi-domain graph.** `tick_clock` polls all tasks each settle; cross-domain comb paths should
  not exist (crossings are registered synchronizers). Confirm the comb DAG is effectively
  within-domain and that other-domain tasks are quiescent sources under the topo pass.
- **Dynamic spawn.** `spawn_child` (structural hierarchy) and `RegOut`/`Memory` `ClockEdgeListener`
  registrations add graph nodes/sources; build the graph lazily on first `tick_clock` and invalidate
  on new spawns.
- **Comb-module `delta_yield`.** A `#[hardware(combinational)]` body runs once per phase under a
  single topo pass; confirm no design relies on multiple `delta_yield` passes within one phase (the
  gate analysis says none should).
