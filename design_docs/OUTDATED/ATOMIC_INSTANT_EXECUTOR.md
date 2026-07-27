# Atomic-Instant Executor — design

> **PARTIALLY SUPERSEDED (2026-07-25).** The "one reaction per `tick_clock`"
> structure below is retained, but the tick-resolution *phase* was flipped from
> pre-edge to **post-edge continuation** after the dual-convention experiment
> (EXECUTOR_CONVENTION_EXPERIMENT.md) showed the pre-edge choice over-delays
> write-after-tick outputs by one cycle vs hand-written Verilog. The "every output
> appears in the instant its `out.write` runs, held ⇒ registered" rule below is the
> thing that changed: held/registered *output ports* now use an explicit `RegOut`
> instead of a uniform hold-⇒-register. Read this doc for the one-reaction-per-tick
> structure; read EXECUTOR_CONVENTION_EXPERIMENT.md for the current phase convention.

**Status:** design (2026-07-25, branch `synchronous-semantics`). Realizes the
simulation half of SYNCHRONOUS_SEMANTICS.md: **one reaction per `tick_clock`**, so
every output appears in the instant its `out.write` runs (held otherwise) and every
read samples its own instant. Fixes `mac_fsm`, the `if_tick` write-collapse, and the
mid-phase read (`accum_2`) at the root, and replaces the compressed-execution model.

## The invariant

> **Each `tick_clock(N)` runs exactly one reaction — the coroutine code from where
> it suspended (after tick `N-1`) up to the next tick — with inputs `i_N`, and the
> value observed for output `q` after `tick_clock(N)` is `o_N(q)`.**

No reaction ever spans a tick boundary within one `tick_clock` (that is the
"compression" bug: a tick resolving at the current post-edge lets the *next*
reaction run early, in the same `tick_clock`).

## Mechanism: ticks resolve at the *next* pre-edge

`tick_clock` keeps its two settle passes (`pre-edge`, `advance`, `post-edge`) for
combinational settling, but **`clk.tick()` becomes ready only in a pre-edge pass**
(once `cycle >= target`). Concretely, `ClockTick::poll` returns `Ready` iff
`state.cycle >= target && is_pre_edge()`.

Trace of the effect (probe, 2 ticks/iter):

| tick_clock | pre-edge pass | post-edge pass | observed |
|---|---|---|---|
| 0 | resume → run instant 0 (`x = inp[0]`), hit tick, **Pending** (cycle 0 < 1) | tick still Pending (post-edge) | `o_0 = init` |
| 1 | tick ready (cycle 1 ≥ 1, pre-edge) → run instant 1 (`out.write(x)`), hit tick, Pending | Pending | `o_1 = inp[0]` |
| 2 | resume → run instant 2 (`x = inp[2]`), Pending | Pending | `o_2 = inp[0]` (held) |

So exactly one reaction per `tick_clock`; reads see the current instant's inputs;
writes are observed in their own instant. This is the same change prototyped on
2026-07-24 (`copper-core/src/types.rs`, executor sets the pre/post flag) — **now
understood to be correct**, with the "det_010 regression" reinterpreted below.

## Per-construct behavior

- **Read** `p.read()` in instant `N`: returns `i_N(p)`. Because a reaction runs in
  one pre-edge pass, each read naturally samples the input driven before that
  `tick_clock`. This **subsumes `synced_read`** (loop-top and after-tick reads both
  fall out correctly; the mid-phase `accum_2` case is expected to resolve — verify).
- **Write** `q.write(v)` in instant `N`: sets `o_N(q) = v`; unwritten outputs hold.
  A held output is therefore observed in its write instant — the registered trace.
- **State** (`let mut` across a tick): the coroutine's own locals carry the value to
  the next instant; committed by suspension at the tick. No separate register model
  needed — rustc's captured fields *are* the registers.

## The `det_010` case, handled explicitly

`det_010`: `loop { state = next(state, in.read()); clk.tick().await;
if matches!(state,D) { out.write(One) } else { out.write(Zero) } }` — output written
**post-tick**.

Under atomic instants:
- instant 0 = the loop's initial `state = next` (the only segment before tick 0);
  no `out.write`, so `o_0 = init`.
- instant `N ≥ 1` = `out.write(f(state))` (using the state entering the instant)
  then the next `state = next`. So `o_N = f(S_N)`.

The current (compressed) sim and the transpiler both give `o_N = f(S_{N+1})` — one
cycle **earlier**, because the transpiler's `assign out = f(state_r)` publishes the
state combinationally *before* the `out.write` instant, and the compressed sim runs
that `out.write` early. **This is exactly the `probe` situation, not a special
case:** the output is re-timed one cycle later, to the instant where `out.write`
actually runs. The 2026-07-24 prototype "breaking `det_010`" was this re-timing,
mis-read as a regression.

**The one semantic consequence to confirm (not an implementation bug):** re-timing
makes `det_010`'s Moore output appear in the *accepting-state cycle* (one cycle
after the last input) rather than the current "output reflects the current input"
cycle. The former is the standard registered-Moore-detector timing; the latter is
what the existing reference model encodes. Both are legitimate; the atomic model
picks the faithful one (output where `out.write` is). This must be a **conscious
re-baseline of `det_010`'s expected trace**, not silently accepted.

