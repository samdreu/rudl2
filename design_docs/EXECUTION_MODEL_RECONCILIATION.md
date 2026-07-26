# Execution-Model Reconciliation: Simulator vs Transpiled FSM Timing

**Status:** READ timing RECONCILED (2026-07-24) — verdict: hardware is correct,
the simulator was reading one cycle early; fixed in `copper-sim`. OUTPUT timing
scoped as a distinct, still-open item (see the last section). The investigation,
evidence, and the debate that preceded the verdict are kept below.

---

## VERDICT (2026-07-24): the simulator was wrong on reads — fixed

An **independent hand-written Verilog** reference (owing nothing to Copper's
transpiler or simulator) settles it. For the `probe` design (`let x = inp.read();
tick; out.write(x); tick;`) and an independent 3-stage MAC, driven with dense
distinct inputs:

| source | samples input at cycles | e.g. `probe` values |
|---|---|---|
| Copper **simulator** (before fix) | 0, **1**, 3, 5 | 10, 11, 13, 15 |
| Copper **transpiler** (phase FSM) | 0, 2, 4, 6 | 10, 12, 14, 16 |
| **Independent hand-written Verilog** | 0, 2, 4, 6 | **10, 12, 14, 16** |

The hand-written hardware matches the transpiler; the simulator is the sole
outlier. `10,12,14,16` is a clean period-2 sampling (a phase bit toggling
`0,1,0,1`); `10,11,13,15` samples at `0,1,3,5` — a startup double-tap (spawn read
+ loop re-read) that no phase register produces. So: **the transpiled FSM is
hardware-accurate; the simulator's execution model was the optimistic one.** This
also refutes the earlier "sim is self-consistent, so probably right" lean — two
sim codings agreeing with *each other* is not evidence against independent
hardware. (`tests/probe_timing_investigation.rs`, `tests/mac_read_timing_investigation.rs`.)

The boundary is **not** tick-count: even the single-tick `probe_fsm` (a phase-gated
cross-tick read) diverged. The real boundary is "does a value cross a tick edge."

### The fix (`copper-sim/src/synced_read.rs`)

A `.read()` positioned **before any tick** in its loop iteration settles at a
pre-edge and must sample at the *registering* clock edge — the pre-edge of the
next `tick_clock`, where the transpiled FSM's input register captures. A tick
always resolves at the post-edge, so the loop re-enters (and such a read is first
re-polled) during a post-edge settle; it is now held back to the next pre-edge.
The old `same_call` guard only caught the 1-tick version of this; the new
`last_success_pre_edge` flag distinguishes a **before-tick** read (settles
pre-edge → defer) from an **after-tick** read like `counter`'s `count + step.read()`
(settles post-edge → fire immediately, must not be deferred). Result: multi-tick
loops sample inputs on the same schedule as synchronous hardware.

**Validation:** `mac_pipeline_equivalence` (3-tick pipeline, the original example)
now passes `verilator` and is un-`#[ignore]`d; `tests/read_timing_equivalence.rs`
adds `sample_hold_2` / `sum_hold_2` (2-tick loop-top reads, single and double
port); `probe_timing_investigation` asserts the sampling schedule; all
previously-green 0/1-tick tests stay green (no regressions).

**Known remaining read-timing gap (mid-phase reads).** The fix covers loop-top
reads (before any tick, any tick count) and after-tick reads in 1-tick loops
(`counter`). It does **not** yet cover a read positioned in a *middle* phase whose
result registers at a *later* tick — e.g. `out.write(acc); tick; acc = acc +
step.read(); tick;`. There the read should sample at its result's registering edge
(the second tick) but the sim samples it when the statement executes (one cycle
early). Independent hand-written Verilog confirms the transpiler is right and the
sim is one cycle early — the same verdict as the loop-top case, just an
uncovered pattern. This is *pre-existing* (the old `same_call` rule read early too),
not a regression. Repro: `accum_2` in `tests/read_timing_equivalence.rs`
(`#[ignore]`d). Fixing it needs dataflow knowledge (how many ticks until the read's
result is registered), which the per-read guard doesn't have.

---

## The problem in one line

For a sequential module whose loop body contains **2 or more `clk.tick().await`**,
the **simulator** and the **transpiled phase FSM** disagree on *which cycle a
`.read()` samples its input* — so they produce different outputs for the same
stimulus. `mac_pipeline` is the first example that exposes it: `trace: PASS` but
`verilator: FAIL`.

## Where it does and doesn't happen (the sharp boundary)

The divergence is exactly the 1-tick → 2-tick boundary:

| ticks / loop | example(s) | sim vs transpiled |
|---|---|---|
| 0 (combinational) | rotate_right, priority_encode, mux, … | ✅ agree |
| 1 | counter, lfsr, pattern_detector, traffic_light, shift_register | ✅ agree |
| 2+ | mac_pipeline (3) | ❌ diverge |

