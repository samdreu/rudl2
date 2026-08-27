# The paired implementation — scope

**Status: EXECUTED 2026-08-26 (phases A, B, D landed with user sign-off; C and E
deferred — see below).** Execution notes, in the order things were learned:

* **A — DONE.** `sv-baseline` (snapshot/diff over all 189 corpus modules) landed
  and immediately found the transpiler emitting **non-deterministically**
  (`sipo_block`'s promoted-register declaration order came from a `HashSet`
  iteration; fixed with a sort in `find_promoted_wires`). Baseline pinned.
* **B — DONE, smaller than scoped.** No `region_anchor` plumbing was needed:
  `edge_value` already *is* the per-drive forwarded form, so the change is a
  per-drive selection in `shir_lower` (an opening-prefix drive — no `In` read
  precedes it in its segment walk — uses `edge_value` as its continuous-assign
  form; trailing segments excluded, their committed unforwarded form being m2's
  target). Byte-diff: **exactly `add_then_write` + the `fast_counter` witness, 0
  live modules** (F1 confirmed). Both flips re-blessed per their messages; the
  `ref_fast_counter` adjudication re-pointed at the corrected spelling with the
  R7 rationale recorded in the test. No transition flag was needed — the
  snapshot/diff gives the same comparison without doubling the test matrix.
* **C — DEFERRED, with corrected scoping.** The scope's "localized to
  `shir_lower`" prediction was wrong: the **linear** path already commits
  trailing updates at the opening edge (the 2026-08-25 shared-map work); the
  residual one-state-late placement lives in **`control_extract`**, where
  trailing statements arrive through the break-inlining machinery — deep
  surgery. And `RegOut` absorbs the placement difference (why uart's trailing
  write agrees), so the only affected class is plain-`Out`/register trailing —
  which `unprotected_trailing_out_write` keeps unwritable. Cost/benefit: the
  code change would only legalize the `trailing_update` witness. Deferred like
  E; the trailing rule is therefore **kept**, not retired.
* **D — DONE, evidence-first.** The V-battery was re-measured under B before the
  rule moved (`d_narrowing_battery_verdicts`): V5 **and V7** dissolved (V7's
  2026-08-21 verdict was stale even before B — the 2026-08-25 trailing
  forwarding had already repaired it), W4 diverges. D1 narrowed to the
  **read-preceded** / path-dependent-boundary rule; exact-set pin updated both
  directions (W4 in; `add_then_write`, `fast_counter`, `ram_prewrite` out — the
  last with a no-behavioral-verdict note); dissolved witnesses dropped their
  opt-outs and compile clean; the ui/fail V1 case became a ui/pass case with W4
  taking its place; the macro diagnostic now describes the path-dependent
  boundary.

Original scope follows, kept for the record.

---

Original status: SCOPE ONLY. User-requested scoping of the migration that
realizes `CYCLE_DATAFLOW_SEMANTICS.md` (DERIVED AND VALIDATED, phase 1
complete). Implementation starts only after the decision points in §6 are ruled
on — each phase changes semantics-bearing code and falls under the standing
consult-first constraint.

---

## 0. The scoping finding: the "paired" implementation is codegen-only