There is no separate startup special-case: every module whose first `out.write` is
after its first tick gets `o_0 = init`, uniformly (`probe`, `det_010`,
`mac_pipeline`, …). Modules that write before their first tick (`counter`:
`out.write(v)` then tick) have their first output in instant 0, also uniformly.

## What must be added (not free)

- **No-tick-iteration guard.** A loop iteration that executes *no* tick would spin
  forever inside one pre-edge pass (a combinational loop). The old `synced_read`
  `same_call` term caught this incidentally; the atomic executor needs an explicit
  guard: an iteration that reaches its wrap point again without an intervening tick
  in the same pass is a combinational loop → the existing oscillation/`MAX_DELTA`
  panic must cover it (verify it fires, with a clear message).
- **Composition / delta settling.** Within an instant, module A's output feeds
  module B's input. Each pre-edge pass must still settle to a fixed point
  (`poll_tasks` already loops until no dirty). Ordering: all modules run their
  instant-`N` reaction in the same pre-edge pass; a produced output must propagate to
  consumers in that pass. Confirm `poll_tasks`'s dirty-driven re-poll achieves this
  now that reactions no longer straddle passes.
- **`synced_read` removal/retention.** If atomic instants make reads correct on
  their own, `synced_read` can be simplified or removed. Keep it until the atomic
  reads are verified against the read-timing suite, then delete if redundant.

## Re-baseline scope

Outputs move from the compressed/early timing to the write-instant timing:

- **Fixed (now pass):** `mac_fsm` (cycle 2), `if_tick`/`branch_merge` (no collapse).
- **Re-timed +1 (re-baseline expected traces):** `probe`/`sample_hold_*`,
  `sum_hold_*`, `mac_pipeline`, `det_010`, and any post-first-tick-output module.
- **Unchanged:** `counter` and other write-before-tick / every-cycle outputs
  (verify each).
- **Transpiler:** currently early/aliased for the re-timed set; aligning it (drop
  aliasing → register held outputs to their write instant) is the *separate*,
  later transpiler task. Sim-vs-Verilator equivalence for those modules is expected
  to be red until the transpiler is aligned — that is acceptable per the current
  "sim semantics first" priority.

## Validation results (2026-07-25)

Applied the pre-edge-tick change and classified every changed sim trace:

- **Fixes (root cause):** `mac_fsm` → `[0,0,10,10,10,…]` (cycle 2, matches the
  registered hardware); `accum_2` mid-phase read now samples the correct (odd)
  cycles (sums `2,6,12`).
- **Uniform +1 re-times (faithful, expected):** `counter` `[0,1,2,…]` (out = count
  *before* the post-tick increment); `sample_hold_2`/`probe` `[0,10,10,12,…]` (=
  the `probe_hand` registered reference); `mac_pipeline`, `det_010`, and the
  executor's own counter unit tests. All are "output appears in its `out.write`
  instant."
- **No hangs.** The spin guard (`synced_read`'s `same_call` term) parks a no-tick
  iteration (`det_010_awaits`'s `rstn=1,in=1` path) at its read until the next
  `tick_clock`; everything *completes*.
- **The one apparent divergence (`det_010` vs `det_010_awaits`) is not a bug.**
  Hand-trace matches the sim exactly. Both detect the same pattern; they differ by
  **one cycle** solely because `det_010` writes its output *post-tick* (+1) while
  `det_010_awaits` writes *at the detection instant*. The compressed executor hid
  that one-cycle gap; the atomic model correctly exposes it.

**Verdict: the atomic model is the correct simulation semantics.** No category-(c)
bug remained after classification.

**Consequence for the test suite:** the sim now *differs from the un-aligned
transpiler* for every +1-re-timed module (the transpiler still aliases/early-drives
their outputs), so their sim-vs-Verilator equivalence tests go red until the
transpiler is aligned (a separate task). `mac_fsm` is the exception — its transpiler
is already registered, so sim == transpiler now.

### One-cycle-latency rule (record for the semantics)

Output latency is determined by **`out.write`'s position relative to ticks**: an
output written before its loop's first tick appears in instant 0; a post-tick write
appears one instant later. Two pattern-equivalent codings are cycle-identical only
if their `out.write`s sit at the same tick offset (see `det_010` vs
`det_010_awaits`). This replaces the transpiler's opportunistic aliasing, which
published values ahead of their `out.write`.

## Validation plan (executed above)

1. Re-apply the pre-edge-tick change; run the whole suite; **classify every changed
   trace** as (a) a fix, (b) a correct +1 re-time, or (c) a genuine bug. Only (c)
   blocks. Use the independent-Verilog references as the arbiter for the ambiguous
   ones (registered variant).
2. Confirm the no-tick guard fires (a deliberately tickless loop panics, not hangs).
3. Confirm reads still match the read-timing suite; then decide `synced_read`'s fate.
4. Re-baseline the +1 tests' expected traces to the write-instant timing; record why.
5. Leave transpiler alignment and `RegOut` for the follow-up branch/phase.
