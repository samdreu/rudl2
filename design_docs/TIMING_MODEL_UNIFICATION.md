# Timing model unification — scope, after measuring

**Status:** measured; one fix landed (see §1b). No refactor undertaken — the measurement did not support one. Written 2026-08-25, starting from the claim that
Copper derives "which cycle does this statement run in" twice — once in the simulator
(by coroutine suspension) and once in `shir_lower` (structurally, by splitting the
lowered body at `clk.tick().await`) — and that every silent sim ≠ synth divergence in
the project has been a disagreement between the two.

**The measurement changed the plan.** What follows is what the corpus actually says,
and the narrower work it implies.

---

## 1. What was measured

`copper_analysis::clock_phase_count` computes the phase count from the source CFG
(Comb-connected components). `tests/timing_model_derivations.rs` compares it with the
count `shir_lower` arrives at, for every module in the corpus:

| | modules |
|---|---|
| the two derivations **agree** | **58** |
| they **disagree** | **8** |
| not lowerable (combinational, or blocked on a recorded cause) | 18 |

Every one of the 8 disagreements has the same shape: **source = N, lowered = 1**, and
every one is control-extracted. `control_extract` rewrites a body whose ticks live
inside branches or loops into a single-tick `match pc` FSM, so the phases are not
*lost* — the `pc` states are the phases — they are no longer *represented* as phases.

Two consequences follow, and they point in opposite directions from the original
framing:

* **The lowering is not wrong.** All 8 disagreeing modules pass their differential
  cases in the corpus sweep: `rom_paced`, `rom_gated`, `det_010_awaits`, `handshake`,
  `waiter`, `while_waiter`, `capture_after_wait`, `if_tick`. The emitted FSM is
  correct. The disagreement costs *visibility*, not correctness.
