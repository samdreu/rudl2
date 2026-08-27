# Cycle-dataflow semantics — the normative timing model for the async-await surface

**Status: DERIVED, VALIDATED, AND PARTIALLY IMPLEMENTED (2026-08-26)** — phase 1
complete; the paired implementation's phases A, B and D are **landed** with user
sign-off (`PAIRED_IMPLEMENTATION_SCOPE.md` execution notes): opening-prefix
plain-`Out` drives now emit their forwarded form (the V1 family is *legal and
agreeing*, at zero live-module SV cost, byte-diff-gated), and D1 is narrowed to
the derived path-dependent-boundary rule. Phases C (extracted-path trailing
placement) and E (multi-phase mux) are deferred with recorded rationale; the
trailing rule stays. The derivation table (`DERIVATION_TABLE.md`)
covered every corpus module, and all four owed measurements (m1–m4) confirmed
their derivations on the first run — including one silent, unguarded divergence
the model predicted from first principles (V8a, since guarded as the fifth
pretick rule with user sign-off). What remains is the **paired implementation**
(§6): a semantics change to executor and codegen, which needs its own sign-off
before any scoping. Originally decided with the user as:

1. **The denotation is normative.** The per-cycle value function defined here is the
   semantics; the simulator and the transpiler are both *implementations* of it, each
   with stated obligations. A sim ≠ SV disagreement is no longer adjudicated case by
   case — the model assigns which side failed its obligation. Independent hardware
   references validate the *model* (does the denotation match real hardware?), not one
   implementation against the other.
2. **Registers stay plain locals.** No current/next split for locals (the 2026-08-25
   standing decision holds); "a value live across a tick is a register" is kept.
3. **Derive on paper before touching code.** This document re-derives every existing
   timing rule and divergence from the model, and every derivation must be checked
   against a recorded measurement (or get a new one) before any executor/codegen
   change is scoped. This partially re-opens the 2026-08-25 "restrict, don't grow"
   decision **for the D1 family specifically** — the goal is that sim and transpiler
   agree on timing and registration *by construction*, with refusal reserved for the
   genuinely unrealizable residue.

**The defect this replaces**, in one sentence: today the cycle a statement's effects
land in is decided by *where the task parks* (barrier vs tick), which is decided by an
*incidental* property (whether a leading `In` read happens to appear) — so the
simulator is not internally uniform (`PRETICK_ALIGNMENT_GUARDRAIL.md` §5.3's general
result), no single lowering can match it, and every silent divergence to date has been
a patch surface over that non-uniformity.

Companions: `SYNCHRONOUS_SEMANTICS.md` (the current reference; to be rebased on this
model once the derivation survives measurement), `PRETICK_ALIGNMENT_GUARDRAIL.md`
(the measured evidence this document derives from).

> **Phase 1 is COMPLETE: `DERIVATION_TABLE.md`** (mechanical columns regenerate via
> `cargo run -q -p copper-codegen --bin derivation-audit`). Refinements this
> document must absorb in its rebase: **anchoring is per *region* (pre-tick vs
> trailing), not per Comb-component** — the trailing region always commits at its
> opening edge and is opening-anchored, which is why §5.4's two regions could
> never share a rule; §3's anchor classification counts **memory stagings,
> `RegOut` writes, and the implicit `pc`** (a control-steering read) as commits;
> and §4 gains the measured **commit-frontier taxonomy** (constants, committed
> reads, write-after-update; the between-read-and-update position is divergent
> and guarded). Validation record: F1 — the forwarded-emission dissolve set is
> **empty on the non-witness corpus**, so the paired fix changes no live module's
> SV; F2 — the V8 battery, predicted then measured, now the fifth pretick rule;
> F3 (as REVISED by the scoping, `PAIRED_IMPLEMENTATION_SCOPE.md` §0) — the
> read-site barrier is *correct* under the model (region-entry barriers are
> falsified by V8c), `probe_fsm` is a path-dependent boundary and stays refused,
> and **the executor needs no changes**; m2 — the sim already implements the
> model for trailing segments
> (codegen is the side to change); m3 — the memory family is anchored to the
> denotation itself; m4 — the uart sampling edges match the derivation exactly,
> and both `read-retime` flags were reporter blind spots, not retimings.

---

## 1. Instants, windows, and the invariant

Fix a clock domain. **Edges** are numbered `1, 2, …`; **cycle N** is the interval
`[edge N, edge N+1)`. The **observation instant** of cycle N is just after edge N's
post-edge settle (the harness convention: drive inputs, tick, observe — so "cycle N's
input value" `x_N` is the value driven *before* edge N and stable through cycle N
until `x_{N+1}` is driven late in it).

The simulator executes statements at *instants*; hardware evaluates continuous
functions. The whole reconciliation problem is window arithmetic:

> **The write-window invariant.** A sim port write executed at instant `t` is
> observed for the window `[t, next write)`. The emitted hardware for that port is a
> continuous function `e` of registers and inputs. The two agree iff, at every
> observation instant inside the write's intended window, the written value equals
> `e` evaluated at that instant's wire values.

Every rule in the D1 family is an instance of this invariant failing; every remedy
(`RegOut`, reorder, barrier) is a way of restoring it. The model below makes the
invariant hold by *construction* for accepted programs, instead of detecting its
violations one discriminator at a time.

## 2. The denotation

A sequential module's body is a top-level `loop`. A **segment** is a maximal
tick-free region of an execution (trailing statements merged with the head — decided
2026-08-25, unchanged). The segment that *opens* at edge N denotes cycle N's
computation. Its meaning:

- **Program order with forwarding.** Statements execute in source order; a local
  read yields the last value assigned to it in this segment, else the value the
  local's register committed at the segment's opening edge. This is what plain Rust
  does, so it is what "the async fn is just Rust" already promises — and it is what
  Prost, the only other coroutine HDL, lowers (§10.5). *This clause is the meaning of
  the source; both implementations owe it.*
- **Registers = the liveness rule** (defined-in-loop ∧ live-across-a-tick, with the
  back-edge clause). Unchanged, and already shared. **The authority obligation is
  DISCHARGED (2026-08-27):** `capture_frontend_ir` fills
  `FrontendModuleIR::registers` from `infer_registers` — the identical inference,
  on the identical input, the sim macro consumes — `transpile_fir` appends the
  names the FIR→FIR passes synthesize (`pc`, counters, hoisted locals, found by
  diffing the pre-loop `let mut` set across the passes), and `chir_lower`
  **consults** that authority instead of deciding syntactically. The change was
  byte-identical across all 195 corpus modules (sv-baseline), which is the proof
  the old syntactic decider had only ever *coincided* with the inference;
  `register_reconciliation.rs` remains the end-to-end oracle, now verifying
  plumbing rather than a coincidence. (`held_temp_trap`, the historical
  disagreement this bullet used to cite, had already been repaired by the
  2026-08-26 reassigned-wire fix.) Four degenerate unit fixtures that pinned the
  rival's behavior on never-touched locals were re-blessed with genuinely-live
  registers.
- **Commits anchor to the closing edge.** The segment's final forwarded values for
  register locals and `RegOut` ports commit at edge N+1. An `In` value that feeds a
  commit is sampled at the closing edge's pre-edge settle (`x_{N+1}`) — this is the
  anchored read-timing semantics (`det_010` vs `pattern_detector_010.sv`: the FSM
  samples at its own edge) and is **not** being changed.
- **Plain `Out` drives anchor to the opening edge.** A plain `Out` written in the
  segment is cycle N's combinational drive, observed from edge N's observation
  instant onward; unwritten-on-a-path = hold (the enabled-register idiom, unchanged).

The tension the model must resolve — and the denotational restatement of §5.1's
durable finding ("the pre-tick segment does two jobs") — is that **a segment has two
temporal anchors**: its commits belong to the closing edge, its comb drives to the
opening one. The current system picks *one* execution instant per segment (decided by
the incidental barrier) and patches the observable fallout. The model instead
classifies segments and assigns obligations per anchor.

## 3. Segment anchoring — derived, not incidental

**Definition.** A segment is **closing-anchored** iff some value it *commits*
(register or `RegOut`) depends on a same-cycle `In` read — that read must sample
`x_{N+1}`, so the segment's effects can only be finalized at the pre-edge settle of
the closing tick. Otherwise the segment is **opening-anchored**: nothing in it needs
the closing edge's input values, so it can execute at the opening edge's post-edge
settle — which is exactly the observation instant.

This replaces "does a leading read appear" (syntactic, incidental) with "does a
commit *depend* on a same-cycle input" (a data-flow fact of the source, computable on
the CFG both front-ends share).

**Obligations, per anchor:**

| | executor | transpiler (plain-`Out` emission) |
|---|---|---|
| **opening-anchored** | run the segment in the opening edge's post-edge settle — **no barrier** | emit the **forwarded** expression (`assign o = r + 1` for `r = r+1; o.write(r)`) — Prost's lowering |
| **closing-anchored** | barrier — run in the closing edge's pre-edge settle | a plain-`Out` write is realizable **only if its value is expressible over the commit frontier** (see §4); emit the **unforwarded** committed-register expression |

Why each cell, from the write-window invariant:

- *Opening-anchored + forwarded*: the sim's write executes at the observation
  instant with post-commit registers and cycle-N inputs on the wires; the forwarded
  assign evaluated there uses the same values. The window is the whole cycle
  (registers next change at edge N+1, when the next iteration re-writes; inputs are
  stable per the harness convention). Exact agreement. **Checked against
  measurement**: V1's sim trace `[2,3,4,…]` equals Prost-style SV exactly (§5.3
  table) — the sim was never wrong on V1; the *unforwarded* emission was.
- *Closing-anchored + unforwarded*: the sim's write executes at pre-edge N+1 with
  value `f(x_{N+1}, forwarded locals)` — and the forwarded locals *are* the values
  committing at edge N+1. So the written value equals the **unforwarded** expression
  evaluated just after edge N+1, and the window `[pre-edge N+1, …)` contains exactly
  the observations from N+1 on. Agreement — *iff* the written value reduces to
  committed state (§4). **Checked against measurement**: this is why V4, `lfsr`,
  `det_110101`, `shift_register` all agree today under (barrier, unforwarded)
  emission, and why Prost-style emission broke exactly them (§5.3).
- The two rejected blanket fixes are the two off-diagonal cells: always-barrier
  (§5.1) forced closing-anchoring onto opening-anchored segments (broke Moore
  outputs, 22 failures); Prost-everywhere (§5.3) forced forwarded emission onto
  closing-anchored segments (broke the barrier-protected majority). The pairing is
  what neither single-sided fix could be.

## 4. The commit-frontier condition — D1's family, derived

For a **closing-anchored** segment, a plain-`Out` write's value is computed from
`(x_{N+1}, r_N)` — a *mixed-generation* pair. A continuous assign can only ever show
same-generation pairs: at observation N it shows `e(x_N, r_N)`; at N+1,
`e(x_{N+1}, r_{N+1})`. So the write is realizable in hardware **iff its value is
independent of the mixed generation** — i.e. expressible over the commit frontier:

- the write equals a value **committing at N+1** (a register the segment assigned,
  forwarded) → emit the unforwarded register: window aligns at N+1. This is V4 /
  `lfsr` / the whole barrier-protected corpus. ✔ legal, and *why* it is legal is now
  stated instead of observed.
- the write is a **constant on every path** → generation-free. This derives the
  constant-write exemption *including* its 2026-08-25 all-paths narrowing: an
  unwritten path substitutes the held value, which is a different generation's
  function, so conditional constants are mixed after all (`pc_arm_write` /
  `pc_arm_toggle` / `branch_merge_explicit` — the measured set).
- anything else (a genuine Mealy function of the closing-edge input that is not
  committed) → **no continuous assign shows that value at any observation instant**.
  Unrealizable; refuse. This is the derived, positive form of what D1 + D2's original
  divergence + `memory_result_drives_plain_out` each caught a fragment of.

