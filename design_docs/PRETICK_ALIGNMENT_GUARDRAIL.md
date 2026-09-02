# Pre-Tick Segment Alignment — Divergence Analysis and Guardrail Plan

> **Status (verified against the tree 2026-09-01): the family is GUARDED — four
> rule functions (one of them carrying a constant-write clause), four exact-set
> corpus pins; D2 fixed outright.** `pretick_out_write_before_update` (V8) joined
> on 2026-08-26 and gained its trailing clause on 2026-08-27. Phases 0–3 complete,
> gates G0–G3 met. D1 is a compile error (the language is restricted); D2 was
> adjudicated against independent hardware and fixed in the simulator. Three
> candidate fixes were tried and rejected with measured evidence before the first
> shippable rule was found (§5.1–5.3), and two further widenings of D1 were
> measured and rejected (§5.4) before the discriminator that works was located.
> This doc is both the plan and the record of what was ruled out and why. **Sections
> §1–§4 and §7–§9 are the 2026-08-21 record and are annotated, not rewritten**;
> §5.4–§5.7 and the tables in this header describe the rules as they stand.
>
> **NARROWED 2026-08-26 (cycle-dataflow phase D, §5.7):** phase B's forwarded
> emission gave the no-read opening shapes their defined meaning (V1, V5, V7 and
> both `fast_counter` ports re-measured AGREEING — V7's 2026-08-21 verdict was
> stale even before phase B), so D1's register-reading clause now additionally
> requires the write to be **read-preceded** — the path-dependent-boundary class
> (W4, `probe_fsm`). The hold/conditional-constant clause is unchanged
> (`pc_arm_*` still diverge). The former ui/fail V1 case is a ui/pass case now.
>
> **The rules.** All four live in `copper-analysis/src/cfg.rs` as methods on `Cfg`
> (re-exported as free functions from `copper_analysis`). The corpus-cost column is
> **history**: each figure was measured once, at the named commit, and is not
> reproducible from the tree or checked by any test — the exact-set pins below are
> what is enforced today.
>
> | rule | what it refuses | corpus cost when it landed (measured once, not a gate) |
> |---|---|---|
> | `unprotected_pretick_out_write` (D1, narrowed §5.7) | a plain `Out` written where an `In` read reaches the write on some path while a register is assigned unprotected on another (path-dependent boundary), or a conditional/constant write in such a segment | — |
> | …its **constant-write** clause (§5.5; `written_on_all_paths`) | the same, where the port is written on *some* paths only — a constant is idempotent across the phase shift only if it lands on every path | 3 modules, all measured divergences (commit `348ddd0`) |
> | `unprotected_trailing_out_write` (§5.4; narrowed 2026-08-27) | the same hazard past the *last* tick, gated on `crosses_more_than_one_tick` **and** `has_nested_tick` (the source-level mirror of extraction's trigger) — the LINEAR class is exempt, measured agreeing; the extracted class is measured one-edge-late wherever the last tick sits | 1 real module (`rv32i_cpu_pipelined`'s `program_counter`, → `RegOut`; commit `56233ce`) |
> | `multi_phase_out_write` | a plain `Out` driven in more than one clock phase | 9 modules, six of them its own witnesses, three real (`rv32i_cpu` — since replaced by `rv32i_cpu_transpilable` — and `uart_tx`/`uart_rx`, → `RegOut`; commit `99290da`) |
> | `pretick_out_write_before_update` (§5.6, 2026-08-26; trailing clause 2026-08-27) | a plain `Out` written before the update of a register it reads — behind a leading read in the pre-tick segment, or anywhere in the trailing segment (whose updates commit at the opening edge) | 0 real modules at landing (caught a compile-only UI fixture; commit `7abfd98`); the trailing clause then caught THREE real sim-only-tested modules (`tests/module_composition_hybrid.rs`'s stages, measured divergent as `v8t_stage_publish_then_load`, migrated to the canonical write/read/tick/update spelling; commit `b439894`) |
>
> **Where each is enforced, and the opt-out.** Every rule is a spanned compile
> error in the `#[hardware(sequential)]` arm of `copper-macros/src/lib.rs`, and
> every one is silenced — *detection unchanged* — by
> `#[hardware(sequential, allow_pretick_alignment)]` (`ALLOW_PRETICK` in the
> macro). `multi_phase_out_write` is additionally enforced by the transpiler in
> `copper-codegen/src/lib.rs::transpile_target`, with the same opt-out read by
> `opts_out_of_pretick_alignment`; the other three are macro-only (a module that
> reaches the transpiler has already compiled through the macro). The opt-out is
> for modules that exist to *demonstrate* a divergence and must never be reached
> for in a real design.
>
> **The pins.** Each rule has three layers, all green as of 2026-09-01:
>
> | rule | unit tests (`cfg.rs` `mod tests`, positive / negative) | trybuild (`copper-macros/tests/ui/`) | exact-set corpus pin (`copper-analysis/tests/pretick_alignment_corpus.rs`) |
> |---|---|---|---|
> | `unprotected_pretick_out_write` | `hazard_w4_mixed_alignment_flagged`, `hazard_probe_fsm_flagged` / `hazard_v4_leading_read_not_flagged`, `hazard_v6_post_tick_assign_not_flagged`, `hazard_w8_regout_immunises_mixed_alignment`, `hazard_w9_regout_immunises_the_minimal_case`, `hazard_lfsr_shape_not_flagged`, `hazard_regout_only_module_not_flagged`, and the three DISSOLVED shapes `hazard_v1_assign_then_write_dissolved`, `hazard_v5_trailing_read_dissolved`, `hazard_v7_escape_across_tick_dissolved` | `fail/pretick_alignment.rs` (`mixed_alignment`), `pass/pretick_alignment_ok.rs` (`post_tick_update`, `registered_output`, `leading_read`, `constant_write`, `forwarded_opening_drive`) | `pretick_alignment_hazard_flags_exactly_the_known_divergent_modules`; `EXPECTED_FLAGGED` = {`tests/fixtures/probe_timing_dut.rs::probe_fsm`, `tests/sequential_forwarding_divergence.rs::w4_mixed_alignment`, `tests/fixtures/control_extraction_dut.rs::branch_merge_explicit`, `…::pc_arm_toggle`, `…::pc_arm_write`} |
> | …constant-write clause | `a_conditional_constant_write_is_flagged` / `an_unconditional_constant_write_is_not_flagged` | `pass/pretick_alignment_ok.rs::constant_write` (no dedicated `fail/` case; the corpus pin carries the positives) | the last three entries of `EXPECTED_FLAGGED` above |
> | `unprotected_trailing_out_write` | `trailing_folded_loop_flagged`, `trailing_branch_nested_tick_flagged` / `trailing_linear_class_exempt` | **none** — no `ui/fail` or `ui/pass` case exercises this rule through the macro | `trailing_out_write_flags_exactly_the_demonstration_modules`; `EXPECTED_TRAILING` = {`tests/sequential_forwarding_divergence.rs::trailing_update`, `…::branch_trailing`} |
> | `multi_phase_out_write` | **none in `cfg.rs`**; the positive/negative pair is `tests/sequential_forwarding_divergence.rs::the_rule_flags_the_plain_form_and_not_the_registered_one` (`pulse_plain` flagged, `pulse_registered` clean) | **none** | `multi_phase_out_write_flags_exactly_the_demonstration_modules`; `EXPECTED_MULTI_PHASE` = {`tests/sequential_forwarding_divergence.rs::pulse_plain`} |
> | `pretick_out_write_before_update` | `v8a_write_between_read_and_update_flagged`, `v8t_trailing_write_before_update_flagged` / `v8b_write_after_update_not_flagged`, `v8c_write_before_read_not_flagged`, `v8_constant_write_not_flagged`, `v8_regout_not_flagged`, `v8_unrelated_update_after_write_not_flagged`, `v8t_trailing_write_after_update_not_flagged` | `fail/write_before_update.rs` (`stale_publish`), `pass/pretick_alignment_ok.rs` (`v8b_publish_after_update`, `v8c_publish_before_read`), `pass/single_loop_local_ok.rs::accum` (reordered to V8c) | `write_before_update_flags_exactly_the_demonstration_modules`; `EXPECTED_WRITE_BEFORE_UPDATE` = {`tests/sequential_forwarding_divergence.rs::v8a_read_write_update`, `…::v8d_temp_renamed_update`, `tests/fixtures/out_phase_dut.rs::out_from_reg_before_commit`, `…::v8t_stage_publish_then_load`} |
>
> The four corpus pins scan every clocked (`sequential` / `synchronizer`) module
> under `examples/`, `src/` and `tests/` and print the scanned count
> (`cargo test -p copper-analysis --test pretick_alignment_corpus -- --nocapture`);
> that printout, not any figure in this document, is the current corpus size. Every
> pinned module carries `allow_pretick_alignment`, so the pins are also the proof
> that the opt-out silences the error and not the detection.
>
> **Measured, not argued:** `tests/sequential_forwarding_divergence.rs` (19
> `#[test]`s, none ignored, all sim-vs-Verilator on the transpiled SV; each records
> *today's* verdict and flips loudly when a divergence is fixed or a dissolution
> regresses).
>
> **Read first:** `SYNCHRONOUS_SEMANTICS.md` §Output timing. This is the **third**
> member of the blocking/non-blocking family, after the `Out`-hold semantics and the
> multi-write-around-a-tick collapse.

---

## 1. The defect

### D1 — a pre-tick register assignment is not phase-pinned

```rust
loop { r = r + Bits::from_lit::<1>(); o.write(r); clk.tick().await; }
```

| | trace |
|---|---|
| Copper simulator | `[2, 3, 4, 5, 6, 7]` |
| transpiled SystemVerilog | `[1, 2, 3, 4, 5, 6]` |

Silent — no error, no warning, no lint.

**Mechanism.** The simulator's *sequential forwarding* makes the assignment visible
to the write that follows it in the same segment. Codegen emits `r <= r + 1`
(non-blocking) with `assign o = r`, and a flip-flop's Q cannot reflect its own D.

More precisely, the difference is **where the task parks**, not how often the body
runs. With no barrier the task parks *at the tick*, so the pre-tick body for cycle
N+1 executes during cycle N's **post-edge settle** — and the post-edge observation
of cycle N therefore sees cycle N+1's value. With a barrier the task parks *at the
barrier*, so that body executes in the **pre-edge settle** of cycle N+1 instead,
which is what the synthesized `always_ff` does.

**What installs the barrier.** A leading `In` read classifies `Deferred`
(impl-plan item 3) and injects `pre_edge_barrier()`. A module with no `In` params
at all gets none — `inject_synced_reads` returns early on `in_params.is_empty()`.

> **The real defect underneath:** a module's pre-tick phase alignment is decided by
> an **incidental** property — whether it happens to read an input before its tick —
> rather than by anything the designer expressed.

### D2 — a combinational passthrough of a post-edge-produced signal — **FIXED 2026-08-21**

> Adjudicated against independent hand-written Verilog (a clocked producer feeding a
> passthrough gives `mid == out`; the simulator gave `1/0 2/1 3/2 …`), then fixed in
> `classify_reads`: a read feeding a combinational `Out` in a segment that assigns no
> register is `Immediate`. Corpus cost: **666/667**, the one failure being the test
> that pinned the old behaviour (measured once at commit `d640526`, 2026-08-21 —
> `git log -S"666/667"` — not a regression gate; the fix is pinned today by
> `tests/sequential_forwarding_divergence.rs::d2_is_fixed_and_d1_still_demonstrates_the_hazard`
> and the unit test `trailing_reads_are_immediate` in `cfg.rs`). A first attempt keyed on the *module* ("no registers
> anywhere") broke three passing tests including a hardware-anchored one, because
> `det_010_awaits` and `if_tick` have no data registers yet read inside control flow
> whose *tick count* depends on the sampled value; the narrowed per-read rule excludes
> condition positions.

```rust
loop { out.write(inp.read()); clk.tick().await; }   // → `assign out = inp;`
```

Transpiles to zero cycles. In the simulator the leading read is `Deferred` so it
samples at the **pre**-edge, while a clocked producer updates at the **post**-edge:
the passthrough lags one cycle. Standalone the two agree (a testbench drives the
input before the edge, which coincides with the pre-edge sample) — the divergence
needs a *clocked producer*, which is why no existing test caught it.

### Relationship to `multi_write_collapse`

Same family, **complementary triggers** — neither rule subsumes the other. Checked
by hand on 2026-08-21: `multi_write_collapse` returns `[]` for both the D1 minimal
case and `fast_counter`, while returning `["o"]` for the shape it targets. **No
unit test pins that `[]` result** — `copper-analysis/tests/analysis_semantics.rs`'s
`multi_write_*` tests cover the collapse rule's own shapes only. (Since the
2026-08-26 narrowing, §5.7, the D1 minimal case is a legal shape anyway.)

| | `multi_write_collapse` | D1 |
|---|---|---|
| leading `In` read | **required** (shifts the pre-tick write into the pre-edge) | **must be absent** (no barrier ⇒ body runs in the post-edge settle) |
| symptom | pre-tick value never observed | update observed one cycle early |

The leading read is load-bearing in both — once as the cause, once as the cure.

---

## 2. Why this matters

- **It breaks the project's correctness bar.** "Sim and synth agree" is the bar; here
  they silently disagree on a five-line module.
- **It has already corrupted an anchor.** `tests/two_domain_hierarchy_cdc.rs` — the
  independent-hardware anchor for the whole dual-clock design — is green *because D1
  and D2 cancel*: the flag asserts one cycle early, the consumer one cycle late, and
  the observable boundary lands on the reference's cycle. Verified by experiment
  (correcting only the counter moves the boundary 5 → 6). A green anchor that agrees
  for the wrong reason is worse than a missing one.
- **It qualifies a paper claim.** Contribution 5 frames Copper as having *found* the
  blocking/non-blocking distinction and restricted the synthesizable subset so every
  accepted program preserves `sim ≡ synth`. D1 is an accepted program that does not.

---

## 3. What is established (measured, not argued)

### 3.1 The variant map

Each row is one module run in the simulator and, independently, under Verilator on
its own transpiled SV.

| # | shape | verdict |
|---|---|---|
| V1 | `r = r+1; o.write(r); tick;` | **DIVERGE** |
| V2 | `r = r+1; r = r+1; o.write(r); tick;` | **DIVERGE** |
| V3 | `r = r+1; s = r; o.write(s); tick;` | **DIVERGE** |
| V4 | `if en.read() {r = r+1;} o.write(r); tick;` | agree |
| V5 | `r = r+1; o.write(r); en.read(); tick;` | **DIVERGE** |
| V6 | `o.write(r); tick; r = r+1;` | agree |
| V7 | `r = r+1; o.write(s); tick; s = r;` | **DIVERGE** |

> **Superseded for V1/V2/V3/V5/V7 (2026-08-26, §5.7).** These verdicts are the
> 2026-08-21 measurement against the *unforwarded* `assign o = r` lowering. Under
> phase B's forwarded opening-prefix emission all five **agree** — re-measured in
> `tests/sequential_forwarding_divergence.rs::d_narrowing_battery_verdicts` and
> `pre_tick_update_forwarding_agrees_end_to_end`, and asserted DISSOLVED by the
> `cfg.rs` unit tests `hazard_v1_assign_then_write_dissolved`,
> `hazard_v5_trailing_read_dissolved`, `hazard_v7_escape_across_tick_dissolved`. V4
> and V6 still agree. What the rule retains is W4 (§4 Q1), which is not in this table.

Two results here are load-bearing and neither was predicted:

- **V5 vs V4** — the input read must **precede** the assignment. A read after it does
  not help, because the barrier only suspends at the point it appears.
- **V7** — the assigned register is **never read again in that segment**, and it still
  diverges. This killed the first candidate rule ("assigned then read back").

### 3.2 The adjudication — independent hardware sides with codegen

`examples/cdc/sv/two_domain_hierarchy.sv::ref_fast_counter`, committed in `0d67f9e`
(item 4, 2026-07-30) — long before this divergence was known, so a genuine outside
opinion — run as referee against both sides of `fast_counter`:

```
sim         = … (7,0) (8,1) (9,1) …
transpiled  = … (7,0) (8,0) (9,1) …
independent = … (7,0) (8,0) (9,1) …

transpiled == independent → true
sim        == independent → false
```

**Copper's simulator disagrees with both.** What this establishes is narrow and
worth stating precisely: a hardware engineer implementing *the English description*
("a sticky flag that asserts when the count reaches 8") writes the registered form,
and Copper's codegen matches that. It does **not** establish that forwarding is
inherently wrong — see the correction below.

> **CORRECTION (2026-08-21, after the prior-art pass).** An earlier revision of this
> doc concluded "the simulator is the wrong side" and used that to rule out fixing
> codegen. That overstated the evidence. Prost — the closest prior art, and itself a
> coroutine HDL — lowers coroutine bodies to a **combinational next-state block using
> blocking assignments**, which *preserves* forwarding (see §10.5). Forwarding is
> therefore a legitimate lowering for a coroutine surface, and arguably the one that
> preserves the source's meaning. What the adjudication actually rules out is the
> *disagreement*, not option (c).

### 3.3 Corpus status under the candidate rule

Rule tried: *a register assigned in the pre-tick segment with no `In` read
comb-reaching it on that path.* Path-sensitive, over `Cfg`, reusing
`leading_read_reaches`. **Matched all 7 variants — and over-flagged the corpus.**
12 of 77 clocked modules (measured once at commit `7bc7c90`, 2026-08-21 — `git log
-S"12 of 77"` — not a regression gate; the current pinned set is `EXPECTED_FLAGGED`
in the header, and the corpus pin prints today's scanned count):

| module | copies | status (2026-08-21) | evidence | today |
|---|---|---|---|---|
| `fast_counter` | 3 | **TRUE POSITIVE** | measured divergence + hardware adjudication | the three copies were migrated to the post-tick sticky update (§7 3b); the witness copy in `sequential_forwarding_divergence.rs` is DISSOLVED by §5.7 and compiles with no opt-out |
| `add_then_write` | 1 | **TRUE POSITIVE** | the V1 fixture itself | DISSOLVED 2026-08-26 (§5.7): legal, agreeing, no opt-out |
| `mac_fsm` | 3 | **FALSE POSITIVE** — output is `RegOut` (Q1) | `mac_fsm_sim_matches_transpiled_verilog` passes | not flagged |
| `if_tick_explicit` | 1 | **FALSE POSITIVE** — output is `RegOut` (Q1) | `if_tick_sim_matches_transpiled_verilog` passes | not flagged |
| `probe_fsm` | 1 | **TRUE POSITIVE** — same defect as D1, confirmed (Q2) | a leading read on every path fixes it, exactly as it fixes V1; plain `Out` | still flagged (the retained W4 class); in `EXPECTED_FLAGGED` |
| `branch_merge_explicit` | 1 | **AGREES** — measured (phase 0a) | plain `Out`, but every write is a CONSTANT; produced the constant-write clause. An earlier revision wrongly recorded this as `RegOut` | re-measured DIVERGENT by the sweep on 2026-08-25 (§5.5) — the 0a trace was too weak to see it; in `EXPECTED_FLAGGED` |
| `ram_prewrite` | 1 | **UNKNOWN** (plain `Out`, so the refined rule retains it) | `probe_mem_latency` is `#[ignore]`d — phase 0b stands | **narrowed out 2026-08-26** (§5.7): its write is not read-preceded, so it falls on the dissolved side; it never had a behavioral verdict and does not transpile (`tests/mem_latency_probe.rs::probe_mem_latency` remains an `#[ignore]`d diagnostic printout with no assertions) |

Two observations that shape the plan:

1. `mac_fsm` is the project's **G2 name-exact register reference**. A rule that
   rejects it is not shippable.
2. Two flagged modules have **no behavioral equivalence coverage at all**. They
   cannot be adjudicated until that coverage exists — which makes coverage a
   *prerequisite* of the guardrail, not a follow-up.

### 3.4 Why `mac_fsm` survives — ANSWERED

**Because its output is `RegOut`.** Not because of anything about its control flow.
`RegOut` buffers and commits at the clock edge, so the phase at which the write
executes cannot be observed. Changing *only* the port type on an otherwise-identical
divergent module flips it to agreeing (Q1, W8/W9). The same is true of
`if_tick_explicit` and `branch_merge_explicit` — all three false positives are
`RegOut` modules, and every divergent case uses a plain `Out`.

An earlier revision of this section guessed at an "escapes to an output" condition and
noted it could not explain V7. That line of reasoning was looking at the wrong axis.

---

## 4. Open questions (what must be answered before a rule exists)

- **Q1 — ANSWERED 2026-08-21. The discriminator is the OUTPUT PORT TYPE, not control
  flow.** The leading hypothesis ("the module contains no `In` read anywhere") is
  **refuted**: W4 — a mixed-alignment module shaped like `mac_fsm`, with a read on one
  arm and an unprotected assignment on the other — **diverges**. Mixed alignment does
  not protect.

  What actually separates them is that `mac_fsm`, `if_tick_explicit` and
  `branch_merge_explicit` all declare their outputs **`RegOut`**, while every
  divergent case uses a plain `Out`. Proven by changing *only* the port type on two
  otherwise-identical modules:

  | | plain `Out` | `RegOut` |
  |---|---|---|
  | W1/W9 (D1 minimal) | **DIVERGE** | **agree** |
  | W4/W8 (mixed alignment) | **DIVERGE** | **agree** |

  `RegOut` buffers and commits at the edge, so the phase at which the write executes
  cannot be observed — it is immune to the alignment by construction. That is exactly
  what the `RegOut` axis was introduced for.

  **Consequence for the rule: key on plain combinational `Out` writes, not on
  registers.** The §5.2 rule failed because it keyed on registers. This is the same
  structure `multi_write_collapse` already has (it excludes `RegOut` by construction),
  and it is corpus-clean by inspection: the refined predicate exempts both confirmed
  false positives (`mac_fsm` ×3, `if_tick_explicit`, `branch_merge_explicit` — all
  `RegOut`) while retaining every true positive (`fast_counter` ×3, `add_then_write`,
  `probe_fsm`, `ram_prewrite` — all plain `Out`).

- **Q2 — ANSWERED 2026-08-21. `probe_fsm` is the SAME defect as D1, not a second one.**
  W6 — `probe_fsm` with an unconditional leading `In` read, so *every* path carries a
  barrier — **agrees** with its transpiled SV, where `probe_fsm` as written diverges.
  The prescribed test ("does adding a preceding input read fix it, as it fixes V1?")
  returns yes. So the pre-existing `#[ignore]`d divergence in
  `tests/probe_timing_investigation.rs` is in scope for this plan, and its sibling
  `accum_2` (same family per its own note) very likely is too — a separately-tracked
  bug consolidates into this one. `accum_2` itself is not yet measured.

- **Q4 — ANSWERED by Q1: the legal alternative is `RegOut`.** The guardrail can point
  at exactly the remedy `multi_write_collapse` points at, rather than needing a
  bespoke rewriting rule per case.

- **Q3 — PARTLY ANSWERED.** D2 is *not* subsumed by D1's rule, and it now has a
  known legal form (read after the tick — see phase 3c), which is what the CDC
  designs use. It remains **unguarded**: there is still no independent-hardware
  adjudication for it, which is the prerequisite. Its root cause is narrower than it
  looked — `classify_reads` defers a read because *a tick follows it*, not because
  *its result crosses the tick*; in a passthrough the value is consumed before the
  edge and never needed deferring. Original question: They are pinned
  together only because they cancel; nothing shows they share a mechanism.
- **Q4 — What is the correct form for a designer who wants a sticky flag?** The
  guardrail must point at a *legal* alternative. For `fast_counter` that is the
  post-tick update (`tick; if count[3] { latched = 1 }`), verified to match the
  independent reference — but the general rewriting rule is unstated.
- **Q5 — REOPENED AND CLOSED 2026-08-25. An instance turned up, and the answer was
  a SECOND RULE, not a wider one.** The note below ends "if one turns up it should
  be measured before the rule is widened". One did: a one-cycle output pulse, found
  while writing the first sim-vs-Verilator test for the UART receiver, whose
  `rx_dv` is written on both sides of a tick.

  **Widening D1 past the head segment: MEASURED AND REJECTED.** Extending it to
  every post-tick segment flags **36 of 120** corpus modules, ~30 of which have
  passing equivalence tests — `det_010`, `mac_pipeline`, `dual_port_ram`,
  `bsg_dff_en`, every memory fixture (measured once at commit `99290da`,
  2026-08-25 — `git log -S"36 of 120"` — not a regression gate). Writing a plain
  `Out` after a tick is the ORDINARY multi-phase pattern and is correct.

  **What the divergent shape actually is: a plain `Out` driven in TWO phases** —
  which the multi-tick lowering *already refuses* ("output port `p` is driven in
  more than one phase … hold it in a register"). Control extraction hides the
  phases by rewriting the body into a single-tick `match pc` FSM, so the check
  counts one tick and passes while the `pc` states are the phases it meant to
  count. Witnesses, each flipping one condition (`tests/sequential_forwarding_divergence.rs`):

  | | shape | verdict |
  |---|---|---|
  | plain `Out`, two segments, no `In` | pulse | **DIVERGE**, exactly one cycle |
  | identical, `RegOut` | pulse | agree |
  | clear moved out of the trailing segment | pulse | **DIVERGE** — not the trailing segment |
  | plain ticks, no trailing statements | pulse | **refused by the linear path** |

  So the fix is `Cfg::multi_phase_out_write`, enforced in both front-ends
  (`copper-macros/src/lib.rs` and `copper-codegen/src/lib.rs::transpile_target`)
  with the same `RegOut` remedy and the same `allow_pretick_alignment` opt-out. On
  landing it flagged 9 corpus modules, six of them the synthetic witnesses; the
  three real ones (`examples/cpu/rv32i_cpu.rs` — since replaced by
  `rv32i_cpu_transpilable` — and `uart/system.rs::uart_tx` / `uart_rx`) were
  migrated to `RegOut` and their self-checks are unchanged (measured once at commit
  `99290da`, not a regression gate). Gate:
  `pretick_alignment_corpus.rs::multi_phase_out_write_flags_exactly_the_demonstration_modules`.
  Note the "9" is **unverifiable history**: the pin at that same commit already
  listed exactly {`pulse_plain`}, and that is still its whole `EXPECTED_MULTI_PHASE`
  today, so the figure predates the witnesses' final form and cannot be
  reconstructed from the tree.

  D1's own rule is UNCHANGED and still head-segment-only — deliberately. The two
  are complementary in the same way D1 and `multi_write_collapse` are.

  *Original note, kept for the measurement:* The rule examines only
  head → first tick, and this was recorded as a known false negative citing the
  multi-tick `accum_2`. **`accum_2` does not diverge.** Its test was `#[ignore]`d as
  "sim and transpiler disagree by one cycle … adjudication pending", but it passes —
  and passes *without* the D2 fix, so earlier read-timing work had repaired it and the
  ignore went stale. Un-ignored. So the rule has **no known false negative**; the
  middle-segment gap is theoretical, with no instance in the corpus. If one turns up
  it should be measured before the rule is widened.

---

## 5. Approaches tried — three rejected, and the rules that landed

§5.1–5.3 are **rejected** and must not be re-tried (§5.3 only as a *blanket*
change — §5.7 records the narrow sub-class where it was later applied on measured
evidence); §5.4 and §5.5 record two more rejected widenings *and* the rules that
eventually worked, kept together so the discriminator is read beside the attempts
it replaced; §5.6 is the derived-first rule; §5.7 is the 2026-08-26 dissolution and
narrowing.

### 5.1 "Always-barrier" — REJECTED 2026-08-21

Inject `pre_edge_barrier()` unconditionally at the loop top so alignment stops being
incidental. Implemented behind a `COPPER_ALWAYS_BARRIER` env flag in `copper-macros`,
measured, reverted — the flag no longer exists in the tree (`git log
-S"COPPER_ALWAYS_BARRIER"` finds the record, commit `7bc7c90`).

- **Fixes** D1's forwarding (V1 sim `[2,3,4…]` → `[1,2,3…]`, matching SV) and leaves
  the already-correct leading-read form alone.
- **Over-corrects Moore outputs.** The loop top then runs only at the pre-edge, so a
  Moore output shows the **pre**-edge register value where SV's `assign count_out =
  count` shows the post-edge one. `fast_counter`'s count went `[(1,0),(2,0)…]` →
  `[(0,0),(1,0)…]` — still ≠ the independent reference, just wrong elsewhere.
- **Corpus damage: 22 failures / 654** (measured once at commit `7bc7c90`,
  2026-08-21, not a regression gate), and they are the modules that are currently
  *correct* — `counter`, `up_down_counter`, `accumulator_en`, `slow_counter`,
  `traffic_light`, `seq6` equivalence; the `sync_2ff` anchor; poll-order
  independence; the frozen golden traces.

> **The durable finding.** The pre-tick segment does **two jobs**: compute next state
> (wants pre-edge values — what D1 gets wrong) *and* drive Moore outputs (wants the
> post-edge register value — what the barrier breaks). **No single global phase
> choice satisfies both.** Any real fix must separate the two jobs, or reject the
> shape. This is also why "just fix the executor's phase machinery" is not a small
> change.

### 5.2 The naive static rule — REJECTED 2026-08-21

*A register assigned in the pre-tick segment with no `In` read comb-reaching it.*
Matched 7/7 variants, false-positived on `mac_fsm` and `if_tick_explicit` (§3.3).
Reverted.

**Why it failed, now known (Q1):** it keyed on **registers**. The observable
divergence requires a plain combinational **`Out`** — `RegOut` is immune by
construction.

> **Correction (2026-08-24): `RegOut`'s immunity is narrower than stated here.** It is
> immune to the *phase* question this document is about — when the segment runs is
> unobservable through a port that commits at the edge. It is **not** immune to
> sequential forwarding *within* the segment: measured, a `RegOut` written **after** a
> register update in the same segment emits `out <= n`, the register's **old** value,
> while the simulator forwards the update and writes the new one. D1's compile error
> exempts `RegOut`, so that shape passed the guardrail and diverged silently.
>
> **Resolved 2026-08-25 (`TODO` causes L, L-1, L-2): the forwarding was repaired, and
> the exemption stands.** The two options were "narrow the D1 exemption" or "repair the
> forwarding"; the second is right, because the shape has a correct lowering — a drive
> sampled AT the edge simply has to be emitted from pre-edge register values. It is
> also NOT a `RegOut` question, which is why narrowing the exemption would have been
> the wrong repair: a plain `Out` written *conditionally* becomes an implicit-hold
> register too and had exactly the same lag (L-1). The rule is the emission context,
> not the port type. So `RegOut`'s immunity to the *phase* question this document is
> about is intact, and unrelated to the forwarding bug that briefly seemed to qualify
> it. Pinned by `tests/regout_forwarding_equivalence.rs`.

Every false positive was a `RegOut` module. The corrected rule keys on the output
write, which is the same structure `multi_write_collapse` already uses.

### 5.3 Option (c), Prost-style lowering — REJECTED 2026-08-21 (as a blanket change)

Measured by hand-writing the lowering Prost uses (combinational next-value in
coroutine order; an output write reads the *forwarded* value) and running it under
Verilator against the simulator. The controlled pair is V1 and V4 — **identical
designs differing only in a leading `In` read**:

| | sim | current SV | Prost-style SV |
|---|---|---|---|
| **V1** (no leading read) | `2 3 4 5 6 7 8 9` | `1 2 3 4 5 6 7 8` ✗ | `2 3 4 5 6 7 8 9` ✓ |
| **V4** (leading read) | `1 2 3 4 5 6 7 8` | `1 2 3 4 5 6 7 8` ✓ | `2 3 4 5 6 7 8 9` ✗ |

It **fixes D1 exactly** and **breaks the currently-correct case exactly**. Symmetric
to §5.1: always-barrier fixed the barrier case and broke Moore outputs; Prost-style
lowering fixes the no-barrier case and breaks the barrier-protected one.

Projected corpus impact (same shape, barrier-protected, currently passing): `lfsr`,
`det_110101`, `shift_register` — 6 module copies with equivalence tests that would
start failing. Not individually measured; the V1/V4 pair is the controlled evidence.

> **THE GENERAL RESULT — this is the important part.** No single codegen lowering can
> match the simulator, because **the simulator is not internally consistent**: two
> structurally identical modules behave differently depending on whether one of them
> happens to read an input. For codegen to match, it would have to replicate that
> incidental rule — i.e. emit different hardware for the same logic based on the
> presence of an unrelated port read. That is not a lowering anyone should write.
>
> So **the simulator must be made uniform first**, and only then can codegen be
> matched to whichever uniform semantics is chosen. Fixing either side alone is now
> measured-and-rejected in both directions (§5.1, §5.3).

Note this does **not** impugn Prost: its lowering is correct *for Prost*, whose
alignment is uniform because it has no barrier mechanism to make it incidental. The
defect is Copper's, and it is upstream of the lowering.

### 5.4 D1 in the TRAILING segment — two widenings REJECTED, then GUARDED 2026-08-25, NARROWED 2026-08-27

The gap is real and measured: D1's canonical shape moved past the last tick,

```rust
loop { for _ in 0..2 { clk.tick().await; } n = n + 1; o.write(n); }
```

diverges by one cycle, and `unprotected_pretick_out_write` does not flag it (it
examines head → first tick). Pinned as
`sequential_forwarding_divergence.rs::d1_in_the_trailing_segment_is_an_unguarded_gap`.
The trailing segment runs in the SAME cycle as the head segment — falling off the
end of the body and re-entering it costs no clock — so it is exposed to the
identical phase question. Two ways of covering it were implemented and measured
(once, at commit `b853d46`, 2026-08-25 — `git log -S"newly flagged, all passing"`
— not a regression gate):

| widening | newly flagged, all passing | why it is wrong |
|---|---|---|
| merge the trailing segment into the head region | **25** | includes `fast_counter_corrected` — the module D1's OWN REMEDY produces. "Move the register update after the `clk.tick().await`" puts it in the trailing segment; merging then flags the fix as the bug. Also `sync_2ff` (the CDC anchor) and `dual_port_ram`. |
| apply the two clauses to the trailing segment as a SEPARATE region | **10** | all memory modules. `rom_from_fn`'s trailing segment is `if ready { q = data() } data.write(q)` — assigns a register and drives a plain `Out` from it, structurally identical to the divergent DUT — and it AGREES. |

> **The open question is the discriminator between that DUT and `rom_from_fn`, and
> a leading `In` read is NOT it** — the same DUT plus one was measured and still
> diverges (which also refutes the natural hypothesis that the loop-top barrier
> pins the whole iteration). Until some condition has a flipping witness (R3),
> widening rejects correct designs (R1). The second widening is the closer of the
> two and is where a third attempt should start.

#### ANSWERED 2026-08-25 — the discriminator is how many clock edges the body crosses

The third attempt started where the note says, and the flipping witness (R3) came
from varying exactly one thing. With the **identical** trailing body
`n = n + 1; o.write(n);`:

| loop | result |
|---|---|
| `loop { clk.tick().await; … }` — one edge per iteration | **agrees** — and this is `rom_from_fn`'s shape |
| `loop { for _ in 0..2 { clk.tick().await; } … }` — more than one | **diverges** by one cycle, uniformly |

In a single-tick loop the trailing statements **share the head's phase**: falling off
the end and re-entering costs no cycle, the CFG puts them in one Comb-component, and
`shir_lower`'s single-tick path hoists them into one phase. There is no separate
trailing region, so there is nothing to be misaligned against. That is exactly why
widening #2 — which treated every trailing segment as its own region — cost ten false
positives, **all of them single-tick memory modules**.

Three hypotheses were measured and discarded on the way, each in one run: the update's
conditionality, the write's conditionality, and whether the register is loaded from an
input rather than itself. All four variants diverge identically, so none of them
separates anything.

**Not the Comb-component count.** The divergent DUT has *one* component too — its
extra edges live inside a folded nested loop, which the parent CFG cannot see into.
Gating on components suppressed precisely the case the rule exists for (measured
during implementation). `Cfg::crosses_more_than_one_tick` counts tick nodes and treats
a folded tick-bearing loop as more than one, since it crosses an edge per iteration of
its own.

**Corpus cost: one real module** (commit `56233ce`, 2026-08-25).
`rv32i_cpu_pipelined`'s `program_counter`, migrated to `RegOut` — the same remedy
its scalar sibling had already been given for the multi-phase rule, and the harness
discards that port, so only the type and its wire changed. On landing the flagged
demonstration witnesses were `trailing_update` and `pulse_plain`; `pulse_plain`
left the set the same day when `written_on_all_paths` was scoped to the region
being asked about (its trailing `dv.write(Zero)` is an unconditional constant — it
stays guarded by `multi_phase_out_write`, which is what is actually wrong with it).

#### NARROWED 2026-08-27 — the linear class is exempt (commit `8ffade7`)

The phase-C decision probes of `PAIRED_IMPLEMENTATION_SCOPE.md` measured a witness
on each side of a second discriminator, **which lowering route the trailing
statements take**:

| witness (`tests/sequential_forwarding_divergence.rs`) | shape | verdict |
|---|---|---|
| `linear_trailing` (`linear_trailing_probe`) | every tick a bare top-level statement — the linear lowering path | **agrees**: the linear path commits trailing updates at the right edge (the 2026-08-25 shared-map work) |
| `trailing_update` | the last tick inside a folded `for` | **diverges** one cycle |
| `branch_trailing` (`branch_trailing_probe`) | control extraction with a top-level last tick | **diverges** one cycle — the extraction route commits trailing updates one edge late wherever the last tick sits |

So the rule's remaining refusal is a lowering limitation of the extraction route,
and it is gated on exactly that: `Cfg::unprotected_trailing_out_write` returns `[]`
unless **both** `crosses_more_than_one_tick` (two tick nodes, or a folded
tick-bearing nested loop) **and** `has_nested_tick` (some tick sits inside a
branch or loop — the source-level mirror of extraction's trigger) hold. Within the
trailing region — nodes with a tick-free path back to the loop head — it then
applies D1's two clauses: a write that reads a register, or a write not on every
path (`written_on_all_paths` over the trailing entries). `linear_trailing` dropped
its opt-out and compiles clean; it stays in the divergence file as the exemption's
measured witness. Unit pins in `cfg.rs`: `trailing_linear_class_exempt`,
`trailing_folded_loop_flagged`, `trailing_branch_nested_tick_flagged`. Exact-set
pin: `pretick_alignment_corpus.rs::trailing_out_write_flags_exactly_the_demonstration_modules`,
`EXPECTED_TRAILING` = {`trailing_update`, `branch_trailing`}, both carrying
`allow_pretick_alignment`. The recorded follow-up is to retire the rule when phase
C lands the corrected extraction-path trailing lowering (`PAIRED_IMPLEMENTATION_SCOPE.md`,
Phase C).

**The head rule is unchanged and still head-segment-only**, deliberately: widening #1
proved the two regions cannot share a rule, and they are now complementary the way D1
and `multi_write_collapse` are.

### 5.5 The CONSTANT-WRITE exemption was unsound for a conditionally-written `Out` — found AND FIXED 2026-08-25

`unprotected_pretick_out_write` clause (ii) considers only plain `Out` ports driven
**from a register**, exempting a write of a constant: *"a write of a constant is
idempotent across the phase shift — the misalignment changes when the write happens,
so it is only observable if the value written differs between phases."*

That premise holds only when the write happens **every** cycle. It does not when the
port is written on some paths and not others (the enabled-`Out` idiom), or when
different arms write different constants — then *when* the write lands is observable,
because the alternative is the port's held value.

**Measured**, `tests/sequential_forwarding_divergence.rs`:

```rust
loop {
    match pc {
        0u8 => { if sel.read() == Logic::One { pc = 1; } }
        1u8 => { o.write(Logic::One); pc = 0; }   // belongs to the NEXT cycle
        _ => {}
    }
    clk.tick().await;
}
```

| | cycle 0 | 1 | 2 | … |
|---|---|---|---|---|
| simulator | 1 | 1 | 1 | 1 |
| transpiled SV | **0** | 1 | 1 | 1 |

and with the other arm driving the port low, so the shift is visible every cycle
rather than once (`pc_arm_toggle`):

| | 0 | 1 | 2 | 3 | … |
|---|---|---|---|---|---|
| simulator | 1 | 0 | 1 | 0 | … |
| transpiled SV | 0 | 1 | 0 | 1 | … |

The two traces are each other shifted by exactly one cycle: a phase shift, not an
initialisation artifact. `unprotected_pretick_out_write` returns `[]` for both.

**How it was found, and why it had not been.** The corpus differential sweep —
then `tests/corpus_equivalence.rs`, phase 1 of `design_docs/CORPUS_DIFFERENTIAL_SWEEP.md`;
since 2026-08-25 (commit `6ce15d6`) generated by `build.rs` into
`tests/corpus_generated.rs` and guarded as G-D by `tools/regression.sh` — ran 200
cycles of seeded random stimulus at `branch_merge_explicit`, a fixture that had
lived in the tree with only a *structural* check on it. The sharpest statement of the finding is that
`branch_merge` and `branch_merge_explicit` transpile to **byte-identical**
SystemVerilog (asserted by `control_extraction_structural.rs`), the async twin agrees
with that SV for all 200 cycles, and the explicit twin leads it by one — so the
simulator disagrees with *itself* depending on how the same hardware is spelled.

**FIXED the same day, and it is the first widening of this rule that passes R1.**
The exemption now applies only where the output is written on **every path** of the
segment (`Cfg::written_on_all_paths`). §5.4 records two widenings rejected for corpus
cost — 25 and 10 modules, most of them correct designs — so this one was measured the
same way before landing:

> **Corpus cost: three modules, and all three are the measured divergences**
> (measured once at commit `348ddd0`, 2026-08-25; not a regression gate — the
> three are the constant-write entries of `EXPECTED_FLAGGED` today).
> `branch_merge_explicit` (the corpus instance the sweep found), plus the two
> witnesses `pc_arm_write` and `pc_arm_toggle`. **Zero false positives.** 26/26
> examples and all 93 differential cases unaffected (the sweep's size on the day;
> `G-D` prints the current count). Unit pins in `cfg.rs`:
> `a_conditional_constant_write_is_flagged`,
> `an_unconditional_constant_write_is_not_flagged`; sim-vs-Verilator witnesses:
> `a_write_in_a_state_arm_leads_the_hardware_by_one_cycle`,
> `the_state_arm_lead_is_systematic_not_a_first_cycle_artifact`.

That is the discriminator §5.4 asked for, and it was hiding in plain sight: *a
constant is idempotent across the phase shift only if it is written on every path.*
Where some path leaves the port holding, the phase shift is observable even though the
value written never changes — which is why the rule keys on the write's
**conditionality**, not on the value.

All three modules carry `allow_pretick_alignment`: each exists to demonstrate the
hazard, and the flag silences the error, not the detection, so they stay visible to
every corpus scan. A real design in this shape now gets a compile error pointing at
`RegOut`.

**Why this direction rather than a simulator change.** The alternative — teaching the
executor to run the segment's two jobs in different phases (§5.1's "no single global
phase choice satisfies both") — is a more complex simulation rule, and the direction
taken (2026-08-25) is to **limit the design expressions instead**: keep "a value
live across a `clk.tick().await` is a register", keep one value per variable (no
current/next distinction, as in §10.1's languages), and reject the shapes that
diverge rather than growing the machinery that reconciles them.

### 5.6 The write BETWEEN a leading read and the update — DERIVED, then measured, then GUARDED 2026-08-26

The first rule in this family to arrive in the opposite direction: **derived from
the cycle-dataflow model before its controlled measurement existed**
(`design_docs/CYCLE_DATAFLOW_SEMANTICS.md`; the derivation and its record are
`design_docs/DERIVATION_TABLE.md` F2/m1). The barrier a leading read installs
parks the task **at the read site**, so a plain-`Out` write placed after the read
executes in the pre-edge settle — and if it also precedes the update of the
register it reads, it captures the **pre-update** value, one generation behind
what `assign o = r` (Q) shows at every observation instant.

**D1 cannot see it, structurally**: D1's clause (i) treats the leading read as the
*protection*, because in D1's shapes the write comes after the update and the
barrier hands it the forwarded (committing) value. Here the read is the
*exposure*. Complementary triggers, like every pair in this family.

The V8 battery (`tests/sequential_forwarding_divergence.rs`), position of the
write the only variable, **every trace predicted before the run**:

| | shape | verdict |
|---|---|---|
| V8a | `read; write; update` | **DIVERGE** — sim `[0,1,…]`, SV `[1,2,…]`, silently |
| V8b | `read; update; write` | agree (the forwarded value *is* the committing one) |
| V8c | `write; read; update` | agree (the write precedes the barrier point) |
| V8d | V8a through a `let next` temp | **DIVERGE** — identically; renaming changes nothing |

The in-vivo instance was `rv32i_cpu_transpilable`'s `program_counter` (`TODO`
cause Q, divergence #1), rewritten to the post-commit form before this rule
existed. **Corpus cost: zero real modules** — and the rule earned its keep on
landing day: it flagged `ui/pass/single_loop_local_ok.rs::accum`, a compile-only
fixture nobody had ever measured, which V8d then confirmed diverges exactly like
V8a. The fixture was reordered to V8c's form; the shape is pinned as V8d.

Rule: `Cfg::pretick_out_write_before_update`, three clauses, each with a flipping
witness (V8c flips the read clause, V8b the update-order clause, a constant write
the register-read clause). Enforced in the macro (`copper-macros/src/lib.rs`) with
the same `allow_pretick_alignment` opt-out; not run by the transpiler. Pins: unit
tests `v8a_write_between_read_and_update_flagged`, `v8b_write_after_update_not_flagged`,
`v8c_write_before_read_not_flagged`, `v8_constant_write_not_flagged`,
`v8_regout_not_flagged`, `v8_unrelated_update_after_write_not_flagged` in
`cfg.rs`; trybuild `ui/fail/write_before_update.rs` and the `v8b_publish_after_update`
/ `v8c_publish_before_read` cases of `ui/pass/pretick_alignment_ok.rs`; the
sim-vs-Verilator battery `v8a_write_between_leading_read_and_update_diverges_known_gap`,
`v8b_moving_the_write_after_the_update_removes_the_divergence`,
`v8c_moving_the_write_before_the_read_removes_the_divergence`,
`v8d_temp_renamed_update_diverges_like_v8a_known_gap`; and the exact-set pin
`write_before_update_flags_exactly_the_demonstration_modules` (the fourth scan in
`pretick_alignment_corpus.rs`, `EXPECTED_WRITE_BEFORE_UPDATE` listed in the header).

**The trailing clause (2026-08-27).** The identical mixed-generation hazard
exists in the trailing segment with no read involved: trailing statements
execute at the cycle's opening and commit at that same edge, so a
publish-then-load order shows the previous generation all cycle. Flipping pair:
the `out_phase_dut.rs` claim-ledger entries (`before_commit` diverges,
`after_commit` agrees), minimal-measured as `v8t_stage_publish_then_load` (sim
`[0,2,3,…]`, SV `[2,3,4,…]`). On landing the exact-set pin surfaced **three
real modules** — `module_composition_hybrid`'s pipeline stages, whose tests were
sim-only and whose composed pipeline stream also carried the phantom extra
cycle — migrated to the canonical registered-stage spelling
(`write; read; tick; update`, which matches `always_ff reg <= f(in); assign out
= reg` cycle-for-cycle standalone and composed) and their streams re-blessed to
the true two-flop latency (the stages live in `tests/module_composition_hybrid.rs`;
commit `b439894`). Pins for the clause: `v8t_trailing_write_before_update_flagged` /
`v8t_trailing_write_after_update_not_flagged` in `cfg.rs`, the witness
`v8t_trailing_stage_shape_verdict`, and the last two entries of
`EXPECTED_WRITE_BEFORE_UPDATE`. Honest scope (R6): pre-tick and trailing regions;
middle segments of multi-tick loops are unexamined (no measured instance);
conditional writes carry no extra clause because none has a measured
divergence.

### 5.7 The dissolution and the narrowing — cycle-dataflow phases B and D, 2026-08-26

The paired-implementation migration (`PAIRED_IMPLEMENTATION_SCOPE.md`) landed
**forwarded continuous-assign emission for opening-prefix drives** (a plain-`Out`
write not preceded by any `In` read in its segment now emits its `edge_value` —
`assign o = (r + 8'd1)` for V1), which is §5.3's Prost-style lowering applied to
exactly the sub-class where §5.3's measurement showed it correct, and no wider.
The sv-baseline byte-diff confirmed the corpus cost as **exactly the two
demonstration witnesses and zero live modules** (F1). Re-measured under it
(`d_narrowing_battery_verdicts`):

| | shape | verdict under forwarded emission |
|---|---|---|
| V1/V2/V3 | update-then-write, no reads | **agree** — dissolved |
| V5 | trailing read after the write | **agree** — dissolved (the write is an opening-prefix drive) |
| V7 | escape across the tick (`s = r`) | **agree** — its 2026-08-21 verdict was stale even before phase B: the 2026-08-25 shared trailing-forwarding map already emits `s <= r + 1` |
| W4 | mixed alignment | **DIVERGE** — sim holds `i + 1`, SV alternates; the path-dependent boundary no single emission can match |

D1 was then narrowed on that evidence: the register-reading clause requires the
write to be **read-preceded** (`leading_read_reaches`), retaining exactly the
W4/`probe_fsm` class; the conditional/constant hold clause is untouched
(constants are blind to forwarded emission — `pc_arm_*`'s pins stayed green
throughout). `add_then_write`, `fast_counter` (witness), V5 and V7 dropped their
opt-outs and compile clean on their own merits; `ram_prewrite` left the flagged set
with a recorded note in `EXPECTED_FLAGGED` (it never had a behavioral verdict and
does not transpile). The narrowing is pinned three ways: the `cfg.rs` unit tests
`hazard_v1_assign_then_write_dissolved`, `hazard_v5_trailing_read_dissolved`,
`hazard_v7_escape_across_tick_dissolved` (each asserts `[]`) beside
`hazard_w4_mixed_alignment_flagged`; `ui/pass/pretick_alignment_ok.rs::forwarded_opening_drive`
(the former ui/fail V1 case); and `EXPECTED_FLAGGED`, which gained
`w4_mixed_alignment` and lost `add_then_write`, `fast_counter` and `ram_prewrite`
the same day (commit `7abfd98`). The rule's implementation is the
`register_read_hazard` conjunction in `Cfg::unprotected_pretick_out_write`:
`!node.uses.is_disjoint(&regs) && self.leading_read_reaches(n)`.

---

## 6. Requirements — what a shippable guardrail must satisfy

These are the acceptance criteria, in the spirit of the `multi_write_collapse`
precedent ("empirically pinned + corpus-clean + false-positive-free").

- **R1 — Zero false positives, corpus-wide.** Every currently-passing equivalence
  test must still compile and pass. `mac_fsm` (the G2 reference) especially.
- **R2 — Catches every confirmed true positive.** At minimum `fast_counter` (3
  copies) and the V1/V2/V3/V5/V7 shapes.
- **R3 — Conditions derived from measurement.** Each necessary condition must be
  justified by a variant that flips when it is removed, as the V4/V5 pair justifies
  "the read must precede".
- **R4 — Static, all-paths, compile-time.** As with the multi-write work: static
  checks over dynamic ones. A dynamic backstop was already tried and dropped there
  for false-firing.
- **R5 — Actionable diagnostic.** A spanned error naming the register and pointing at
  a legal alternative (Q4), like the multi-write rule points at `RegOut`.
- **R6 — Honest scope.** Any known false negative (e.g. multi-tick segments, Q5) is
  documented on the rule and tracked, not left implicit.
- **R7 — The anchor is repaired, not re-blessed silently.** Fixing `fast_counter`
  removes the compensation and exposes D2 in `two_domain_hierarchy_cdc.rs`. That test
  must end up green *for the right reason*, or be explicitly downgraded with a
  recorded rationale.

---

## 7. Development plan

Sequenced, each phase gated on the previous. Phases 0–1 are pure measurement;
phase 3 changes `copper-macros` behaviour. **This section is the 2026-08-21 plan
with its completion marks; where §5.4–§5.7 changed a rule afterwards, the phase is
marked SUPERSEDED with a pointer rather than rewritten.**

### Phase 0 — Coverage prerequisites (no semantics change)

Two flagged modules cannot currently be adjudicated because nothing checks their
behaviour. Fix that first, or the corpus verdict stays partly unknown.

- **0a** ~~Behavioral equivalence test for `branch_merge_explicit`~~ **DISCHARGED
  2026-08-21** — measured sim-vs-Verilator: it **AGREES**. Note the trace is weak
  (its outputs are write-once `Logic::One` and saturate), but it was enough to
  establish it is not divergent, and it produced the constant-write clause.
- **0b** ~~Resolve `ram_prewrite` / `probe_mem_latency` — un-ignore it, or record why
  it cannot be.~~ **MOOT since 2026-08-26**: the §5.7 narrowing put `ram_prewrite`
  outside the rule (its write is not read-preceded), so it no longer needs a
  verdict to satisfy G0. `tests/mem_latency_probe.rs::probe_mem_latency` remains an
  `#[ignore]`d diagnostic printout with no assertions; `ram_prewrite` does not
  transpile, so the corpus sweep will cover it the moment it does.
- **Gate G0:** every module the candidate rule flags has a behavioral verdict:
  diverges, or agrees. **MET as of 2026-08-26** — every entry of the four pinned
  sets has a measured verdict in `tests/sequential_forwarding_divergence.rs` or
  the sweep.

### Phase 1 — Discrimination measurement (answers Q1, Q2) — **DONE 2026-08-21**

- **1a** Test the Q1 hypothesis directly: does a module containing *no* `In` read
  anywhere behave differently from one with mixed per-arm alignment? Construct the
  minimal pair and measure.
- **1b** Determine whether `probe_fsm` and `accum_2` share D1's mechanism. Concrete
  test: does adding a preceding input read to the unprotected path fix them, as it
  fixes V1? If not, they are a separate defect and leave this plan.
- **1c** Extend the variant map until every condition in the proposed rule has a
  flipping witness (R3).
- **Gate G1: MET.** Q1 and Q2 are answered above from a measured 8-variant battery
  (W1–W9), including the two controlled port-type pairs that isolate `RegOut` as the
  discriminator. The rule shape that follows — *a plain combinational `Out` written on
  a path where a register was assigned in the pre-tick segment with no preceding `In`
  read* — exempts both confirmed false positives and retains every true positive by
  inspection. **Phase 2 can proceed**; the remaining work there is implementing it
  over the CFG and re-running the corpus sweep to confirm cleanliness empirically
  rather than by inspection.

### Phase 2 — Rule synthesis and offline validation — **DONE 2026-08-21; SUPERSEDED by §5.7 (2026-08-26)**

> **Read this phase as history.** The rule below is D1 *as landed 2026-08-21*.
> Clause 1 was narrowed on 2026-08-26 to require the write to be **read-preceded**
> (`leading_read_reaches`), and clause 1 gained the constant-write hold condition
> on 2026-08-25 (§5.5). Three rows of the witness table — V1, V7, V5 — carry the
> verdict "DIVERGE → flag"; each is now asserted **DISSOLVED** (returns `[]`) by a
> unit test in `cfg.rs`: `hazard_v1_assign_then_write_dissolved`,
> `hazard_v7_escape_across_tick_dissolved`, `hazard_v5_trailing_read_dissolved`.

**The rule, as landed** (`Cfg::unprotected_pretick_out_write`). Flag a plain
combinational `Out` port `P` when **both** hold in the pre-tick segment:

1. some node writes `P` **and reads a register** — a constant write is idempotent
   across the phase shift, so it is unobservable; and
2. some node assigns a register with **no `In` read comb-reaching it**
   (`leading_read_reaches`) — the barrier is what pins the segment's phase.

`RegOut` is excluded for free: `Node::writes` holds only combinational outputs, the
same way `multi_write_collapse` gets its exclusion.

**Every clause has a measured witness** (R3), each a unit test in `cfg.rs`:

| clause | witness | verdict (2026-08-21) | today |
|---|---|---|---|
| register assigned pre-tick, plain `Out` | V1 | DIVERGE → flag | DISSOLVED (`hazard_v1_assign_then_write_dissolved`) |
| no in-segment read-back needed | V7 | DIVERGE → flag | DISSOLVED (`hazard_v7_escape_across_tick_dissolved`) |
| read must *precede* the assignment | V4 vs V5 | agree / DIVERGE | V4 unchanged (`hazard_v4_leading_read_not_flagged`); V5 DISSOLVED (`hazard_v5_trailing_read_dissolved`) |
| post-tick assignment is safe | V6 | agree → no flag | unchanged (`hazard_v6_post_tick_assign_not_flagged`) |
| mixed alignment does **not** protect | W4 | DIVERGE → flag | unchanged — the retained class (`hazard_w4_mixed_alignment_flagged`) |
| `RegOut` is immune | W8, W9 | agree → no flag | unchanged (`hazard_w8_…`, `hazard_w9_…`) |
| **write must read a register** | `branch_merge_explicit` | agree → no flag | REVERSED by §5.5: a constant on *some* paths only is flagged (`a_conditional_constant_write_is_flagged`) |
| same defect as `probe_fsm` | W5/W6 | DIVERGE → flag | unchanged (`hazard_probe_fsm_flagged`) |
| barrier-pinned corpus shape | `lfsr` | agree → no flag | unchanged (`hazard_lfsr_shape_not_flagged`) |

The constant-write clause was added *during* phase 2: `branch_merge_explicit` drives
three plain `Out`s from an unprotected path and was flagged by the first cut, so it
was measured (phase 0a, discharged) — it **agrees**, because every write is
`Logic::One`. Flagging it would have rejected a correct design.

- **Gate G2: MET.** `copper-analysis/tests/pretick_alignment_corpus.rs` scans the
  clocked modules across `examples/`, `src/` and `tests/` and flags an exact set —
  the measured-divergent modules, nothing else. On landing that was 76 scanned,
  **exactly 7** flagged (measured once at commit `ccf4877`, 2026-08-21, not a
  gate); today the pin is `EXPECTED_FLAGGED` — five modules, listed in the header —
  and the test prints the scanned count on every run. The expectation is an *exact
  set*, so the test fails in both directions: a newly flagged module is a
  regression or a real bug, and a no-longer-flagged one means the divergence was
  fixed and several pinned tests need re-blessing.

### Phase 2 (original scope, for reference)

- **2a** Implement as `Cfg::…` in `copper-analysis`, path-sensitive, reusing
  `comb_reaches` / `leading_read_reaches`.
- **2b** Unit tests: one per variant, asserting flag/no-flag.
- **2c** Corpus sweep as a *test*, not a scratch file — the sweep is the evidence, so
  it should be permanent (this is the lesson from `register_reconciliation.rs`, whose
  narrow scope hid a real bug).
- **Gate G2:** sweep is clean — flags exactly the confirmed-divergent set, nothing
  else (R1, R2).

### Phase 3 — Wiring and fixture migration — **DONE 2026-08-21**

- **3a DONE** — wired into the `#[hardware(sequential)]` arm as a spanned error naming
  the port and all three remedies. **Escape hatch decision: opt-out attribute**
  `#[hardware(sequential, allow_pretick_alignment)]`, on the precedent that every lint
  in this space ships a waiver (§10.2). It silences the **error, not the detection** —
  the corpus test still counts opted-out modules, so one cannot quietly vanish.
- **3b DONE** — `fast_counter` ×3 migrated to the post-tick sticky update (the form
  adjudicated against independent hardware); the four demonstration fixtures carry the
  opt-out. `fast_counter_corrected` deliberately does **not** carry it, so the
  corrected form is proven to pass on its own merits.
- **3c DONE — and better than planned.** The plan expected D1's fix to expose D2 and
  force a downgrade of the anchor (R7). It did expose it — all three arms failed — but
  **D2 also has a legal form**: a *leading* read classifies `Deferred` and samples at
  the pre-edge while its producer updates at the post-edge; reading **after** the tick
  classifies `Immediate` and tracks it.

  | | flag_raw | sync_q | consumer |
  |---|---|---|---|
  | leading read (old) | 4 | 5 | **6** ✗ |
  | trailing read | 4 | 5 | **5** ✓ |

  With both corrected, `two_domain_hierarchy_cdc.rs` passes **for the right reason**
  rather than by cancellation — verified by checking that correcting only one moves
  the boundary 5 → 6 and breaks it. **The anchor is repaired, not downgraded.**
- **3d DONE** — `ui/fail/pretick_alignment.rs` plus `ui/pass/pretick_alignment_ok.rs`
  covering all four accepting clauses (post-tick update, `RegOut`, leading read,
  constant write). Since 2026-08-26 the `fail` case is `mixed_alignment` (W4) and
  the `pass` file also carries `forwarded_opening_drive` (the former V1 fail case)
  and the V8b/V8c shapes.
- **Gate G3: MET** — the full regression run (`tools/regression.sh`, ending in
  `REGRESSION OK`) green.

**A bug introduced and caught during this phase, worth recording.** Adding the flag
broke three attribute parsers using `parse_args::<syn::Ident>()`, which fails outright
on two idents — so opted-out modules **silently disappeared** from
`pretick_alignment_corpus`, `register_reconciliation` and `real_examples`. It surfaced
only because the corpus test asserts an **exact set**: it reported all 7 modules as
"no longer flagged" when just 3 should have been. A `>=` threshold would have passed.
That is the same defect class this whole document is about, introduced while fixing it.

### Phase 3 (original scope, for reference)

- **3a** Wire into the `#[hardware(sequential)]` arm as a spanned error (R5).
- **3b** Migrate the three `fast_counter` copies to the legal form (Q4).
- **3c** Re-bless `two_domain_hierarchy_cdc.rs` — the compensation is now gone, so
  D2 is exposed. Either fix/accept D2 or downgrade that test with a recorded
  rationale (R7). **This is the step where D2 stops being deferrable.**
- **3d** `trybuild` `ui/fail/` + `ui/pass/` cases, per the P1 pattern.
- **Gate G3:** `cargo test --workspace` green, `tools/regression.sh` `REGRESSION OK`.

### Phase 4 — Documentation

- **4a DONE** — `SYNCHRONOUS_SEMANTICS.md` §Output timing carries the family
  ("The pre-tick alignment family — dissolved where the denotation defines it,
  refused where nothing can realize it") and points back here for the record.
- **4b OPEN** — the paper's Threats to Validity / Limitations section and
  contribution 5: report the divergence and the restriction, as the multi-write
  case is reported. (The earlier `paper/threats_to_validity.md` draft is gone; the
  paper draft under `paper/` — `sigconf.tex`, untracked as of 2026-09-01 — has a
  `Limitations` section and no Threats to Validity section yet.)
- **4c OPEN** — retire this doc to a status note, or move it to `OUTDATED/`. Not
  yet: the trailing rule's retirement is still gated on phase C
  (`PAIRED_IMPLEMENTATION_SCOPE.md`), and this doc is the record of the rejected
  fixes.

---

## 8. Consequences to plan for — all RESOLVED (kept as the 2026-08-21 record)

- ~~**Fixing D1 exposes D2** (R7 / step 3c). These were found together because they
  cancel; they must be resolved together or the anchor breaks.~~ **Resolved
  2026-08-21** — §7 3c: D2 fixed in `classify_reads`, the anchor
  `tests/two_domain_hierarchy_cdc.rs` passes for the right reason.
- ~~**`probe_fsm` may become unexpressible.** It is a deliberate *investigation
  fixture* demonstrating a divergence. If the guardrail rejects it, that fixture
  stops compiling and `probe_timing_investigation.rs` goes with it. Decide whether
  the rule needs an escape hatch for study fixtures, or whether that investigation is
  now subsumed and can be retired.~~ **Resolved 2026-08-21** — §7 3a: the
  `allow_pretick_alignment` opt-out; `probe_fsm` carries it, stays in
  `EXPECTED_FLAGGED`, and `tests/probe_timing_investigation.rs::probe_fsm_sim_matches_verilog`
  stays `#[ignore]`d as the W4-class demonstration.
- ~~**Three `fast_counter` copies** live in two examples and one test. Migrating them
  changes `two_domain_counter.rs`'s printed timeline and its prose, which already
  mis-describes the latency decomposition.~~ **Resolved 2026-08-21** — §7 3b:
  migrated to the post-tick sticky update.

---

## 9. Open decisions — all DECIDED (kept as the 2026-08-21 record)

- ~~**Escape hatch or not?** `multi_write_collapse` has none — it points at `RegOut`.
  D1's legal form is a rewrite, not a type change, so there may be no equivalent
  "just use this instead" for every case.~~ **Decided 2026-08-21: yes** —
  `#[hardware(sequential, allow_pretick_alignment)]`, §7 3a; it silences the error,
  not the detection.
- ~~**D2's disposition** — its own guardrail, a codegen change, or accepted-and-
  documented? Unlike D1 there is no independent-hardware adjudication yet; getting
  one is the prerequisite.~~ **Decided 2026-08-21: fixed in the simulator** after
  adjudication against hand-written Verilog — §1 D2, §7 3c.
- ~~**Option (c) is back on the table.**~~ **MEASURED AND REJECTED as a blanket
  change — see §5.3.** Prost-style lowering fixes D1 exactly and breaks the
  barrier-protected majority exactly. The reason generalises: the simulator is not
  internally consistent, so *no* single lowering can match it. Both single-sided
  fixes are now measured and rejected (§5.1 sim-only, §5.3 codegen-only). What
  remains is (a) reject the shape, (d) make it unexpressible, or a *paired* fix that
  makes the sim uniform **and** matches codegen to it. **The paired fix was
  executed 2026-08-26** (`PAIRED_IMPLEMENTATION_SCOPE.md`, phases A/B/D; §5.7):
  forwarded emission for the opening-prefix sub-class where §5.3's own measurement
  showed it correct, and D1 narrowed to what no emission can match.
- ~~**A fourth option, from §10: make it unexpressible.** Give register locals the
  current/next distinction that `Out`/`RegOut` already gives ports — a `Reg<T>`
  with explicit read/write. This is what MyHDL, Chisel, Amaranth, Spade and
  Bluespec all do, and it dissolves the problem rather than detecting it. Cost: it
  changes the surface syntax for sequential state, which is a headline ergonomic
  claim.~~ **Not taken.** The 2026-08-26 dissolution (§5.7) made most of the class
  legal with the surface syntax unchanged; what remains is refused, not retyped.
- ~~**Is the deeper fix worth scoping separately?** §5.1's finding — that the pre-tick
  segment conflates next-state and Moore-output evaluation — describes an executor
  restructuring that would make the guardrail unnecessary. Out of scope here, but it
  is the principled fix and should be recorded as such rather than forgotten.~~
  **Superseded by the cycle-dataflow model** (`CYCLE_DATAFLOW_SEMANTICS.md`,
  `DERIVATION_TABLE.md`): the denotation is normative, the simulator is checked
  against it, and the remaining rules are derived from it rather than from the
  executor's phase machinery.

---

## 10. Prior art — how other HDLs handle this

Researched 2026-08-21. The field has converged on **two** answers, and Copper
currently has neither.

### 10.1 Structural prevention — separate "current value" from "next value"

Most modern HDLs make this class **unexpressible** rather than checking for it. In
each case reading a register yields its *current* value and writing goes to a
syntactically distinct *next*-value slot, so a mid-cycle assignment cannot be
forwarded to a same-cycle read:

| HDL | mechanism |
|---|---|
| **MyHDL** | read `sig` (current, read-only attr); write `sig.next` — documented as "the MyHDL equivalent of the VHDL signal assignment and the Verilog non-blocking assignment" |
| **Chisel / FIRRTL** | a `Reg`'s output *is* the current value; `:=` connects the next value, with last-connect semantics resolving multiple writes |
| **Amaranth** | `m.d.sync` vs `m.d.comb` domains; a signal is driven by exactly one domain and driving from two is an error; all `sync` assignments take effect at the edge |
| **Spade** | `reg(clk) name = expr` — the expression *is* the next value; registers are the only sequential element and are always explicit |
| **Bluespec** | rules are guarded atomic actions; register reads within a rule observe the value at rule start (cross-rule same-cycle forwarding is an explicit opt-in, not the default) |
| **Clash** | pure functions over streams; no mutable state to mis-order |

**Copper already has this axis — for ports.** `Out` vs `RegOut` is exactly the
current/next distinction, and it is what resolved the multi-write-around-a-tick
collapse. It just does not exist for *locals*: a register-classified local is an
ordinary Rust binding, so `r` means both "the flop's Q" and "the flop's D"
depending on where you read it.

### 10.2 Static lint — for languages where it *is* expressible

Verilog/SystemVerilog can express the hazard, so the ecosystem lints it:

- **Cummings, SNUG 2000, *Nonblocking Assignments in Verilog Synthesis, Coding
  Styles That Kill!*** — the canonical reference. Guideline #1: model sequential
  logic with non-blocking assignments; use blocking for combinational; do not mix
  the two in one `always` block.
- **Verilator**: `BLKSEQ` — "a blocking assignment (`=`) is used in a sequential
  block"; `COMBDLY` — "a delayed assignment inside of a combinatorial block";
  `BLKANDNBLK` — "a variable is driven by a mix of blocking and non-blocking
  assignments"; plus `MULTIDRIVEN`.
- **Verible**: `always-ff-non-blocking` — "use only non-blocking assignments inside
  `always_ff` sequential blocks"; no non-blocking in combinational logic.

### 10.3 Why Copper can't just copy the lint — and what that implies

Every one of those lints works by comparing an **author-written marker** (`=` vs
`<=`) against an **author-written block kind** (`always_comb` vs `always_ff`). The
tool never has to infer intent; it checks two declarations against each other.

Copper has **neither declaration**. Every register assignment is a plain Rust `=`,
and the "block kind" is implicit in position relative to `.await`. So Copper is
currently in the worst of the three positions:

- it has Verilog's *expressiveness* (the hazard is writable), but
- not Verilog's *marker* (so no syntactic check is possible — the rule must infer
  both sides, which is exactly why §5.2's rule over-flagged), and
- not the modern HDLs' *structural separation* (so the hazard exists at all).

**This adds a fourth option to §9** — and it is the one the field converged on:
give locals the same current/next distinction ports already have. A `Reg<T>` with
explicit read/write (`.get()` / `.set()`, or a `next` slot) would make D1
**unexpressible** instead of merely detected, in the same way `RegOut` did for the
multi-write collapse. Cost: it changes the surface syntax for sequential state,
which is a headline ergonomic claim ("a register is just a local live across an
await"). That trade is a real design decision, not an implementation detail.

### 10.4 A direct hint for Q1

Verible's rule is not "no blocking assignments in sequential logic" — it is that
**blocking assignments may target locals** in sequential logic. That is precisely
the distinction §3.4 was groping for: an assignment whose value never becomes
observable is harmless, which is why `mac_fsm`'s `Mul` arm is safe.

The catch is V7, where the register is never read again in its segment yet still
diverges — because it escapes *across the tick* (`s = r` post-tick, then `o.write(s)`).
So the right predicate is likely **"does this value become observable at all"**,
not "is it written to a port in this segment". That is a reachability question over
the CFG, and it is the concrete thing to test first in phase 1.

### 10.5 Prost — the closest prior art, and the only one that is *also* a coroutine HDL

[Riedl, Scheipel & Baunach, **LATTE '26**] proposes coroutines as the fundamental
abstraction of synchronous hardware — the same thesis as Copper's contribution 1,
arrived at independently (see the paper's Related Work section). Because it shares Copper's
substrate, it is the single most relevant data point here, and it answers three
questions at once.

**(a) It hit the same problem and solved it structurally.** Prost signals carry an
explicit current/next projection, with the semantics defined *in terms of the
suspension boundary*:

> "their current value can be accessed using the `.val` projection. Output signals
> can be modified using the `.next` projection, **which will affect all `.val`
> accesses after the next wait**."

That is §10.1's answer, restated for a coroutine language — and it is precisely the
`Out`/`RegOut` distinction Copper already has for ports. Note the asymmetry that
matters for D1: Prost applies `.val`/`.next` to **signals**, while **local variables
stay plain** ("Local variables correspond to registers … variable updates describe
the datapath"). So Prost does *not* solve D1 by typing locals — see (b).

**(b) Its lowering preserves forwarding, unlike Copper's.** Prost's synthesized
next-state logic (their Listing 2) is a **combinational block using blocking
assignments**:

```verilog
RESET: begin
  acc = 0; byte = 0; i = 0;
  cycles = 250;
  cycles = cycles - 1;      // blocking — forwards, cycles becomes 249
  next_state = STATE_0;
end
```

Local updates are computed combinationally *in coroutine order*, with the registers
taking the computed next values at the edge. **This is the sequential-forwarding
semantics Copper's simulator has** — and it is exactly what Copper's codegen does
*not* do, since it emits `r <= r + 1` non-blocking directly in `always_ff`.

So the two coroutine HDLs resolve the same ambiguity in opposite directions, and
Prost's direction is the one that preserves the source's meaning. **This is why §3.2
carries a correction and why option (c) is back on the table in §9.** It also
reframes D1: Copper's codegen may be the side that fails to preserve coroutine
semantics, rather than the simulator being "wrong". (This is the direction §5.7
eventually took, for the opening-prefix sub-class only.)

**(c) Its guardrail philosophy is the one to adopt.** Prost states the design
constraint for exactly this kind of check:

> "One requirement of the Prost compiler is to reject code that cannot be
> synthesized. Determining statically whether a program reaches a certain state is
> **undecidable in the general case** … Therefore, we envision the compiler using a
> **heuristic** instead, which must be **well-defined and computationally efficient
> while rejecting as few valid programs as possible**. Currently, the compiler
> requires each loop to contain at least one wait statement and to run for at least
> one iteration. Future work shall develop more precise heuristics and also formally
> verify their correctness."

Three things follow directly:

1. **Copper already implements Prost's one shipped guardrail.** "Each loop must
   contain at least one wait" *is* `check_reachability` (impl-plan item 2), enforced
   in both front-ends. Independent arrival at the same rule is worth citing.
2. **"Rejecting as few valid programs as possible" is R1**, stated by the prior art
   as a first-class design constraint rather than a nicety. The §5.2 rule failed
   exactly this test — `mac_fsm` is a valid program.
3. **A heuristic is the expected form, not a compromise.** Prost concedes the precise
   question is undecidable and plans heuristics plus later formal verification. That
   licenses Copper shipping a *sound-but-incomplete* rule — one that may miss cases
   (§4 Q5's multi-tick false negative) provided it never rejects a valid program —
   rather than holding out for an exact characterisation.

**Also relevant:** Prost lists "combinational cycles are currently not expressible"
and multi-clock as open — the same two frontiers Copper tracks (item 6's comb-loop
detection, item 4's multi-clock).

### 10.6 MyHDL — the convertible-subset discipline

`SYNCHRONOUS_SEMANTICS.md` already cites MyHDL's "restrict the synthesizable subset"
discipline as the precedent for the multi-write guardrail, and it applies here too:
MyHDL's converter *rejects* constructs outside its convertible subset rather than
silently emitting something that behaves differently. The G4 finding (recorded for
the paper's Related Work section) is the sharp version — MyHDL's **convertible** subset is
strictly larger than its **RTL-synthesizable** subset, and its multi-`yield`
cycle-slicing converts only to *behavioral*, non-synthesizable HDL. That is the same
boundary Copper is negotiating, with the difference that Copper claims the
multi-suspension shape *is* synthesizable and therefore owes a guardrail where MyHDL
simply declined the territory.

**Sources**
- [Cummings, *Nonblocking Assignments in Verilog Synthesis, Coding Styles That Kill!* (SNUG)](https://csg.csail.mit.edu/6.375/6_375_2009_www/papers/cummings-nonblocking-snug99.pdf)
- [Verilator — Errors and Warnings](https://verilator.org/guide/latest/warnings.html)
- [verible-verilog-lint rule list](https://umarcor.github.io/verible/verilog_lint.html)
- [MyHDL manual — Signals and `.next`](http://docs.myhdl.org/en/stable/manual/reference.html)
- [Chisel — Sequential Circuits](https://www.chisel-lang.org/docs/explanations/sequential-circuits)
- [Amaranth — Language guide (domains)](https://amaranth-lang.org/docs/amaranth/v0.4.3/lang.html)
- [Spade: An Expression-Based HDL With Pipelines](https://spade-lang.org/osda2023.pdf)
- [Bluespec — Rule Scheduling (UCSB course notes)](https://web.ece.ucsb.edu/its/bluespec/training/BSV/slides/Lec06_Scheduling.pdf)
- [Riedl, Scheipel & Baunach, *Prost! Coroutine-based Hardware Description*, LATTE '26](https://capra.cs.cornell.edu/latte26/paper/latte26-final31.pdf)
