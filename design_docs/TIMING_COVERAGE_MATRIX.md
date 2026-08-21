# Timing-Pattern Coverage Matrix (gate G1)

> Groundwork for `SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md` gate **G1**. Establishes,
> per timing pattern, whether Copper's simulation semantics are anchored to an
> **independent hardware golden** — hand-written or third-party Verilog run under
> Verilator — as opposed to a transpiler-circular or self-consistency-only check.
>
> **Why this matters (from the plan):** the c2 refactor makes sim-vs-transpiler
> *timing* agree *by construction*. Once that lands, a sim/transpiler equivalence
> test can no longer catch a timing bug — an independent hardware golden becomes
> the *only* thing that can. So every timing pattern the semantics work touches
> must have one *before* the refactor (item 3), or its correctness rests on
> self-assertion. This matrix is the audit of where that holds.

## What counts as an independent anchor

| Kind of check | Harness | Independent of transpiler? | Adjudicates timing? |
|---|---|---|---|
| Sim trace vs **hand-written / third-party Verilog** under Verilator | `HardwareTest::with_verilog(<.sv>)` (examples, `det_010_independent_golden`) | **Yes** | **Yes** |
| Sim trace vs **transpiler-generated** Verilog under Verilator + a Rust model | `tests/common::EquivalenceTest` | No (circular for timing) | No — datapath only |
| Sim trace vs **itself** (recorded run replayed) | `HardwareTest` + `finish_with_expected(self)` | n/a | No (self-consistency) |

Only the first row anchors a *timing* claim. The independent goldens live in
`examples/**/sv/*.sv` (BaseJump-derived and textbook) and
`examples/basejump/sv/*.sv` (third-party BaseJump STL, Solderpad-licensed). The
`EquivalenceTest` cross-check is still valuable — it catches transpiler datapath
bugs — it just cannot referee *timing*, by construction.

## The matrix

| # | Timing pattern | Exercised by | Independent hardware golden? | Status |
|---|---|---|---|---|
| 1 | **Multi-tick FSM** (≥1 tick per loop iter / `match pc`) | `sipo_block` (4-word deserializer); `mac_fsm`/`mac_pipeline` (3-stage) | **Yes** — `examples/basejump/sv/sipo_block.sv` (BaseJump), LIVE, passes | ✅ anchored |
| 2 | **Mid-phase read** (read whose result crosses a tick within the iteration) | `sipo_block` (words 1–3 sampled after a tick) | **Yes** — `sipo_block.sv`, LIVE, passes | ✅ anchored |
| 3 | **Moore-`RegOut` vs Mealy-`Out`** (registered-output timing axis) | RegOut: `bsg_counter_up_down`, `bsg_dff_en`; comb-out: `det_110101`, `det_010`, combinational examples | **Yes** — BaseJump registered goldens + textbook comb-output goldens, all LIVE | ✅ anchored |
| 4 | **Memory read latency** | `dual_port_ram` | **Yes** — `examples/memory/sv/dual_port_ram.sv` (independent template), LIVE | ✅ anchored |
| 5 | **CDC / synchronizer latency** | `sync_2ff` (stdlib, isolated); `two_domain_hierarchy` (dual-clock hierarchy) | **Yes** — `examples/cdc/sv/sync_2ff_ref.sv` + `examples/cdc/sv/two_domain_hierarchy.sv`, both LIVE, pass | ✅ **anchored (closed 2026-08-21)** |
| 6 | **Variable-iteration loop** (`det_010` shape; data-dependent tick count) | `det_010` (Moore) vs `det_010_awaits` (sliced) | **Was NO** (two codings vs each other). **Now YES** — see below | ✅ **anchored (this work)** |

## Detail per pattern

**1–2 · Multi-tick FSM + mid-phase read — anchored by `sipo_block`.**
`examples/basejump/sipo_block.rs` is a serial-in / parallel-out deserializer that
samples one input word per cycle across a multi-tick block and presents them in
parallel; words 1–3 are read *after* a tick ("mid-phase"). It runs cycle-by-cycle
against the independent BaseJump-distilled golden `sipo_block.sv` under Verilator
and **passes**. This is the plan's *empirical status* result: the mid-phase *read*
timing is correct in the current sim, against third-party hardware. `mac_fsm`/
`mac_pipeline` add a second multi-tick witness but are checked with
`EquivalenceTest` (transpiler-circular); a hand-written `mac_fsm.sv` exists in
`tests/fixtures/timing_probe_sv/` but is currently only exercised by the
`#[ignore]`d `timing_probe_investigation` — a *dormant* independent golden that
could be promoted to a live check cheaply.

**3 · Moore-`RegOut` vs Mealy-`Out`.** Both output-timing axes have live
independent goldens: registered outputs via BaseJump `bsg_counter_up_down` /
`bsg_dff_en`, combinational/Moore-comb outputs via `det_110101`, the new `det_010`,
and the combinational suite. (The RegOut lowering itself was landed and anchored
in commit `75f9c26`/`1d13296`.)

**4 · Memory read latency — anchored by `dual_port_ram`** against an independent
dual-port block-RAM template `.sv`, LIVE. (`ram1.sv` in the timing-probe fixtures
is a second, currently-dormant golden behind the `#[ignore]`d `mem_latency_probe`.)

