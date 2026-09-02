# The cycle-dataflow derivation table — phase 1 of CYCLE_DATAFLOW_SEMANTICS.md

**Status: COMPLETE (2026-08-26) — SUPERSEDED as a work list by
`PAIRED_IMPLEMENTATION_SCOPE.md`, which was scoped and executed the same day on
this table's findings (commit 7abfd98). Status markers below reflect 2026-09-01.**
This was the paper-derivation gate the model decision required: every corpus
module classified by segment anchor, with its behavior under the model derived and
compared against today — *before any executor or codegen change was scoped*.
Nothing in this file changes behavior; it is kept as the derivation record.

**The mechanical columns regenerate** — do not re-measure them by hand:

```bash
cargo run -q -p copper-codegen --bin derivation-audit
```

The bin (`copper-codegen/src/bin/derivation-audit.rs`) walks every `#[hardware]`
module in `examples/`, `tests/` and `src/` (scratch `old/` excluded) and prints,
from `Cfg::derivation_facts` in `copper-analysis/src/cfg.rs` — the same CFG
authority the timing rules read; **reporting only**, no rule keys on it — each
module's phases, first-cut anchors, plain-`Out` writes, forwarding observability,
today's guard verdicts, and transpile ground truth. Its **disposition column is a
first cut, not a derivation**: every row past `unchanged` must be hand-derived
here before it is believed. Three documented approximations on
`Cfg::derivation_facts` (path-reachability instead of value dependence;
iteration-local reach; `RegOut` writes are commits the CFG does not carry) plus
the calibration corrections recorded in §2.

**First-cut totals (2026-08-26, 185 modules, historical):** 83 unchanged · 33
unaffected (no clock) · 60 review · 9 guarded-today · 0 sv-changes. The
2026-09-01 printout is in §4b, with what its labels mean now that phase B ships.

---

## 1. Method — the window arithmetic, done once

Per `CYCLE_DATAFLOW_SEMANTICS.md` §1: a sim write executed at instant `t` is
observed for `[t, next write)`; the emitted continuous assign is a function of the
wires; the two agree iff they coincide at every observation instant in the window.
Working the corpus rows produced one refinement the model doc must absorb:

> **Anchoring is per *region*, not per Comb-component phase.** The merged
> head-phase of a single-tick loop contains two regions with different commit
> edges: the **trailing region** (last tick → head) commits at the edge that
> *opens* its cycle and executes there — it is always opening-anchored, and its
> reads are the `Immediate` class; the **pre-tick region** (head → first tick)
> commits at the edge that *closes* the cycle — it is closing-anchored iff a
> commit depends on a same-cycle input, and its reads are the `Deferred` class.
> The Comb-component merge (which the audit bin reports) is about *cycle
> membership* and is correct for phase-counting; the anchor question cuts finer.
> §5.4 of the guardrail measured that "the two regions cannot share a rule" —
> this is why.

The four legal shapes, each verified below against a real module **and** a recorded
measurement:

| shape | why it agrees | witness |
|---|---|---|
| opening-region write of committed state | executes at the observation instant with post-commit registers; assign evaluates the same values there | `counter`, `det_010`, `sync_2ff` (trailing Moore) |
| closing-region write **after** every update of what it writes | the forwarded value written at pre-edge N+1 *is* the value committing at N+1, so it equals the unforwarded `assign` from observation N+1 on | `lfsr`, V4 (§5.3) |
| write of a constant on **every** path of its region | generation-free | `branch_merge` (the agreeing twin) |
| `RegOut` / register commit | commits at the edge; execution instant unobservable | `mac_fsm` (W8/W9) |

And the illegal shape the model *derives* (finding F2 below): a closing-region
plain-`Out` write **before** the update of the register it reads — the written
value is the previous generation's, which the assign never shows at any
observation instant.

## 2. Calibration corrections made while building the audit

Recorded because each was a wrong first answer the checks caught:

1. **Memory staging is a commit.** The first cut counted only register `defs` as
   commit sites, so every RAM/ROM classified opening-anchored and landed in
   `sv-changes` — wrong; `mem.write_port().write(addr, v)` latches its operands at
   the closing edge exactly as a register does. Fixed in `derivation_facts`
   (staging nodes are commit sites); the memory family moved to closing/`review`.