Everything currently green is 0 or 1 tick, so **no passing equivalence test is
secretly wrong** — the gap is confined to multi-tick loops.

## Why (the mechanism)

`HardwareExecutor::tick_clock` runs **pre-edge poll → `clk.advance()` →
post-edge poll**. A module's loop body runs to its first `.await` at spawn
(pre-edge); every loop *re-entry* resumes at the **post-edge poll of the previous
iteration's last tick**.

The synced-read freshness rule (`copper-sim/src/synced_read.rs`): a loop-top read
blocks **only if** (a) the loop wrapped since it last read here **and** (b) no new
`tick_clock` has happened since. It exists to stop a same-cycle double-read.

- **1 tick/loop:** re-entry happens in the *same* `tick_clock`'s post-edge poll →
  (b) holds → the read **blocks to the next `tick_clock`**. Reads land one-per-cycle,
  on the same edge the FSM samples. Aligned by construction.
- **2+ ticks/loop:** several `tick_clock`s have passed since the last read, so (b)
  is false → the read **fires immediately** at the last tick's post-edge — one
  cycle before the FSM's next phase-0 edge. Freshness never aligned a read to the
  loop's *phase period*; it only guards same-cycle re-reads.

## Minimal reproduction (2-tick probe)

```rust
#[hardware(sequential)]
async fn probe(clk: Clock<MainClk>, inp: In<Bits<8>, MainClk>, out: Out<Bits<8>, MainClk>) {
    loop {
        let x = inp.read();   // x lives across .await → register: x <= inp
        clk.tick().await;     // edge A (phase 0 → 1)
        out.write(x);
        clk.tick().await;     // edge B (phase 1 → 0)
    }
}
```

Drive `inp = 10,11,12,13,14,15,16` on cycles 0–6:

| reads input at cycles | out values |
|---|---|
| **Simulator** | 0, **1, 3, 5** → 10, 11, 13, 15 |
| **Transpiled FSM** | 0, **2, 4, 6** → 10, 12, 14, 16 |

Same output cadence and latency; only *which input got sampled* differs (agree on
the first, then the sim samples every later input one cycle earlier).

## "Is `Out` a register?" (a related, distinct question)

No — currently `Out` is **not** a register. The transpiler drives it with a
continuous `assign out = <internal register>` (`assign out = x_r;`,
`assign out = sum_r;`); the register is the variable living across `.await`. In the
simulator, `out.write(v)` writes a shared cell that **holds** between writes.

This surfaces separately in `mac_fsm` (below), which writes `out` only in one match
arm and so is **rejected as a latch** — because the transpiler treats `out` as
combinational while the sim treats a conditionally-written output as an
**implicit-hold register**. Resolving that (the *conditional/phased-output
semantics*, which also gates the Phase D P0s) is a prerequisite to the adjudication
below.

## The adjudication — which side is correct?

This flipped twice under scrutiny; recording the progression honestly:

1. **First lean — fix the FSM (pragmatic).** Sim is the reference the harness is
   built on; keep it, make the transpiler match (the "trailing-segment" hoist).
   Chosen for low blast radius, not correctness.
2. **Flip to fix the sim (first-principles).** Argued the FSM is hardware-accurate:
   a flip-flop samples at its clock edge, and the FSM samples at the phase-0 edge
   every iteration, so the sim "reads one edge early." **This argument was flawed:
   it reasoned *from the transpiled phase-FSM* — the very artifact under
   suspicion.**