* **What it costs is checks.** A check downstream of extraction that counts ticks
  sees one phase where there are several. That is the class recorded five times in
  `TODO` ("a check that counts a syntactic feature rather than the thing it means,
  placed downstream of a pass that legitimately removes that feature").

So "two derivations that disagree" is too strong. The accurate statement is: **the
lowering's derivation is sound on its own terms, and the phase structure is invisible
to anything downstream of extraction.**

## 2. The work this implies, split in two

### 1a — Phase structure hidden after extraction

**Mostly already solved, by a method that is working.** Three instances of the class
have been fixed, all the same way: move the check to `copper-analysis`, where it runs
on the source and the ticks are still there — `multi_phase_out_write` (2026-08-25),
`check_memory_staging` and `memory_result_drives_plain_out` (both 2026-08-25). None
needed a change to the IR.

What remains:

* **The trailing-statement rule** (`shir_lower`, "combinational statements after the
  last `clk.tick().await`"). It is invisible to extracted modules, and — measured in
  the same audit — it is *not* the semantic rule it looks like: the linear path
  refuses a shape that the extracted path accepts and that agrees with its SV. Moving
  it to the source means first deciding what it should say, which is the open
  semantics question in `TODO` ("DECIDING where those statements belong").
* ~~**A structural guard**~~ — DONE 2026-08-25,
  `copper-codegen/tests/phase_sensitive_checks.rs`. It found two sites a hand-written
  scan had missed (one in an `impl` block, one a `Display` impl whose *message* names
  phases — excluded as text, not logic), and it names the one open instance of the
  class: `shir_lower`'s trailing-statement refusal, filed as a limitation and audited
  as a rule.

An IR phase tag — `control_extract` recording which source phase each `pc` state came
from — is the alternative, and would let checks stay in codegen. It is **not
recommended**: it adds a field that must be maintained by every future pass, to
support checks that are better off on the source anyway, where they also serve the
sim front-end. Recorded here so it is not re-proposed without a reason.

### 1b — Value visibility *within* a phase

**Two claims in this section's first draft were wrong, and the code says so.** They
are corrected here rather than deleted, because both were used to argue for a large
refactor that the evidence does not support:

* *"The lowering does not forward."* It does. `SHIRPortDrive` carries both `value`
  (unforwarded) and `edge_value` (forwarded), and `vlir_lower::split_output_reg`
  picks at the one point where a drive actually becomes a non-blocking assignment.
* *"`RegOut` is not immune to sequential forwarding"* ([TODO:1283](../TODO)). That
  was **repaired** on 2026-08-25 (causes L / L-1 / L-2). All four
  `regout_forwarding_dut` witnesses agree with their emitted SystemVerilog under 200
  cycles of random stimulus. The TODO entry was stale and is now corrected.

What is actually left is **D1's phase alignment**, and §5.1 of
`PRETICK_ALIGNMENT_GUARDRAIL.md` already contains the durable finding: the pre-tick
segment does two jobs — compute next state (wants pre-edge values) and drive Moore
outputs (wants post-edge values) — and **no single global phase choice satisfies
both**. The "always-barrier" fix was implemented, measured (22 corpus failures, all
in modules that are currently correct) and reverted.

### The standing decision (2026-08-25)

**Limit the design expressions rather than grow the simulation rules.** Specifically:

* **Keep** "a value live across a `clk.tick().await` is a register". That is the
  model, and it stays.
* **No current/next distinction.** The `d`/`q`-style split that §10.1's languages use
  to make this question unaskable is explicitly not the direction, even though it
  would dissolve the divergence.
* So where a shape diverges and no phase choice fixes it, **reject the shape**. A
  more complex executor — per-statement phases, split barriers, anything that makes
  "when does this run" cleverer — is the thing being traded away, deliberately.

The first application of this policy landed the same day: §5.5's constant-write
exemption was narrowed to unconditional writes, at a measured corpus cost of three
modules, all of them the measured divergences and none of them a correct design. That
is the shape future work here should take — a discriminator sharp enough to reject
only what actually diverges, verified against the sweep before it lands.

## 3. Why the sweep had to come first

Any move in 1b changes behaviour corpus-wide. The differential cases in
`tests/corpus_generated.rs` — 95 of them at the time of writing — are what makes that
measurable rather than hopeful: a
change to forwarding either keeps every module agreeing with its emitted SystemVerilog
or it does not, and the answer arrives in 90 seconds. That is the ordering the
standing rule for migrations asks for — build against a differential oracle, keep the
old path, no silent regressions.

## 4. What was left, and where it went

* ~~**§5.4's trailing-segment gap**~~ — CLOSED 2026-08-25. The discriminator was not
  conditionality (three such hypotheses were measured and discarded) but **how many
  clock edges the body crosses per iteration**: single-tick trailing statements share
  the head's phase, multi-tick ones do not. Corpus cost: one real module.
* ~~**The trailing-statement rule's semantics**~~ — DECIDED 2026-08-25 and
  implemented. The answer was already in `SYNCHRONOUS_SEMANTICS.md`: a clock cycle is
  a maximal tick-free region, so the trailing statements are in the head's cycle and
  lower into phase 0. Writing the differential fixture for it exposed a pre-existing
  bug both lowering paths had claimed in a comment and neither had: the trailing
  register updates must share a forwarding map with the phase they commit alongside.
  See "Trailing statements" in the semantics doc.
* ~~**A structural guard** against new tick-counting checks appearing in codegen.~~
  DONE 2026-08-25: `copper-codegen/tests/phase_sensitive_checks.rs` pins every
  function in the transpiler that both reasons about phases and can fail, each with
  the reason it is allowed to — a LIMITATION of this lowering path (legitimate) or a
  RULE about the language (which belongs on the source). A new one fails the test with
  that question. Negative-controlled.

**All three are closed, and none of them needed the refactor this document was opened
to scope.** That is the finding: the measurement said the lowering's derivation is
sound on its own terms and the phase structure is merely invisible downstream, so the
work was three small, separately-measured changes rather than one rewrite of the
lowering's spine.

What remains genuinely open in this family is **D1 itself** — §5.1's durable finding
that the pre-tick segment does two jobs and no single global phase choice satisfies
both — and the standing decision (§1b) is to keep restricting the design surface
rather than grow the executor. Every rule added since has followed that: each rejects
only shapes with a measured divergence, at a corpus cost quoted before landing.