2. **A condition read commits the implicit `pc`.** `det_010_awaits` first
   classified "unchanged/opening" — wrong for a hardware-anchored module whose
   reads must sample at their edge. A read that steers which suspension point
   comes next feeds the implicit state register, which the CFG carries no `def`
   for. Fixed (`control_input_read`: an `In`-reading node with ≥ 2 comb
   successors is closing evidence) — the D2 rule's condition-position exclusion,
   re-derived.
3. **The witnesses were invisible.** Scanning `examples/ + tests/fixtures/ + src/`
   found only 2 guarded modules; the D1-family demonstration witnesses live inline
   in `tests/*.rs`. The scan now covers all of `tests/` (9 guarded rows on
   2026-08-26, matching the exact-set pins in
   `copper-analysis/tests/pretick_alignment_corpus.rs`; 12 on 2026-09-01 — the
   V8 trailing clause and the phase-C probes added witnesses, and the four
   `EXPECTED_*` sets there sum to the same 12).

## 3. Findings

**F1 — the dissolve set is empty on the real corpus.** The opening-anchored
forwarded-observable shape (`H:open…[fwd]`) — the one whose emitted SV changes
under the model's forwarded emission — occurs **only** in the opt-out
demonstration witnesses (`add_then_write`, the `fast_counter` witness,
`trailing_update`). Zero non-witness modules. So the paired fix's forwarded
emission changes **no currently-passing module's SystemVerilog** (first cut on
2026-08-26; the review rows closed in §4b, and phase B's byte-diff confirmed it —
`PAIRED_IMPLEMENTATION_SCOPE.md` execution note B). The corpus was already migrated
onto the model's legal shapes by the guardrail work — the model legalizes the
witnesses with a defined meaning rather than changing live designs.

**F2 — the model derives the un-filed `program_counter` rule. MEASURED AND
CONFIRMED (m1, 2026-08-26).** The derivation, sharpened by working it against
today's actual barrier placement (the barrier parks **at the read site**, not at
the segment start): a plain-`Out` write positioned **between a leading read and
the update of the register it reads** runs in the pre-edge settle with the
previous generation's value, which the emitted `assign o = r` (Q) never shows at
any observation instant. D1 exempts the segment *because* the read comb-reaches
the update, so nothing flags it — and it has **zero corpus instances** (every
corpus module writes after the update, like `lfsr`, or moves the update to the
trailing region, like `counter`/`sync_2ff`). It is exactly the CPU sweep's
measured `program_counter` divergence #1 (TODO cause Q, filed as "worth a rule of
its own").

The controlled measurement is the **V8 battery** in
`tests/sequential_forwarding_divergence.rs`
(`v8a_write_between_leading_read_and_update_diverges_known_gap`,
`v8b_moving_the_write_after_the_update_removes_the_divergence`,
`v8c_moving_the_write_before_the_read_removes_the_divergence`; V8d and the
2026-08-27 trailing entry `v8t_trailing_stage_shape_verdict` joined later),
position of the write the only variable, every prediction confirmed on first run
**including the exact traces**:

| | shape | predicted | measured |
|---|---|---|---|
| V8a | `read; write; update` | diverge, SV leads by one | **sim `[0,1,…,12]`, SV `[1,2,…,13]`** — silent, unguarded |
| V8b | `read; update; write` | agree (lfsr / legal shape 2) | agree, `[1..13]` |
| V8c | `write; read+update` | agree (write precedes the barrier point, executes at the opening on committed state) | agree, `[1..13]` |

**Derived rule (confirmed):** in a closing-anchored region, a plain-`Out` write
placed after the leading read must come after every update of the registers it
reads (or be an every-path constant) — remedy: reorder the write, or `RegOut`.
This is the first divergence in the project's history **predicted from first
principles before its controlled measurement existed**, traces included — the
derive-first direction earning its keep.

> **Consequence for the remaining review rows:** the `close+reg+out` bucket
> *without* `[fwd]` is not automatically safe — `counter`-style rows are safe
> because their updates are trailing, but a pre-tick write sitting between the
> read and the update is V8a. Every remaining `close+reg+out` row must be checked
> for write position, not just shape family.

