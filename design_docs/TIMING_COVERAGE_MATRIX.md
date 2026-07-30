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
| 5 | **CDC / synchronizer latency** | `two_domain_counter`, `flag_crossing` | **No** — self-consistency only (`flag_crossing`: *"no Verilog reference"*) | ⚠️ **gap** |
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

**5 · CDC / synchronizer latency — GAP (secondary).** `two_domain_counter` and
`flag_crossing` are checked only for self-consistency (the sim's recorded run
replayed against itself); `flag_crossing.rs` states outright there is *no Verilog
reference*. There is no independent hardware golden for synchronizer latency or
for a dual-clock crossing. **Disposition:** this gap is owned by **item 4**
(hierarchical clocked submodule / multi-clock), whose own verification section
already calls for "an independent hand-written async-FIFO Verilog reference" and a
clock-interleave fuzzer. It is *not* on item 3's critical path, so it does not
block the read-timing work — but it must be filled before any multi-clock timing
claim is made.

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

5 of 6 timing patterns are anchored to independent hardware; pattern 6's gap is
closed by this work for the canonical coding and made measurable for the
variable-iteration coding. The one remaining gap — **CDC / synchronizer latency
(pattern 5)** — is deferred by design to item 4's multi-clock verification. G1's
correctness precondition for **item 3** (the read-timing retirement) is therefore
satisfied: the timing pattern item 3 touches has an independent hardware golden in
place before the refactor.