And **D1 itself dissolves**: the V1 shape is an opening-anchored segment (no commit
reads an input), so under the model it *has* a defined meaning — the forwarded
assign — which the simulator already implements and codegen currently does not. The
guard exists because codegen emits the unforwarded form for a segment whose anchor
entitles the source to forwarding. Under the paired fix the shape is simply legal,
means `assign o = r + 1`, and the "sticky flag" the engineer wanted is written
`o.write(r); r = r + 1;` — both hardwares expressible, ordering picks. The
`fast_counter` adjudication is consistent with this: it established that the *English
description* maps to the registered form (§3.2's correction), not that forwarding is
wrong — under the model the registered form is the write-then-update spelling.

## 5. Re-derivation of the existing rules

Each row must end up either **dissolved** (the shape acquires a defined meaning both
sides implement), **retained-derived** (a genuine unrealizability, now a consequence
of §1–§4 rather than a discovered discriminator), or **bug-with-assigned-side**.
"Checked" = the derivation matches a recorded measurement; "predict" = needs a new
measurement before it is believed.

| rule / divergence | status under the model | evidence |
|---|---|---|
| D1 `unprotected_pretick_out_write` | **dissolved** — opening-anchored segments get forwarded emission; the sim was the correct side | checked: V1 sim ≡ Prost SV (§5.3) |
| D2 passthrough (fixed 2026-08-21) | **derived** — a segment with no commits is opening-anchored, so the read samples at the opening instant; the per-read `Immediate` fix is the partial implementation of exactly this | checked: the fix's own adjudication |
| constant-write all-paths clause | **retained-derived** — §4's generation argument reproduces the exemption and its narrowing exactly | checked: `pc_arm_*`, `branch_merge_explicit` |
| `unprotected_trailing_out_write` | **predict: dissolved** — a multi-tick trailing segment is (usually) opening-anchored at its own last edge; forwarded emission should define it. The single-tick/multi-tick discriminator (§5.4) should fall out of anchoring; must be re-derived explicitly and measured | predict |
| `multi_phase_out_write` | **retained-derived, narrowed** — an `Out` written in several segments is a mux over FSM state *iff* every writing segment individually satisfies its anchor's condition; refusal remains for mixed cases. Whether the current rule's 9 flagged modules split this way is a measurement | predict |
| `multi_write_collapse` | **retained-derived** — the pre-tick write's intended window contains observation N, but its execution instant (pre-edge N+1) is after it; no scheduling realizes the window. Same conclusion, now from window arithmetic | checked: mechanism matches the rule's own analysis |
| post-entering-edge control read (refused ordering) | **retained-derived** — the value a control decision needs at the opening instant is defined at the closing settle; unrealizable by any implementation | checked: the two-window measurement (2026-08-24) |
| RegOut forwarding (L/L-1/L-2, fixed) | **derived** — commits are computed from forwarded values (§2); the repair implemented the model's clause | checked: `regout_forwarding_equivalence.rs` |
| trailing forwarding map (2026-08-25 fix) | **derived** — same clause at the trailing commit | checked: the differential case that found it |
| `program_counter` divergence #1 (CPU sweep) | **assigned** — a leading read exempts D1 on *mechanism* grounds, but the write is not commit-frontier-expressible in a closing-anchored segment → per §4 it should be refused (or becomes legal if the segment is in fact opening-anchored and emission is fixed). Re-derive with the actual module | predict |
| `program_counter` divergence #2 (trailing `RegOut`, single-tick) | **assigned** — the model says commits land at the segment's closing edge, which for a trailing statement is the edge that *opens* its cycle (`SYNCHRONOUS_SEMANTICS.md` trailing rules); one side folds it into the wrong edge. Determine which against the model | predict |
| memory staging rules (4) | **retained-derived** — bus-per-cycle and observe-after-edge are window statements about the address/data nets; expected to restate cleanly | predict (paperwork, not risk) |
| reachability, CDC, poll-order, comb-loop rules | **untouched** — orthogonal to segment anchoring | — |

## 6. What the paired implementation would be (not yet scoped)

Recorded so the shape is visible; **no code moves until §5's predict rows are
measured** and the per-module impact table (§7) exists.

- `copper-analysis`: `segment_anchor(cfg) -> {Opening, Closing}` per segment, from
  commit-input data dependence. Both front-ends consume it — the same c2 discipline
  as every other shared fact.
- executor / macro: barrier injection keyed on the anchor, not on leading-read
  presence. Opening-anchored segments run barrier-free by definition.
- codegen: plain-`Out` emission keyed on the anchor — forwarded expression for
  opening-anchored segments, unforwarded commit-frontier expression for
  closing-anchored ones (`SHIRPortDrive` already carries both `value` and
  `edge_value`; the selection point exists).
- the D1-family guards convert per §5: dissolved rules are deleted *with* their
  fixtures becoming positive differential cases; retained rules are restated as
  consequences (same detection sites, derived justification); the
  `allow_pretick_alignment` opt-out shrinks accordingly.

## 7. Migration discipline (standing rules apply verbatim)

- **Derivation table first.** Phase 1 is a per-module table over the whole corpus:
  each module's segments classified by anchor, predicted trace under the model,
  compared with today's trace — *predicted on paper, then measured*. Every module
  whose trace changes is listed with its rewrite (usually an ordering change) before
  any implementation lands. `fast_counter`-shaped modules are the known class.
- **The sweep is the oracle.** The differential corpus (G-D) plus the frozen goldens
  plus the independent anchors (BaseJump, `pattern_detector_010.sv`, the CDC
  reference) are the gate. Anchors get **re-verified, never re-blessed** — under a
  normative denotation they now test the model itself.
- **Old path kept.** The anchor-keyed behavior goes behind a mode
  (`COPPER_SEMANTICS=…` in the spirit of `COPPER_SCHEDULER`), with a lockstep
  differential test against the current behavior for the modules the model predicts
  are unchanged — which the derivation table says is the large majority.

## 8. Known open questions for the derivation phase

1. The anchor definition's edge cases: a segment where an input feeds a commit on
   one path only; an input read whose result feeds *both* a commit and a plain `Out`
   (two sample instants for one binding — refuse, or split the read?). The V-battery
   method applies: construct the minimal pair, measure both sides.
2. Trailing segments: re-derive §5.4's single-vs-multi-tick discriminator from
   anchoring; if it does not fall out, the model is missing a clause — that is a
   finding, not a footnote.
3. Startup: an `Out` first written in a late segment vs the continuous assign's
   time-0 value (the documented startup discrepancy). Under a normative model this
   needs an initial-value rule, not a caveat.
4. Memory read results interact with anchoring (`is_ready`/`data` are
   edge-produced): restate the four staging rules in window terms.
5. Whether `multi_phase_out_write`'s current flag set splits into legal-mux /
   refused-mixed as §5 predicts.