**5 · CDC / synchronizer latency — anchored (closed 2026-08-21).** Two live
independent goldens now cover this pattern at both scales:

- *The primitive, isolated* — `examples/cdc/sv/sync_2ff_ref.sv` is a hand-written
  textbook two-flip-flop synchronizer, checked against the Copper simulator running
  the **standard-library** `copper::sync_2ff` in `tests/cdc_synchronizer_anchor.rs`.
  This was the real hole: the library primitive is the only sanctioned way to cross
  a domain, and it had no `cargo test` coverage at all — the examples exercised it
  only under `cargo run --example`, and `two_domain_hierarchy_cdc.rs` used a private
  copy of the body rather than the library module.
- *A whole dual-clock hierarchy* — `examples/cdc/sv/two_domain_hierarchy.sv`, landed
  with item 4 and checked by `tests/two_domain_hierarchy_cdc.rs`. It anchors the
  crossing *in context* but measures it through a counter and a consumer, so the
  synchronizer's own latency was inferred rather than observed.

**Measured result:** the primitive's observable latency is **one destination cycle**
(a `d` standing at destination edge *n* appears on `q` after edge *n+1*), it is
independent of the source tick rate (checked at 1:1 / 2:1 / 3:1 / 5:1 / 8:1), and a
pulse that rises and falls entirely between two destination edges is dropped. The
independent Verilog agrees on all of it. Note this does **not** contradict the
"two destination cycles" figure in `examples/cdc/two_domain_counter.rs`'s prose:
that counts the full path from a fast-domain *register's* output, which includes the
producer's own registered delay.

**Gap surfaced while closing this one (open, separate).** The shared
`copper_analysis::infer_registers` reports **one** flip-flop for the 2-FF
synchronizer, where the simulator's behaviour, the independent Verilog, and codegen
all have two — `ff2` is assigned post-tick and read pre-tick, so no def→use path
crosses a tick edge and the "live across a tick" rule classifies it as a wire.
`register_reconciliation.rs` filters on `#[hardware(sequential)]`, so synchronizers
were never reconciled and nothing caught it. A corpus sweep with that filter lifted
measured the blast radius as *exactly* this shape: 41 modules checked, the only
divergences are the three copies of the same 2-FF synchronizer. It is pinned by
`tests/cdc_synchronizer_anchor.rs::register_inference_under_reports_the_second_flop_known_gap`
and affects no landed behaviour today (codegen computes its own set), but it is an
under-approximation of a set the design docs call authoritative.

**6 · Variable-iteration loop (`det_010`) — the glaring gap, now filled.**
`examples/sequential/pattern_detector_2.rs` has two codings of an "010" detector:
`det_010` (canonical single-tick Moore FSM) and `det_010_awaits` (a *sliced
algorithm* whose `while in_i.read() == 0 { tick }` ticks a **data-dependent**
number of times). The only prior check
(`det_010_variants_match_transition_table`) compared these **two Copper codings
against each other** — it cannot say which is correct, and it is `#[ignore]`d
because they diverge.

## What this work added (the G1 det_010 golden)

- **`examples/sequential/sv/pattern_detector_010.sv`** — an independent,
  hand-written 4-state Moore "010" detector (`out` combinational from a registered
  state; synchronous active-low reset), transcribed directly from the canonical
  transition table, *not* from transpiler output. Same shape as `pattern_detector.sv`.
- **`tests/det_010_independent_golden.rs`** — anchors both codings to that golden
  under Verilator:
  - `det_010_matches_independent_verilog` — **LIVE, passes.** The canonical Moore
    coding matches the independent hardware cycle-by-cycle. The "010" detector now
    has a real hardware referee.
  - `det_010_awaits_matches_independent_verilog` — **`#[ignore]`d.** Empirically it
    diverges from the golden at the **repeat detections** (cycles 9 and 13 of the
    coverage stream): the golden asserts `out=1`, the variable-iteration coding
    records `0` — it **misses the 2nd and 3rd detections**. The golden sides with
    the canonical Moore semantics.

**Consequence for item 3.** The variable-iteration divergence is no longer a
Copper-vs-Copper curiosity — it is now measured against independent hardware, and
the hardware says the canonical semantics are correct. Retiring the runtime
`synced_read` heuristic in favour of CFG-derived static timing (item 3) must make
`det_010_awaits` match `pattern_detector_010.sv`. Un-ignoring
`det_010_awaits_matches_independent_verilog` is that item's provable,
hardware-anchored claim (gate G5's per-item claim for item 3). Whether the fix is
purely read-timing or the sliced coding also needs its overlap logic corrected is
now an empirically decidable question rather than an argued one.

## Summary

**All 6 timing patterns are now anchored to independent hardware.** Pattern 6 was
closed by the `det_010` golden (above); pattern 5 — the last remaining gap — was
closed on 2026-08-21 by `examples/cdc/sv/sync_2ff_ref.sv` and
`tests/cdc_synchronizer_anchor.rs`. G1's correctness precondition for **item 3** (the
read-timing retirement) was already satisfied before that work, since item 3 does not
touch pattern 5; what pattern 5's closure buys is that the **multi-clock** timing
claims now rest on an outside referee rather than on self-assertion.

One *analysis* gap remains open, surfaced by that work and tracked under pattern 5
above: register inference under-reports the synchronizer's second flop.