Scoping the executor side against the V8 measurements **falsified F3's proposed
change** (`DERIVATION_TABLE.md`, corrected there too). The proposal was an
anchor-level barrier — park a closing-anchored region at its *entry* instead of
at the read site. Worked against V8c (`o.write(r); r = r + step.read(); tick`,
measured AGREEING, and the remedy the V8 rule's own diagnostic prescribes):

* today: the write executes at the **opening** (before the read's barrier),
  publishing committed `r_N` for observation N — which `assign o = r` shows
  there. Agree.
* region-entry barrier: the write is dragged to pre-edge N+1, so `r_N` is
  published for observation **N+1**, where the assign shows `r_{N+1}`. Diverge.

So the region-entry barrier breaks a measured-agreeing shape that the guard
family explicitly sanctions. The correct reading is that **the executor already
implements the model**: the read-site barrier *is* the region boundary — an
opening prefix (executes at the opening instant, on committed state) and a
closing suffix (executes at the pre-edge, on closing samples and forwarded
values). Every phase-1 measurement is consistent with this (V8a/b/c, m2's
"the sim already implements the model for trailing segments", m3, m4).

`probe_fsm` (F3's subject) is thereby re-derived, not repaired: its read exists
on only **one path**, so the region boundary — and with it the execution instant
of the shared `o.write(r)` — is *path-dependent*. No single emission can match a
write that runs at the opening on one path and the pre-edge on the other
(worked through both candidate emissions; each matches exactly one path). The
shape stays refused; what changes is its justification — D1's clause becomes the
derived **path-dependent-boundary rule** (§4, phase D). W6's measurement stands
as the *author's* remedy (a read on every path makes the boundary uniform), not
as an executor obligation.

**Consequence: the executor is not touched.** The migration is two codegen
changes, each with a measured target lowering already in the tree, plus rule and
fixture housekeeping.

## 1. Target emission, by region anchor

The anchor is a source-level fact (c2 discipline: computed in `copper-analysis`,
consumed by codegen). The selection infrastructure exists: `SHIRPortDrive`
carries `value` (unforwarded) **and** `edge_value` (forwarded), and
`vlir_lower::split_output_regs` is documented as "the one registration
decision".

| region | plain-`Out` emission | today | change |
|---|---|---|---|
| closing suffix (a commit depends on a same-cycle `In` read; statements after the barrier) | **unforwarded** commit-frontier expression | unforwarded | none |
| opening region (pre-tick with no input-dependent commit — e.g. no `In` reads at all) | **forwarded** expression (`assign o = r + 1` for V1) | unforwarded | **phase B** |
| opening prefix (statements before the first leading read) | committed-state expression | unforwarded (≡ committed here) | none |
| trailing region (multi-tick) | commit trailing updates at the edge **opening** the trailing cycle; assign the committed register | commits one edge late | **phase C** |

The phase-B target is §5.3's Prost-style measurement (matches V1's sim exactly);
the phase-C target is m2's hand-lowered module (matches the sim cycle-for-cycle,
pinned).

## 2. Phase plan

### Phase A — oracle prep (no behavior change)

* **A1 — corpus SV byte-diff harness.** A `tools/`-resident script (or bin) that
  transpiles every corpus module and diffs the emitted SV against a committed
  baseline. F1's prediction — *zero live modules change* — becomes an asserted
  gate instead of an observation. Gate: zero diffs on unchanged codegen.
* **A2 — flip inventory.** The tests that pin today's divergences carry their
  own re-bless instructions in their failure messages; enumerate them so each
  phase's expected flips are declared before the phase lands:
  `pre_tick_update_is_forwarded_in_sim_but_not_in_hardware_known_gap` and the
  `fast_counter` adjudication tests (phase B);
  `d1_in_the_trailing_segment_is_an_unguarded_gap` and
  `m2_model_forwarded_lowering_matches_the_simulator_for_trailing_update`'s
  final `assert_ne` (phase C). Anything that flips outside the declared set
  fails the phase.

### Phase B — forwarded emission for opening regions

* `copper-analysis`: `pub fn region_anchor` (the §1 classification, reusing
  `leading_read_reaches` / the commit set; unit-tested against the V8 battery
  and the phase-1 rows).
* `copper-codegen`: key the `value` / `edge_value` selection on the anchor at
  the `split_output_regs` decision (both lowering paths — linear and
  control-extracted; `phase_sensitive_checks.rs` polices where the fact is
  computed).
* **Predicted corpus delta: zero live modules** (F1; gate A1 asserts it). SV
  changes only for `add_then_write` and the `fast_counter` witness, whose
  divergence pins then flip to equivalence tests and whose
  `allow_pretick_alignment` opt-outs come off.
* **The one anchor to re-point, not re-bless (R7):** §3.2's independent
  `ref_fast_counter` agrees with *today's* emission for the update-then-write
  spelling. Under the model that spelling legitimately means the forwarded
  trace; the English "counter with sticky flag" maps to the write-then-update
  spelling, which `fast_counter_corrected` already is and which already matches
  the reference. The adjudication test moves to the corrected spelling with a
  recorded rationale; the reference itself does not change.
* **Known risk to gate, not argue:** a *conditional* write in an opening region
  becomes an enabled register over the forwarded operand. Derived-agreeing;
  confirmed by the sweep gate before the phase lands.

### Phase C — trailing lowering (m2's target)

* `copper-codegen` (`shir_lower`, both paths): trailing register updates fold
  into the edge that opens the trailing cycle; the port assign reads the
  committed register — byte-for-byte the m2 hand lowering that matches the sim.
* **Predicted corpus delta: zero live modules** (the shape is guarded today;
  the only real module ever in it, `rv32i_cpu_pipelined`'s `program_counter`,
  was migrated to `RegOut` and *may* migrate back afterward as a follow-up).
* Flips: the two phase-C tests in A2; `trailing_update` loses its opt-out and
  becomes an equivalence fixture.

### Phase D — rule and fixture housekeeping

* `unprotected_trailing_out_write`: **retire** (its shape now agrees; the m2
  pin becomes the regression test).
* `unprotected_pretick_out_write` (D1): **narrow to the derived
  path-dependent-boundary rule** — flag only a region whose first leading read
  is not on every path to a commit it protects (`probe_fsm`, W4). V1-shapes are
  dissolved and drop out. Exact-set pins updated in the same commit, both
  directions.
* Unchanged rules, now with derived justifications recorded on them:
  `pretick_out_write_before_update` (V8; mixed generation), the constant-write
  narrowing (`pc_arm_*` — closing regions keep unforwarded emission, so the
  hold-path shift is still real), `multi_write_collapse` (window arithmetic),
  the post-entering-edge control read (value unavailable at decision instant),
  `multi_phase_out_write` (except as phase E revisits it).
* Docs: rebase `SYNCHRONOUS_SEMANTICS.md` §Output timing on the model;
  retire the guardrail doc to a status note per its own §4c plan.

### Phase E — OPTIONAL, gated separately: multi-phase mux (`pulse_plain`)

The §8-5 open question. Requires its own paper derivation plus an m2-style
hand-lowered measurement *before* any scoping — a state-mux with hold arms is
plausible and unproven. Not part of this migration unless separately approved.

## 3. Gates (every phase)

1. `tools/regression.sh` bare — `REGRESSION OK`, nothing less.
2. The A1 byte-diff — changes exactly the declared module set, nothing else.
3. The A2 flip inventory — exactly the declared tests flip, re-blessed in the
   same commit with their messages' instructions followed.
4. Independent anchors (BaseJump, `pattern_detector_010.sv`, the CDC reference,
   `ref_fast_counter` per its phase-B note) — re-run and green **for the right
   reason**; any re-pointing carries a recorded rationale, never a silent
   re-bless.
5. The three derivation-pin files (`sequential_forwarding_divergence`,
   `cycle_dataflow_memory_derivation`, `cycle_dataflow_uart_derivation`) —
   the denotation anchors must stay green throughout, since the model is what
   the migration implements.

## 4. What this migration does NOT do

* **No executor changes** (§0). The scheduler, barrier machinery, phases, and
  memory model are untouched; poll-order and levelized invariants are not at
  risk.
* **No current/next split for locals** — the standing decision holds.
* **No memory changes** — m3 anchored the family to the denotation as-is.
* `probe_fsm`, `pc_arm_*`, `branch_merge_explicit`, V8a/V8d stay refused, each
  with its derived justification.

## 5. Effort shape

Phase A is small (a script plus an inventory). Phase B is the substantive one:
one analysis function, one selection-point change, two lowering paths, and the
fast_counter anchor re-point. Phase C is localized to the trailing handling in
`shir_lower` with a pinned target. Phase D is wide but mechanical (pins, ui
tests, opt-outs, docs). B and C are independently landable, each behind gates
1–5; B-then-C is the natural order because C's flips are a superset check on
B's machinery.

## 6. Decision points before implementation starts

1. **Go/no-go per phase** — B and C each change emitted SV for the witness
   shapes; D retires/narrows two shipped rules.
2. **Transition flag or not.** Recommendation: a short-lived `EmitConfig` field
   (legacy vs cycle-dataflow emission) during B/C development so the byte-diff
   can compare both from one build, **removed in phase D** — not a permanent
   mode (a permanent flag doubles the test matrix for zero corpus benefit given
   F1).
3. **`program_counter` back-migration** (RegOut → plain `Out` after C) — a
   design-preference call on the CPU example, not a correctness one.
4. **Phase E inclusion** — recommendation: defer; derive and measure first.
