# Pre-Tick Segment Alignment — Divergence Analysis and Guardrail Plan

> **Status (2026-08-21): D1 GUARDED, D2 FIXED.** Phases 0–3 complete, gates G0–G3
> met. D1 is a compile error (the language is restricted); D2 was adjudicated against
> independent hardware and fixed in the simulator at no corpus cost.** Three candidate fixes were tried and rejected with
> measured evidence (§5) before the shippable rule was found. This doc is both the
> plan and the record of what was ruled out and why.
>
> **Pinned by:** `tests/sequential_forwarding_divergence.rs` (4 tests, green; each
> flips loudly when the corresponding divergence is fixed).
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
> that pinned the old behaviour. A first attempt keyed on the *module* ("no registers
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

Same family, **complementary triggers** — neither rule subsumes the other. Verified:
`multi_write_collapse` returns `[]` for both the D1 minimal case and `fast_counter`,
while returning `["o"]` for the shape it targets.

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
12 of 77 clocked modules:

| module | copies | status | evidence |
|---|---|---|---|
| `fast_counter` | 3 | **TRUE POSITIVE** | measured divergence + hardware adjudication |
| `add_then_write` | 1 | **TRUE POSITIVE** | the V1 fixture itself |
| `mac_fsm` | 3 | **FALSE POSITIVE** — output is `RegOut` (Q1) | `mac_fsm_sim_matches_transpiled_verilog` passes |
| `if_tick_explicit` | 1 | **FALSE POSITIVE** — output is `RegOut` (Q1) | `if_tick_sim_matches_transpiled_verilog` passes |
| `probe_fsm` | 1 | **TRUE POSITIVE** — same defect as D1, confirmed (Q2) | a leading read on every path fixes it, exactly as it fixes V1; plain `Out` |
| `branch_merge_explicit` | 1 | **AGREES** — measured (phase 0a) | plain `Out`, but every write is a CONSTANT; produced the constant-write clause. An earlier revision wrongly recorded this as `RegOut` |
| `ram_prewrite` | 1 | **UNKNOWN** (plain `Out`, so the refined rule retains it) | `probe_mem_latency` is `#[ignore]`d — phase 0b stands |

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
- **Q5 — Should the rule cover every inter-tick segment, not just the pre-tick one?**
  The candidate examines only head → first tick, which is a known false negative for
  multi-tick loops (`accum_2` class).

---

## 5. Rejected approaches (do not re-try)

### 5.1 "Always-barrier" — REJECTED 2026-08-21

Inject `pre_edge_barrier()` unconditionally at the loop top so alignment stops being
incidental. Implemented behind `COPPER_ALWAYS_BARRIER` in `copper-macros`, measured,
reverted.

- **Fixes** D1's forwarding (V1 sim `[2,3,4…]` → `[1,2,3…]`, matching SV) and leaves
  the already-correct leading-read form alone.
- **Over-corrects Moore outputs.** The loop top then runs only at the pre-edge, so a
  Moore output shows the **pre**-edge register value where SV's `assign count_out =
  count` shows the post-edge one. `fast_counter`'s count went `[(1,0),(2,0)…]` →
  `[(0,0),(1,0)…]` — still ≠ the independent reference, just wrong elsewhere.
- **Corpus damage: 22 failures / 654**, and they are the modules that are currently
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
construction. Every false positive was a `RegOut` module. The corrected rule keys on
the output write, which is the same structure `multi_write_collapse` already uses.

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
- **R4 — Static, all-paths, compile-time.** Per the standing decision recorded for
  the multi-write work: static checks over dynamic ones. A dynamic backstop was
  already tried and dropped there for false-firing.
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

Sequenced, each phase gated on the previous. Phases 0–1 are pure measurement and
need no sign-off; phase 3 changes `copper-macros` behaviour and does.

### Phase 0 — Coverage prerequisites (no semantics change)

Two flagged modules cannot currently be adjudicated because nothing checks their
behaviour. Fix that first, or the corpus verdict stays partly unknown.

- **0a** ~~Behavioral equivalence test for `branch_merge_explicit`~~ **DISCHARGED
  2026-08-21** — measured sim-vs-Verilator: it **AGREES**. Note the trace is weak
  (its outputs are write-once `Logic::One` and saturate), but it was enough to
  establish it is not divergent, and it produced the constant-write clause.
- **0b** Resolve `ram_prewrite` / `probe_mem_latency` — un-ignore it, or record why
  it cannot be.
- **Gate G0:** every module the candidate rule flags has a behavioral verdict:
  diverges, or agrees.

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

### Phase 2 — Rule synthesis and offline validation — **DONE 2026-08-21**

**The rule, as landed** (`Cfg::unprotected_pretick_out_write`). Flag a plain
combinational `Out` port `P` when **both** hold in the pre-tick segment:

1. some node writes `P` **and reads a register** — a constant write is idempotent
   across the phase shift, so it is unobservable; and
2. some node assigns a register with **no `In` read comb-reaching it**
   (`leading_read_reaches`) — the barrier is what pins the segment's phase.

`RegOut` is excluded for free: `Node::writes` holds only combinational outputs, the
same way `multi_write_collapse` gets its exclusion.

**Every clause has a measured witness** (R3), each a unit test in `cfg.rs`:

| clause | witness | verdict |
|---|---|---|
| register assigned pre-tick, plain `Out` | V1 | DIVERGE → flag |
| no in-segment read-back needed | V7 | DIVERGE → flag |
| read must *precede* the assignment | V4 vs V5 | agree / DIVERGE |
| post-tick assignment is safe | V6 | agree → no flag |
| mixed alignment does **not** protect | W4 | DIVERGE → flag |
| `RegOut` is immune | W8, W9 | agree → no flag |
| **write must read a register** | `branch_merge_explicit` | agree → no flag |
| same defect as `probe_fsm` | W5/W6 | DIVERGE → flag |
| barrier-pinned corpus shape | `lfsr` | agree → no flag |

The constant-write clause was added *during* phase 2: `branch_merge_explicit` drives
three plain `Out`s from an unprotected path and was flagged by the first cut, so it
was measured (phase 0a, discharged) — it **agrees**, because every write is
`Logic::One`. Flagging it would have rejected a correct design.

- **Gate G2: MET.** `copper-analysis/tests/pretick_alignment_corpus.rs` scans 76
  clocked modules across `examples/`, `src/` and `tests/`, and flags **exactly 7** —
  the measured-divergent set, nothing else. The expectation is an *exact set*, so the
  test fails in both directions: a newly flagged module is a regression or a real bug,
  and a no-longer-flagged one means the divergence was fixed and several pinned tests
  need re-blessing.

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
  constant write).
- **Gate G3: MET** — `smoke.sh` green.

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
- **Gate G3:** `cargo test --workspace` green, `smoke.sh` `SMOKE OK`.

### Phase 4 — Documentation

- **4a** `SYNCHRONOUS_SEMANTICS.md` §Output timing gains this as the third member of
  the family.
- **4b** `paper/threats_to_validity.md` + contribution 5: report the divergence and
  the restriction, as the multi-write case is reported.
- **4c** Retire this doc to a status note, or move it to `OUTDATED/`.

---

## 8. Consequences to plan for

- **Fixing D1 exposes D2** (R7 / step 3c). These were found together because they
  cancel; they must be resolved together or the anchor breaks.
- **`probe_fsm` may become unexpressible.** It is a deliberate *investigation
  fixture* demonstrating a divergence. If the guardrail rejects it, that fixture
  stops compiling and `probe_timing_investigation.rs` goes with it. Decide whether
  the rule needs an escape hatch for study fixtures, or whether that investigation is
  now subsumed and can be retired.
- **Three `fast_counter` copies** live in two examples and one test. Migrating them
  changes `two_domain_counter.rs`'s printed timeline and its prose, which already
  mis-describes the latency decomposition.

---

## 9. Open decisions

- **Escape hatch or not?** `multi_write_collapse` has none — it points at `RegOut`.
  D1's legal form is a rewrite, not a type change, so there may be no equivalent
  "just use this instead" for every case.
- **D2's disposition** — its own guardrail, a codegen change, or accepted-and-
  documented? Unlike D1 there is no independent-hardware adjudication yet; getting
  one is the prerequisite.
- ~~**Option (c) is back on the table.**~~ **MEASURED AND REJECTED as a blanket
  change — see §5.3.** Prost-style lowering fixes D1 exactly and breaks the
  barrier-protected majority exactly. The reason generalises: the simulator is not
  internally consistent, so *no* single lowering can match it. Both single-sided
  fixes are now measured and rejected (§5.1 sim-only, §5.3 codegen-only). What
  remains is (a) reject the shape, (d) make it unexpressible, or a *paired* fix that
  makes the sim uniform **and** matches codegen to it.
- **A fourth option, from §10: make it unexpressible.** Give register locals the
  current/next distinction that `Out`/`RegOut` already gives ports — a `Reg<T>`
  with explicit read/write. This is what MyHDL, Chisel, Amaranth, Spade and
  Bluespec all do, and it dissolves the problem rather than detecting it. Cost: it
  changes the surface syntax for sequential state, which is a headline ergonomic
  claim.
- **Is the deeper fix worth scoping separately?** §5.1's finding — that the pre-tick
  segment conflates next-state and Moore-output evaluation — describes an executor
  restructuring that would make the guardrail unnecessary. Out of scope here, but it
  is the principled fix and should be recorded as such rather than forgotten.

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
arrived at independently (see `paper/related_work.md`). Because it shares Copper's
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
semantics, rather than the simulator being "wrong".

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
silently emitting something that behaves differently. The G4 finding
(`paper/related_work.md`) is the sharp version — MyHDL's **convertible** subset is
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