3. **Flip back — sim is likely correct (independent evidence).** `pipeline_mac.rs`
   contains two codings: `mac_pipeline` (multi-tick) and **`mac_fsm` (single-tick,
   explicit `Stage` state machine)**. In simulation they produce **identical**
   output = the expected trace. `mac_fsm` is single-tick → the category that
   transpiles *correctly* → a reference-quality, human-written witness. Its
   agreement with the sim (not with the multi-tick FSM) shifts the weight of
   evidence to: **the simulator is correct, and `mac_pipeline`'s multi-tick
   transpilation is the bug** — it reads inputs one cycle *too late* (leading edge
   instead of the previous iteration's trailing edge). This is the original Option A
   (fix the transpiler's phase extraction), now on *evidence* rather than
   convenience.

## Update (2026-07-24): the discrepancy is broader than reads — it's a uniform 1-cycle offset

Fixed the conditional-output semantics (below) so `mac_fsm` transpiles, then ran it
against its own sim. Result: `trace: PASS`, `verilator: FAIL` — the transpiled
`mac_fsm`'s **output** (`out <= result` at the `Stage::Out` edge, a registered
implicit-hold output) is **one cycle later** than the simulator. That's the *same*
one-cycle gap as `mac_pipeline`'s read timing, now on the **output** side.

So the pattern is unified: **natural clocked hardware — a register that captures at
the clock edge — is consistently one cycle *behind* the simulator for multi-cycle
sequential logic**, whether the register is on the input read (mac_pipeline) or the
output write (mac_fsm). The passing cases (0–1 tick, *unconditional* output) are
exactly where no such register-vs-sim offset arises.

This complicates the earlier "sim is correct" lean: the two sim codings agree with
*each other* (self-consistent), but *both* natural-hardware realizations land one
cycle later. Self-consistency of the sim is not proof it matches hardware. The
weight now: **the simulator appears to run one cycle "ahead" of clocked hardware**
for multi-cycle logic — evidence back toward "the sim's execution model is the
optimistic one." Still not provable without an **independent hand-written Verilog
reference** for the intended behavior; that remains the only thing that settles it.

### Conditional-output semantics — decided + implemented (2026-07-24)

A conditionally-driven **output port** in a sequential module (written on some
paths/phases but not all) is now an **implicit-hold register**: its drive moves
from `always_comb` (where an undriven path is a latch) to `always_ff` as a guarded
`out <= v` (holding otherwise). Unconditional outputs stay combinational `assign`s
(the passing examples are unchanged). This matches the sim's "an output holds
between writes" *storage* semantics — though, per above, not yet its *timing*
(the register lands one cycle later; the timing is the open reconciliation). Also
unblocks the Phase D multiply-driven-output P0 (drive each write in a
phase-guarded `always_ff`). `vlir_lower::{conditional_output_ports,
split_output_regs}`.

## Current status / what would make it definitive

Not yet airtight: `mac_fsm` **does not transpile** (conditional-output latch), so
we can confirm `mac_fsm == mac_pipeline == expected` in *simulation* but can't yet
put `mac_fsm`'s *Verilog* against the expected trace. The definitive check needs a
**single-tick reference that transpiles** and produces the expected trace:

1. **Fix the conditional-output / implicit-hold semantics** so `mac_fsm` transpiles
   (higher-leverage — it's needed regardless and turns `mac_fsm` into the
   adjudicator). **← chosen next step (2026-07-24).**
2. Or hand-write a minimal single-tick MAC that drives `out` every cycle.

If the transpiled single-tick reference produces the expected trace, the verdict is
sealed: **sim correct → fix `mac_pipeline`'s multi-tick phase extraction (trailing
read).** The likely fix generalizes the single-tick freshness behavior to multi-tick
loops (block the loop-top re-read to its registering edge).

## OUTPUT timing — the remaining, distinct open item (scoped 2026-07-24)

The read fix does **not** address the output side, which is a genuinely separate
axis (and *not* a symmetric one-liner). Same verdict direction — the sim shows
outputs one cycle early vs registered hardware — but three things are tangled:

1. **Genuinely-held outputs** (`mac_fsm`: `out` written only in the Out arm) are
   correctly implicit-hold **registers** in Verilog (`out <= v` at the edge); the
   sim observes `out.write()` immediately → one cycle early. Real gap.
2. **Multi-write-around-a-tick collapse.** For `out.write(0); tick; out.write(1)`
   the single tick resolves *within one `tick_clock`* (pre→post edge), so the
   post-tick `write(1)` executes at that same `tick_clock`'s post-edge and
   overwrites `write(0)` before observation — the sim only ever sees the last
   write. (`if_tick`'s sim is all-`1`s.) The pre-tick cycle's value is lost.
3. **Transpiler over-registration.** `if_tick`'s `out_o` is registered *only*
   because the unreachable empty `default: {}` arm drops it from
   `ports_driven_all_paths` (it intersects with the empty default). It's driven on
   every *reachable* path, so a combinational `assign` would be correct — but even
   then, issue (2) still makes the sim disagree.

Why it's architectural, not a poll-guard: modules write outputs at different
points relative to ticks — `det_010` writes *post*-tick (combinational, passes),
`counter` writes *pre*-tick but unconditionally (combinational `assign`, passes),
`mac_fsm`/`if_tick` write *pre*-tick conditionally (registered). No single sim
observation point or blanket "register all outputs" rule satisfies all of them —
registering `counter`'s output would break it. A correct fix must replicate the
transpiler's conditional/phased-output classification (`vlir_lower::
{conditional_output_ports, split_output_regs}`) inside the sim so held outputs are
modeled as registered.

Until then, modules whose outputs cross a tick conditionally can't be validated by
sim-vs-Verilator. `mac_fsm_equivalence` stays `#[ignore]`d for this reason. The
increment-A control-extraction pass is instead validated **structurally**
(`tests/control_extraction_structural.rs`: extracted async Verilog == hand-written
explicit `match pc` FSM Verilog), which is immune to this axis.

## Related TODO items

- ~~Phase C: "Sim vs phase-FSM input-read timing"~~ — DONE (read fix above).
- Cross-cutting: conditional/phased **output** timing (this section; gates the
  Phase D P0s and sim-vs-Verilator for conditional-output modules).
- ~~`mac_pipeline_equivalence` `#[ignore]`d~~ — un-ignored, passing.
- `mac_fsm_equivalence` remains `#[ignore]`d pending the output-timing fix.