**F3 — ~~the anchor-level barrier repairs `probe_fsm`~~ — REVISED 2026-08-26
during the paired-implementation scoping (`PAIRED_IMPLEMENTATION_SCOPE.md` §0),
and the revision is the finding.** The proposed anchor-level (region-entry)
barrier is **falsified by V8c**: parking before a write-before-read shifts its
publication one observation late, breaking a measured-agreeing shape that is
also the V8 rule's own prescribed remedy. The read-site barrier the executor
already has is therefore *correct* under the model — it realizes the region's
opening-prefix / closing-suffix split at the barrier point. `probe_fsm`'s true
defect is a **path-dependent region boundary** (its read exists on one path
only, so the shared write executes at the opening on one path and the pre-edge
on the other — no single emission matches both, worked both ways). It stays
**retained-derived**, and D1's clause narrowed to exactly this rule in the
migration's phase D (landed 2026-08-26; `Cfg::unprotected_pretick_out_write`'s
register-reading clause now requires `leading_read_reaches`). W6's measurement
stands as the *author's* remedy (a read on
every path makes the boundary uniform), not as an executor obligation. Net
effect on the migration: **the executor needs no changes at all** — the paired
implementation is codegen-only.

## 4. Hand-derived rows (tranche 1 — sources read, measurements cited)

