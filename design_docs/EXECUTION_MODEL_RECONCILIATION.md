# Execution-Model Reconciliation: Simulator vs Transpiled FSM Timing

**Status:** open investigation (2026-07-24). Records the multi-tick read-timing
discrepancy found via the equivalence harness, the debate over which side is
correct, and the evidence — so the reasoning is not lost while it's resolved.

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

## Related TODO items

- Phase C: "Sim vs phase-FSM input-read timing" (P1) and control extraction.
- Cross-cutting: conditional/phased output semantics (gates the Phase D P0s).
- The `mac_pipeline_equivalence` test is `#[ignore]`d pending this.
