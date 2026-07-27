# Session handoff — read-timing / `clk.tick().await` semantics (2026-07-26)

Read this before continuing the read-timing work. It separates **empirical facts**
(re-runnable, trustworthy) from **interpretation** (my analysis this session, which
was repeatedly wrong and reversed — treat as hypotheses to re-verify, NOT settled).

The user's explicit plan for the next session: **walk the examples by hand against
real hardware semantics** to decide the correct read timing, rather than trusting
the model I was iterating on. Follow that. Do not treat the transpiler as a golden
while deciding *simulator* semantics — it is Copper's own artifact (circular). Use
independent hand-written hardware (BaseJump) and established simulator/synchronous
semantics as references.

---

## Tree state (where things are right now)

- **Last commit:** `75f9c26` "transpiler: lower RegOut to a registered output
  (always_ff); un-ignore mac_fsm". Everything through there is committed, pushed to
  `origin/synchronous-semantics`, and green.
- **Uncommitted (this session's leftovers):**
  - `examples/basejump/sipo_block.rs`, `examples/basejump/sv/sipo_block.sv`, and its
    `Cargo.toml` `[[example]]` entry — the mid-phase-read reproduction. **Currently
    FAILS by design** (it is the repro).
  - Nothing else. The WIP read-timing change to `copper-sim/src/synced_read.rs` was
    **reverted** — `synced_read.rs` is back at the committed version. The sim is NOT
    in a half-changed state.

## Empirical facts (re-runnable; these are trustworthy)

Under the **current committed sim** (`cargo run --example <name>`):

| Example (vs independent BaseJump Verilog) | Result | Coding shape |
|---|---|---|
| `bsg_dff_en` | **PASS** | `loop { tick; if en {out.write(data.read())} }` (tick-first) |
| `bsg_counter_up_down` | **PASS** | `loop { tick; …reads…; count_o.write() }` (tick-first) |
| `bsg_mux_one_hot`, `bsg_encode_one_hot`, `bsg_gray_to_binary`, `bsg_adder_one_hot` | PASS | combinational |
| `sipo_block` | **FAIL** | `loop { w0=read; tick; w1=read; tick; w2=read; tick; w3=read; out.write; tick }` (work before first tick) |
| `accum_2` (`tests/read_timing_equivalence.rs`, ignored) | **FAIL** | `loop { out.write(acc); tick; acc += step.read(); tick }` (work before first tick) |
| `sample_hold_2`, `sum_hold_2` (same file) | PASS | loop-top read only |

Decoded `sipo_block` failure (stream = nibbles 1,2,3,4,…):
- BaseJump SIPO (golden): word *i* = `data_i` on cycle *i* → block `{4,3,2,1}` presented at cycle 3.
- Copper sim: `w0`=data@0 ✓, but `w1`=data@0, `w2`=data@1, `w3`=data@2 → `{3,2,1,1}` at cycle 2.
- I.e. **every read that comes after a tick, in a loop that did work before its first tick, samples one cycle early.** Loop-top read (`w0`) is correct.

A throwaway experiment (a "defer every post-tick read to the next pre-edge" rewrite
of `synced_read`, now reverted) produced the **mirror image**: `sipo_block` and
`accum_2` PASSED, but `bsg_dff_en` FAILED (its capture came one cycle late / missed
the cycle-0 capture). So:

> **The two independent BaseJump references demand opposite timing for a read that
> comes after a tick.** `bsg_dff_en` (tick-first) needs it NOT deferred; `sipo_block`
> (work before first tick) needs it deferred. This is a fact, not interpretation —
> re-runnable by toggling the `synced_read` block condition.

## The open question (undecided — for the user to settle by hand)

What does `clk.tick().await` mean relative to the clock edge / cycle numbering?
Prior work genuinely splits on the one case that matters (a loop whose body starts
with `tick`):

- **cocotb `await RisingEdge(clk)`** (the closest analog — Copper is coroutine-based):
  the coroutine resumes *at* that edge and reacts to *its* cycle. `loop { tick; body }`
  reacts at cycle 0 — **no startup delay**. Reads after the tick sample the tick's cycle.
- **Esterel `pause`**: completes and hands control to the *next* instant, so
  `loop { pause; body }` spends instant 0 empty — a **one-cycle startup delay**. To
  model a plain DFF you'd write `loop { body; pause }`.

For any loop that does work *before* its first tick, both readings agree. They differ
only on the tick-first startup. Empirically, matching *both* `bsg_dff_en` and
`sipo_block` requires the cocotb reading (a read before the reaction's first tick
anchors cycle 0; the empty region in a tick-first loop does not). But this needs
independent confirmation from MORE hardware — see the chosen next step.

## Chosen next step (user's decision at end of session)

**Build 1–2 more independent BaseJump references that stress the startup cycle** (a
shift register, a Moore FSM, etc.) and check which reading they corroborate, so the
decision rests on more than `bsg_dff_en` + `sipo_block`. THEN decide the semantics,
THEN implement in `synced_read.rs`, THEN re-baseline/triage tests to the decided
semantics (independent hardware first; transpiler alignment is a separate later task).

## What I (the assistant) got wrong this session — do NOT trust these

I reversed myself multiple times. Concretely, treat the following as **discredited or
unverified**, not as established:
- I claimed at various points that the sim was "hardware-accurate / locked in." It is
  not settled — `sipo_block`/`accum_2` show a real mid-phase read bug.
- I proposed a "defer ALL post-edge reads" rule and called it correct/clean. It is
  **wrong** as-is: it fixes `sipo_block`/`accum_2` but breaks `bsg_dff_en`.
- I said "naive Esterel is wrong." That was imprecise/incorrect — Esterel is a
  consistent semantics; the real point is cocotb-vs-Esterel differ on the startup
  cycle (above).
- I leaned on the transpiler as a correctness oracle for `accum_2`. Don't — it is not
  independent of Copper.

The reliable anchors are: the **empirical table above** and **independent hand-written
Verilog** (BaseJump). Re-derive from those.

## Where the machinery lives

- Read timing: `copper-sim/src/synced_read.rs` (the `SyncedRead::poll` block condition
  is the knob). The macro that injects it and the per-loop `__copper_wrap` counter:
  `copper-macros/src/lib.rs` (`inject_synced_reads`, `SyncedReadRewriter`,
  `WrapCounterInjector`).
- Executor phases: `copper-sim/src/executor.rs` `tick_clock` (post-edge continuation:
  ticks resolve in the post-edge settle). `is_pre_edge()` / `PollPhase` in
  `copper-sim/src/lib.rs`.
- Reproductions: `examples/basejump/sipo_block.{rs,sv}` (uncommitted),
  `tests/read_timing_equivalence.rs` (`accum_2`, ignored), the BaseJump examples in
  `examples/basejump/`.
- Prior context: `design_docs/EXECUTOR_CONVENTION_EXPERIMENT.md` (post-edge decision),
  `SYNCHRONOUS_SEMANTICS.md`, `EXECUTION_MODEL_RECONCILIATION.md`.