Verdicts: **unchanged** (model ≡ today, derivation closed) · **dissolved** (guarded
today, defined meaning under the model) · **retained** (refused today, refusal
re-derived from the model) · **fixed** (divergent/refused today, agrees under the
model's obligations).

| module | audit row | derivation | verdict |
|---|---|---|---|
| `counter` | H:close+reg+out1 | trailing update commits at the opening edge with the `Immediate` read (`x_N`); head write then reads *committed* `count` at the observation instant; `assign out = count` evaluates the same. The "close" tag is the per-region conflation (§1) — the commit is trailing, so nothing closing-anchored exists | **unchanged** |
| `up_down_counter` | H:close+reg+out1 | identical structure (trailing branch update) | **unchanged** |
| `det_010` | H:close+reg+out1 | pre-tick region: reads feed the state commit (closing, barrier, unforwarded) — no `Out` written there; trailing Moore write reads committed `state` | **unchanged** |
| `sync_2ff` | H:close+reg+out1 | trailing `ff2 = ff1; ff1 = d.read()`: both commits at the opening edge, order preserves the two stages (back-edge clause); head write reads committed `ff2` | **unchanged** |
| `lfsr` | H:close+reg+out1[fwd] | closing region, write **after** every update: forwarded value written at pre-edge ≡ committing value ≡ unforwarded `assign` from the next observation on (legal shape 2). Emission stays unforwarded | **unchanged** |
| `mac_fsm` ×2 | H:close+reg (RegOut) | all outputs `RegOut`; commits from forwarded values (the L/L-1/L-2 semantics) | **unchanged** |
| `fast_counter` (examples, corrected form) | H:open+reg+out2 | no inputs ⇒ opening; writes precede the trailing updates in execution order and read committed state; forwarded ≡ unforwarded for its writes | **unchanged** |
| `det_010_awaits` | P:open H:close | condition reads steer ticks ⇒ implicit-`pc` commits ⇒ closing sampling at each edge — the anchored behavior (`pattern_detector_010.sv`) preserved by construction | **unchanged** |
| memory family (`dual_port_ram`, `rom_from_fn`, `rom_from_contents`, `ram_*`) | H:close+reg+mem+out1[fwd] | **MEASURED (m3, 2026-08-26)** on two representatives, `tests/cycle_dataflow_memory_derivation.rs` (`m3_rom_from_fn_matches_the_derived_denotation`, `m3_dual_port_ram_matches_the_derived_denotation`): the trace derived by hand from the model (stagings = closing commits; `q = data()` = trailing commit at the producing edge; write publishes committed `q`) is matched by **both** the simulator and the transpiled SV — obs N = rom[addr_N] with no warm-up for the ROM, and ReadFirst collision / one-edge write visibility / hold-when-unstaged for the RAM. Stronger than the sweep: both implementations now anchor to the *denotation*, not merely to each other | **unchanged** (measured) |
| `add_then_write` (V1), `fast_counter` witness | H:open+reg+out1[fwd], D1 | opening region ⇒ forwarded emission `assign o = r + 1`; the sim's measured `[2,3,4,…]` **is** the model's trace (≡ Prost SV, §5.3) | **dissolved — LANDED 2026-08-26 (phase B)**: both compile clean without an opt-out and agree end to end (`pre_tick_update_forwarding_agrees_end_to_end`, `d_narrowing_battery_verdicts`) |
| `trailing_update` | H:open+reg+out1[fwd], trail | **MEASURED (m2, 2026-08-26)**: the trailing update commits at the edge that opens the trailing cycle and the write publishes the forwarded (≡ committed) value. The hand-lowered model emission matches the simulator **cycle-for-cycle** (`[0,1,1,2,2,…]`, pinned), and today's SV disagrees with both — so the sim already implements the model here and codegen is the side the paired fix changes. `m2_model_forwarded_lowering_matches_the_simulator_for_trailing_update` | **dissolved on paper; still REFUSED in the tree (2026-09-01)** — phase C is deferred, the module keeps its opt-out, and it is in `EXPECTED_TRAILING`; the m2 test's final `assert_ne` still asserts today's SV disagrees |
| `pc_arm_write`, `pc_arm_toggle`, `branch_merge_explicit` | H:close+reg+out, D1 | closing region, constant written on *some* paths ⇒ the hold path substitutes a prior-generation value ⇒ not generation-free (legal shape 3 fails) | **retained** |
| `probe_fsm` | H:close+reg+out1, D1 | F3 **as revised**: a path-dependent region boundary (read on one path only) — the shared write executes at different instants per path, so no emission matches both; W6's fix is the author's rewrite, not an executor change | **retained** (derived) |
| `pulse_plain` | P:open+out1 H:open+out1, mphase | an `Out` written in two phases = a state mux; each phase's write is an unconditional constant, but the *unwritten phases* hold ⇒ the mux needs hold arms. Legality under the model is the §8-5 open question — do not dissolve without derivation | **open** (phase E deferred; still the sole `EXPECTED_MULTI_PHASE` entry) |
| `ram_prewrite` | H:close+reg+out1[fwd], no-transpile | does not transpile; F2's shape is implicated. After phase D's narrowing D1 no longer flags it (it left `EXPECTED_FLAGGED` with a no-behavioral-verdict note and keeps its `allow_pretick_alignment` attribute); the 2026-09-01 audit lists it `unchanged` with no guard — a label, not a verdict | **open** (no behavioral measurement) |
| `uart_tx`, `uart_rx` (system.rs), `uart_rx_dut`, `rx` | 4-phase, RegOut | **MEASURED (m4, 2026-08-26).** The `(read-retime)` flag was a **fold artifact**: a folded tick-bearing loop carries interior `uses` but no `defs`, so the mid-bit sample — which inside the sub-CFG both feeds `byte_val`'s commit and steers a branch — looked commit-free from the parent. Every uart read is closing-anchored; sampling is exactly today's `Deferred` behavior; **no retiming**. The reporter now treats a folded tick-bearing node's input use as closing evidence, and the rows reclassify `unchanged`. Behaviorally pinned by `tests/cycle_dataflow_uart_derivation.rs::m4_uart_sampling_edges_match_the_derived_denotation`: every sampling edge of a frame derived on paper (start detect at edge s, mid-start check at s+4, bit k at s+12+8k, dv one-hot at s+76), asserted against sim AND SV under a stimulus where every non-sample edge carries the complement of the nearest bit — an off-by-one read on any of the ten samples corrupts the byte loudly | **unchanged** (measured) |

## 4b. The review-row pass — CLOSED 2026-08-26

The ~64 `review` rows are dispatched by the **commit-frontier taxonomy** (§4),
which the V8 rule made mechanical. A closing-phase plain-`Out` write is
derived-legal when its operands are frontier: an every-path (or held)
constant; a register with no update after the write (committed, or trailing —
the `counter` family); or a register updated *before* the write (`[fwd]` in a
closing phase — the forwarded value *is* the committing one, V8b/`lfsr`,
measured). The one hazardous position — between a leading read and the update —
is `pretick_out_write_before_update`'s exact-set territory, empty of real
modules corpus-wide. The audit bin now applies this taxonomy (plus a wire-taint
column) directly; the review bucket resolves to:

**Totals at closure (2026-08-26, 189 modules, historical): 117 unchanged
(derived) · 33 unaffected · 27 input-fed (one class, below) · 11 guarded-today ·
1 hand-derived (below) · 0 sv-changes.**

**Printout of 2026-09-01 (`target/debug/derivation-audit`, 207 modules): 125
unchanged · 33 unaffected · 31 input-fed · 12 guarded-today · 4 sv-changes · 2
review.** The two non-zero first-cut buckets are labels the bin has not been
re-taught since phase B landed, and each row is accounted for:

- **`sv-changes` (4)** — the bin's label means "opening-anchored `[fwd]` write, no
  opt-out": *the SV the model wants differs from unforwarded emission*. That was
  the pre-phase-B reading. Today: `add_then_write` and the `fast_counter` witness
  are the two modules whose forwarded emission **already ships** (phase B) and
  are pinned agreeing; `v5_trailing_read` (an opening-prefix drive, re-measured
  agreeing in `d_narrowing_battery_verdicts`, opt-out dropped in phase D) and
  `linear_trailing` (the linear multi-tick class, measured agreeing in
  `linear_trailing_probe`) are trailing-region `[fwd]` writes the linear lowering
  already commits at the right edge. None of the four is an open SV change; the
  bin's label is stale (its source, `copper-codegen/src/bin/derivation-audit.rs`,
  is unchanged since 2026-08-27).
- **`review` (2)** — both flagged `(read-retime)` by the documented `RegOut`-commit
  blind spot. `match_on_updated_reg` is hand-derived below. **`branch_on_updated_reg_bit`**
  (`tests/fixtures/branch_on_updated_reg_dut.rs`, added 2026-08-27 in commit
  1912cfc) was **never derived in this table**; it has the same shape (`1In+1RegOut`,
  `H:open+reg (read-retime)`) and is behaviorally pinned against Verilog by
  `tests/branch_on_updated_reg_equivalence.rs::a_bit_select_on_a_just_updated_register_matches_verilog`
  and its sweep case, but a derivation row is owed — not written as of 2026-09-01.

**The input-fed class** — a plain-`Out` write whose operand flows from a
same-cycle `In` read through *wires* (registers break the taint: reading one is
frontier by definition). Derived once for the class, by anchor:

- *Opening-anchored* (or the read is `Immediate` per the D2 rule): the write
  executes at the opening settle with the held input `x_N` and committed
  registers — the same values the `assign` takes at the observation instant.
- *Closing-anchored* (read `Deferred`): the write executes at pre-edge N+1 with
  `x_{N+1}` and frontier registers; its window contains observation N+1, where
  the `assign` evaluates the identical pair.

Both agree under the drive-then-clock convention. The documented caveat is the
D2 one — the windows are distinguishable only by an input changing mid-cycle,
and an `In` driven by a clocked module in-domain is stable across the window;
the passthrough sampling was adjudicated against independent hardware when D2
was fixed. The class's own behavioral evidence is the strongest in the corpus:
its two lead members, `bsg_dff_en` (the enabled-`Out` idiom) and `sipo_block`,
are **anchored against independent BaseJump hardware**, and every input-fed row
passes its differential sweep case. The per-read dual-anchor question (one read
feeding both a commit and an `Out`) remains §8 item 1 — open as a *refinement*,
with no divergent instance.

**`match_on_updated_reg`** (the last `review` row, flagged `read-retime`) — a
**second reporter blind spot**, not a retiming: a `RegOut` write is a commit
the CFG does not carry (`Node::writes` is comb-only), so the phase's only
input-dependent commit was invisible and it read as opening-anchored. Under the
model the phase is closing-anchored via that commit; the `match` scrutinee reads
the **forwarded** `s` (the source-order meaning), and both implementations
already implement exactly that — pinned by
`tests/match_hold_default_arm_equivalence.rs::a_match_scrutinee_reads_the_registers_own_update`
(sim asserted against the source-order trace, recorded for equivalence, green)
and its sweep case. **Unchanged**; no new measurement owed. The `RegOut`-commit
blind spot is documented on `Cfg::derivation_facts`; the fold handling is a code
comment inside that function, not part of its doc comment.

**With this, every corpus module present on 2026-08-26 has a derived disposition,
and every derived disposition that predicted a measurable outcome has been
measured (m1–m4, four for four on first run).** The model doc left PROPOSED the
same day, and the paired implementation was scoped and executed
(`PAIRED_IMPLEMENTATION_SCOPE.md`). Modules added since carry only the bin's
first-cut label unless a row above says otherwise (`branch_on_updated_reg_bit`
is the one that needs a row).

## 5. What remains

- ~~**The ~60 `review` rows**~~ — **CLOSED 2026-08-26, see §4b**: dispatched by
  the commit-frontier taxonomy (117 unchanged-derived, 27 input-fed with a class
  derivation, 1 hand-derived, 0 residual).
- **Measurements owed before the model doc is updated from PROPOSED:**
  (m1) ~~F2's shape as a new V-battery entry (V8)~~ — **DONE 2026-08-26,
  confirmed on first run, traces included; see F2.**
  (m2) ~~`trailing_update` under forwarded emission~~ — **DONE 2026-08-26,
  confirmed on first run**: the hand-lowered model emission (trailing increment
  committed at the edge closing the final wait state, `assign o = n`) matches the
  simulator cycle-for-cycle; today's SV disagrees with both. The sim already
  implements the model for the trailing segment — the paired fix is
  codegen-side there. Pinned as `m2_model_forwarded_lowering_matches_the_
  simulator_for_trailing_update`.
  (m3) ~~two memory representatives~~ — **DONE 2026-08-26, both first-run
  confirmations**: `tests/cycle_dataflow_memory_derivation.rs` derives each
  representative's trace from the model in the test comment, then asserts the
  simulator AND the transpiled SV both reproduce it under deterministic stimulus
  (`rom_from_fn`: obs N = rom[addr_N], no warm-up; `dual_port_ram`: ReadFirst
  same-edge collision, one-edge write visibility, hold-when-unstaged). This is
  the model-as-reference form of three-way anchoring: the sweep proves the two
  implementations agree, this file proves they agree with the *denotation*.
  (m4) ~~the uart read-retime read~~ — **DONE 2026-08-26, first-run
  confirmation**: a fold artifact, not a retiming (see the uart row above). All
  four measurements are complete, every one a first-run confirmation of the
  derivation — the model doc is ready to move off PROPOSED once the review-row
  pass closes.
- ~~**A guard for F2's shape.**~~ **LANDED 2026-08-26 (commit 7abfd98)**:
  `Cfg::pretick_out_write_before_update`, the fifth member of the pretick family
  (guardrail §5.6) — macro-enforced with the `allow_pretick_alignment` opt-out,
  unit witnesses per clause, a fourth exact-set scan
  (`write_before_update_flags_exactly_the_demonstration_modules` in
  `copper-analysis/tests/pretick_alignment_corpus.rs`), and trybuild cases
  `copper-macros/tests/ui/fail/write_before_update.rs` / `ui/pass/pretick_alignment_ok.rs`.
  A trailing clause joined it 2026-08-27 (commit b439894; `v8t_trailing_stage_shape_verdict`,
  `tests/fixtures/out_phase_dut.rs::out_from_reg_before_commit`). **It earned
  its keep on landing day**: it flagged `ui/pass/single_loop_local_ok.rs::accum` —
  a compile-only fixture nobody had measured — and the V8d measurement confirmed
  it diverges exactly like V8a (the update routed through a `let next` temp
  changes nothing). The fixture was reordered to V8c's legal form; V8d is pinned
  as the fourth battery entry. ~~Under the eventual paired fix this rule is in the
  **dissolved** bucket like D1.~~ **SUPERSEDED 2026-08-26 — see
  `PAIRED_IMPLEMENTATION_SCOPE.md` §2 Phase D:** the V8 shape is *read-preceded*
  (closing suffix), so forwarded emission does not reach it; the rule was kept
  with its derived justification (mixed generation) and remains retained-derived.
- ~~**Model-doc updates owed:**~~ **DONE.** The per-region refinement, F2 as a
  derived rule, and the trailing-region anchor are stated normatively in
  `SYNCHRONOUS_SEMANTICS.md` (rebased 2026-08-27, commit c1bf21f) and recorded as
  dated notes in `CYCLE_DATAFLOW_SEMANTICS.md` §2, §4 and §5 (2026-09-01).
