# Synchronous semantics

The reference for Copper's execution/timing model. It states the semantics as properties of the
**design**, independent of any particular construction (codegen's `match pc` FSM is one realization,
not the semantics). Companion: `SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md` (the staged work); every claim
below is grounded in the code cited inline and exercised by the test suite.

## Execution model — c2 + "just Rust"

**The default simulator always executes plain Rust.** A `#[hardware]` async fn is run as-is by the
async executor (`copper-sim`); rustc's own coroutine lowering *is* the FSM. A shared compile-time
control/liveness analysis (`copper-analysis`, the CFG below) *informs* the sim — it supplies register
and read-timing facts the macro bakes into the generated code — but the sim never *interprets* an IR.
This is load-bearing: sim and transpiler are then two independent derivations from one source, which
is what makes their same-source equivalence non-circular (`paper/threats_to_validity.md` T6) and is
LEAD contribution #1. A CHIR interpreter may exist only as an optional **validation-only backend,
never the default** (impl-plan item 7). This "b vs c2" decision was settled 2026-07-29 against the
alternative of making the sim consume an FSM IR.

## The clock tick — phases

`HardwareExecutor::tick_clock` (`copper-sim/src/executor.rs`) drives one clock cycle through four
phases:

1. **Pre-edge settle** — phase `PreEdge`; `poll_tasks()` runs the combinational delta-cycle loop to a
   fixed point with the registers still holding the *previous* cycle's values. Pre-edge (leading)
   input reads sample here.
2. **Clock edge** — `clk.advance()`; registers commit their next-state values.
3. **Post-edge settle** — phase `PostEdge`; `poll_tasks()` settles again with the new register values.
4. **Post-edge observation** — after `tick_clock` returns, a testbench reads the settled post-edge
   values.

**Post-edge continuation convention.** A `clk.tick()` future resolves in the *post-edge* settle, so a
reaction's post-tick code runs in the **same** `tick_clock`, after the advance. A register clocked at
edge N is therefore observable in cycle N — the standard synchronous-testbench convention — so
Copper's primitives (flip-flop `q <= d`, enabled register, synchronous-read RAM) match hand-written
Verilog cycle-for-cycle. This is validated against independent BaseJump STL hardware
(`examples/basejump/`), not against the transpiler.

**Per-domain phase keying.** The phase, tick-resolution, and read-timing signals are keyed *per clock
domain* (`set_poll_phase::<Domain>`, `set_tick_resolving::<Domain>`; impl-plan item 1). Ticking one
clock cannot perturb another domain's tasks — the prerequisite for the multi-clock interleave
independence below.

## Cycle boundaries and FSM states

Each `clk.tick().await` is a **clock-cycle boundary**; a **clock cycle** is a maximal tick-free region
of an execution. A **suspension point** (a program point at which a coroutine can be paused across a
boundary) is an **FSM state**. **Every value live across a boundary is a register** (made precise by
the liveness rule under *The CFG model*). These are the four informal invariants — boundary,
FSM-state, register, and *every loop path reaches a tick* (reachability well-formedness, below) — now
each a checked property rather than an accident of construction.

### Trailing statements — the segment after the last tick

Decided 2026-08-25; it follows from the definition above rather than adding to it. In

```text
loop { A; clk.tick().await; B; clk.tick().await; C; }
```

`C` and the *next* iteration's `A` sit in one maximal tick-free region — falling off
the end of the body and re-entering it costs no clock — so **`C` is in the head's
cycle**. Three consequences, all now implemented on every lowering path:

* **`C`'s combinational logic lowers into phase 0**, the head's phase. The single-tick
  path always did this (there the trailing segment *is* the one phase) and
  `control_extract`'s path accepts it (`uart/rx`'s trailing `rx_dv.write(Zero)`, sim ≡
  SV); the multi-tick path used to **refuse** it, so the same source was accepted or
  rejected depending on whether an unrelated part of the module triggered extraction.
* **`C` is emitted before `A`**, matching execution order: the simulator runs `C` in
  the post-edge settle of the tick that opens the cycle and `A` in the pre-edge settle
  of the tick that closes it. Unobservable through ports — `multi_write_collapse`
  rejects a port written on both sides of a tick — but observable for a plain local
  flowing from `C` into `A`.
* **`C`'s register updates commit at the last tick's edge**, which is the edge that
  *opens* `C`'s cycle. That is the post-edge continuation convention above ("a register
  clocked at edge N is observable in cycle N"), and it is why moving a register update
  after the tick is D1's sanctioned remedy. Because those updates and the preceding
  phase's commit at the same edge, they **share a forwarding map**: `r = a.read(); tick;
  w = r + 1;` must emit `w` from the *new* `r`, since the simulator computes it after
  the edge. (Both lowering paths claimed this in a comment and neither did it, until a
  differential case caught it.)

A value flowing from `C` into `A` is still a **register**, by the liveness rule's
back-edge clause — verified on `sync_2ff`, where `ff2 = ff1` in the trailing segment
must be a flop or the two synchronizer stages collapse into one. Same cycle, but the
trailing statements execute at an instant just after the edge, and a wire would keep
tracking its inputs for the rest of that cycle.

**Startup.** An `Out` written *only* in `C` reads as its initial value until the
statement first runs, while the emitted continuous `assign` drives it from time 0 — so
the two differ for the first cycle or two and agree thereafter. A continuous assign has
no notion of "not yet written"; this is a property of an output first driven late, not
of the rule above.

## Output timing — `Out` vs `RegOut`

Two output-port kinds capture the Mealy/Moore distinction explicitly:

- **`Out<T,D>`** — a combinational (Mealy) output driven within the cycle (`assign out = …`). It
  reflects the current cycle's logic, with no added latency; an `Out` left unwritten on a path
  *holds* its value (a conditional write is an **enabled register**, verified `sim ≡ BaseJump` on
  `bsg_dff_en`).
- **`RegOut<T,D>`** — a registered (Moore) output driven from `always_ff`; its value commits at the
  clock edge, so it appears one cycle later. Use it for write-before-tick Moore outputs (verified on
  `sipo_block`). The two axes — input read timing and output register timing — are orthogonal.

**Multi-write-around-a-tick guardrail.** A combinational `Out` written on *both sides of one bare
`clk.tick().await`* within an iteration, after a *leading (deferred) input read*
(`… inp.read() …; out.write(x); clk.tick().await; out.write(y)`), is **rejected at compile time**.
The read shifts the pre-tick write into the pre-edge settle; the coroutine then runs the post-tick
write in the *same* `tick_clock`'s post-edge and clobbers the pre-tick value before it is observed —
a silent sim ≠ synth divergence (the synthesized combinational Mealy output is correct; the
simulator loses a cycle). The fix is to declare the port `RegOut` (registered/non-blocking, which
buffers and commits at the edge and reconciles) or to split the writes into distinct FSM states so
each state drives the output once. This is Copper's analogue of Verilog's blocking/non-blocking
(`=`/`<=`) choice, and follows MyHDL's discipline of restricting the synthesizable subset — so every
*accepted* program preserves sim ≡ synth. Detection is `copper_analysis::multi_write_collapse` (over
the shared CFG, top-level loop **and** nested ticking loops); it is precise (three necessary
conditions — bare tick, both-sides write, leading read comb-reaching the pre-tick write) and
corpus-clean. A single-write-per-cycle output, a `RegOut`, and a write-straddle *without* a leading
read (`counter`; `uart_rx`'s `rx_dv`) are all unaffected.

**Pre-tick alignment guardrail.** A plain combinational `Out` **driven from a register** in the
pre-tick segment is **rejected at compile time** when that same segment also **assigns a register
with no `In` read preceding it**. A leading read classifies `Deferred` and injects
`pre_edge_barrier()`, which parks the task at the barrier so the segment runs in the *pre-edge*
phase; with no such read the task parks at the tick instead, the segment for cycle *N+1* runs during
cycle *N*'s **post-edge settle**, and the post-edge observation of cycle *N* therefore sees *N+1*'s
value. Codegen emits a non-blocking `r <= …`, which no flip-flop can reproduce — measured,
`loop { r = r+1; o.write(r); tick; }` simulates `[2,3,4,…]` against the SV's `[1,2,3,…]`.

The remedies are `RegOut` (immune: it commits at the edge, so *when* the write executes is
unobservable) or moving the register update after the `clk.tick().await` so the pre-tick segment only
*reads* state. A module that exists to demonstrate the divergence opts out explicitly with
`#[hardware(sequential, allow_pretick_alignment)]` — the waiver every lint in this space ships
(Verilator's `lint_off BLKSEQ`, Verible's rule waivers); it silences the error, not the detection.

**Detection is four rules, not one.** The obvious consolidation — one rule examining every segment —
was implemented and measured **three times**, and rejected every time on corpus evidence: writing a
plain `Out` after a tick is the *ordinary* multi-phase pattern and is correct, so a rule that cannot
say which segment it is looking at drowns in false positives. What each rule may assume about its
region is what separates them.

- **`unprotected_pretick_out_write`** — the head segment (loop head → first tick). **NARROWED
  2026-08-26 (cycle-dataflow phase D)**: phase B's forwarded continuous-assign emission gave the
  no-read opening shapes their defined meaning (`assign o = r + 1` — the meaning the simulator
  always had; V1/V5/V7 and both `fast_counter` ports re-measured agreeing), so the register-reading
  clause now also requires the write to be **read-preceded**. What remains refused is the
  **path-dependent region boundary** — a read reaches the write on one path while a register is
  assigned unprotected on another (W4, `probe_fsm`: the write executes at the opening on one path
  and the pre-edge on the other, and no single emission matches both) — plus the unchanged
  conditional/constant hold clause below. `RegOut` remains immune.
- **`unprotected_trailing_out_write`** — the same hazard past the *last* tick, unguarded until
  2026-08-25 and measurably divergent (`trailing_update`, one cycle, uniformly). Two widenings were
  measured and rejected first: merging the trailing segment into the head region flags **25** modules
  — including `fast_counter_corrected`, the module D1's own remedy produces — and treating every
  trailing segment as its own region flags **10**, all single-tick memory modules like `rom_from_fn`,
  which structurally matches the divergent DUT and *agrees*. The discriminator, found by flipping
  exactly one thing, is **how many clock edges the body crosses per iteration**: with the identical
  trailing body `n = n + 1; o.write(n);`, `loop { tick; … }` agrees and
  `loop { for _ in 0..2 { tick } … }` diverges. In a single-tick loop the trailing statements *share
  the head's phase* — falling off the end and re-entering costs no cycle — so there is no separate
  region to misalign. Clause (i) does **not** carry over: the same DUT *with* a leading read still
  diverges, which also refutes the natural hypothesis that the loop-top barrier pins the whole
  iteration.
- **`multi_phase_out_write`** — a plain `Out` driven in **more than one clock phase** (a phase being
  a Comb-connected component of the CFG). This rule was already in the language: the multi-tick
  lowering refuses the shape — *"driven in more than one phase … hold it in a register"* — but only
  when it can **see** the phases, and control extraction hides them by rewriting a branch- or
  loop-nested body into a single-tick `match pc` FSM whose `pc` states *are* the phases it meant to
  count. Restating it on the source closes that blind spot. The instance that forced it: a one-cycle
  pulse, `loop { for _ in 0..3 { tick } dv.write(One); tick; dv.write(Zero); }`, found while writing
  the UART receiver's first sim-vs-Verilator test — one cycle late, uniformly, `RegOut` immune,
  measured both ways. Widening D1 to cover it instead flags **36 of 120** corpus modules, ~30 with
  passing equivalence tests (`det_010`, `mac_pipeline`, `dual_port_ram`, `bsg_dff_en`, every memory
  fixture); this rule flags 9, six of them its own synthetic witnesses.

**The constant-write exemption is narrower than it looks.** The misalignment changes *when* a write
happens, so it is observable only if the value written differs between the phases — which is why a
write of a constant was exempt. That premise holds only when the write happens on **every** path.
Where a port is written on some paths and not others (the enabled-`Out` idiom), or different arms
write different constants, the alternative is the port's *held* value, so *when* the write lands is
observable even though the value written never changes. Both D1 rules therefore flag a port that is
driven from a register **or** not written on all paths through the region. The witnesses are
`pc_arm_write` and `pc_arm_toggle`, whose traces are each other shifted by exactly one cycle — a
phase shift, not an initialization artifact — and `branch_merge_explicit`, which this doc previously
cited as *agreeing*: it does not. It transpiles byte-identically to its twin, its twin agrees with
it, and it leads its own emitted SystemVerilog by one. Narrowing the exemption cost exactly those
three modules corpus-wide.

- **`pretick_out_write_before_update`** (2026-08-26) — a plain `Out` written **between a leading
  `In` read and the update of a register the write reads**. The read's barrier parks the task at the
  read site, so the write executes in the pre-edge settle with the register's *pre-update* value —
  which the emitted `assign` (Q) never shows at any observation instant; the hardware leads the
  simulator by one cycle, silently. D1 structurally cannot see it: the leading read is D1's
  *protection* (its shapes write after the update, so the barrier hands them the committing value),
  and here it is the *exposure*. The first rule in the family **derived before it was measured** —
  from the cycle-dataflow model (`design_docs/CYCLE_DATAFLOW_SEMANTICS.md`), with the V8 battery
  confirming every predicted trace and the CPU sweep's `program_counter` divergence as the in-vivo
  instance. Zero corpus cost, and it caught a silently-divergent compile-only UI fixture the day it
  landed. Remedies: move the write after the update, or before the read, or `RegOut`.

Enforcement is asymmetric, and deliberately so: `multi_phase_out_write` runs in **both** front-ends
(the transpiler honours the opt-out too, or a module that exists to demonstrate the hazard could not
be measured against anything), while the two D1 rules run in the **macro** only.

**This is the third member of the blocking/non-blocking family**, after `Out`-hold semantics and the
multi-write collapse — and the three share a root cause worth stating plainly: **Copper infers the
register/combinational boundary where every other HDL makes it explicit.** MyHDL (`sig` vs
`sig.next`), Chisel (`Reg` vs `:=`), Amaranth (`m.d.sync` vs `m.d.comb`), Spade (`reg(clk) … = …`)
and Bluespec (atomic rules) all separate a register's *current* value from its *next* one
syntactically, so the hazard is unexpressible. Verilog leaves it expressible and lints it
(`BLKSEQ`) — but those lints compare an author-written **marker** (`=` vs `<=`) against an
author-written **block kind** (`always_comb` vs `always_ff`), two declarations checked against each
other. Copper has neither, which is why its rules must *infer* both sides. See
`design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md`.

**Passthrough reads are `Immediate` (the D2 fix, 2026-08-21).** A read that feeds a *combinational*
`Out` in a segment that assigns **no register** is classified `Immediate`, not `Deferred`, even when
a tick follows it. The barrier a `Deferred` read injects does two jobs — it defers the read *and*
pins the whole segment to the pre-edge phase. Pinning is essential when the segment updates a
register (that is the hazard above); when the segment assigns nothing there is nothing to pin, and
deferring a read that only feeds a wire makes that wire behave like a flop. A passthrough
(`loop { out.write(inp.read()); tick; }` → `assign out = inp;`) used to lag its clocked producer by
a cycle for exactly this reason.

Adjudicated against independent hand-written Verilog: a clocked producer feeding a passthrough gives
`mid == out` in hardware, and only the `Immediate` form reproduces that. The rule is deliberately
narrow — it does **not** apply to a read in a *condition*, because there the sampled value can decide
how many cycles elapse (`det_010_awaits` reads inside `while … { tick }`; `if_tick` picks a branch
with a different tick count), so its phase genuinely matters. A first attempt that keyed on the
module rather than the read broke exactly those two, one of them a hardware-anchored test.

## Input read timing — static edge-phase classification

An `In` read is classified **statically** by its position relative to ticks, replacing the retired
runtime freshness oracle (impl-plan item 3; `copper_analysis::classify_reads`):

- **`Deferred`** — a "leading"/pre-tick read (a clock tick follows it within the iteration). Its result
  is registered at that edge, so it samples at the **next pre-edge settle**. The macro emits
  `pre_edge_barrier::<D>().await` before the `.read()`.
- **`Immediate`** — a trailing/post-tick read (no tick follows before the iteration closes). It consumes
  the value the just-past edge produced and fires without deferral — a plain `.read()`.

At runtime there is then no timing heuristic, only the accepted phase machinery. The classification
reproduces the timing the old heuristic got right (loop-top reads in `mac_pipeline`/`sipo_block`
defer; the trailing next-state reads in `counter`/`traffic_light` fire immediately) and fixes the
class it got wrong (the variable-iteration `while in_i.read() == 0 { tick }` in `det_010_awaits`) —
anchored to the independent `pattern_detector_010.sv`.

**A repeating wait must test *before* its tick (2026-08-24).** In a nested wait loop, the ordering

```rust
loop { clk.tick().await; if ready.read() == Logic::One { break; } }   // REJECTED
loop { if ready.read() == Logic::One { break; } clk.tick().await; }   // the form to write
```

differs by more than style. The rejected ordering puts the read in the window *after* the entering
edge, where an `Immediate` read consumes the value the just-past edge produced while the flip-flop
the FSM lowers to samples the value present before *its own* edge. Under the harness convention
"drive, then clock" those are different values a cycle apart — measured, the transpiled module
reacted a full cycle earlier than the simulator, and holding each stimulus value for two cycles did
**not** reconcile them (the two models read in different *windows*, not at different points of one).

**Copper does not choose between the two samplings; it declines to let a design depend on which.**
Same disposition as the pre-tick alignment hazard: the divergent program is made unwritable rather
than the divergence adjudicated. The cost is low because the supported ordering expresses the same
designs, and because the divergence needs an input that changes *mid-cycle* — an `In` driven by a
clocked module in the same domain is stable across the window and both models agree. It is a
testbench-observable difference, which is exactly why it could not be left in: sim ≡ SV *under a
testbench* is this project's bar. Enforced in `control_extract` (which declines to flatten the
shape) and reported by `chir_lower`; see `TODO` for the rejected alternatives.

## Memory — staging and read latency

`Memory<T, R, W, D, READ_LAT, WRITE_LAT>` (`copper-core/src/memory.rs`) is a first-class multi-port
synchronous memory: `R` read ports, `W` write ports, latencies in cycles (`≥ 1`), a
`WriteMode::{ReadFirst, WriteFirst}` same-address collision policy and a `ReadMode::{Sync, Async}`
read policy. Both port kinds are fully pipelined — a new address or value may be presented every
cycle regardless of what is in flight.

**A read is *staged*, not returned.** `mem.read_port::<I>().read(addr)` presents an address; the
result appears **at the clock edge** and is observed after the tick with `is_ready()` / `data()`.
None of these are `async` — the `clk.tick().await` between them *is* the wait, which is what makes
memory latency a property of the design's cycle structure rather than of an extra await kind.

Four rules make the divergent shapes unwritable. All four live in `copper-analysis`, on the
**source**, for the reason given under *One analysis, both front-ends*: control extraction rewrites
branch- and loop-nested ticks into a single-tick `match pc` FSM, so a check downstream of it counts
one phase where the `pc` states *are* the phases it meant to count.

- **One access per bus per cycle** (`check_memory_staging`) — a physical port has a single address
  bus. Two accesses conflict iff one can reach the other **without crossing a clock edge**, not
  merely iff they share a phase. That distinction is the whole difference between a bus conflict and
  a multiplexer: `rv32i_cpu`'s seven regfile writebacks sit in exclusive `match` arms, no path joins
  any two, and each drives the bus in its own state — exactly what the emitted `always_comb` does.
  Counting them instead reported a design error where there is a mux.
- **Observe after the edge that produces it** (`check_memory_staging`) — reading `data()`/`is_ready()`
  with no `read()` staged earlier on that port (the port never becomes ready), or before the
  `clk.tick().await` that produces it, is a spanned compile error.
- **No access inside a tick-free nested loop** (`check_memory_staging`) — such a loop is unrolled, so
  every iteration's access lands in the same cycle on one address bus. Refused rather than counted
  as one.
- **No plain `Out` driven from a read result in a multi-phase module**
  (`memory_result_drives_plain_out`) — the read pipeline re-captures on every clock edge, so a plain
  `Out` wired to it either tracks the result into phases that do not observe it or latches one edge
  after the capture the simulator reads. Measured: a full cycle late on every sampled value. The
  remedies are `RegOut` or a register between the result and the port. A **single-phase** module is
  unaffected and deliberately so — there the post-tick segment shares the head's phase and a plain
  `Out` driven from `data()` is correct (`rom_direct`, pinned in
  `tests/multiphase_memory_equivalence.rs`).

## Poll-order and cross-domain interleave independence

**Single domain — poll-order independence.** `poll_tasks` order is an implementation detail: a
well-formed design simulates **bit-identically** under any task order. Enforced by the poll-order
fuzzer (`tests/poll_order_fuzz.rs`: `Insertion` ≡ `Reversed` ≡ `Seeded`). **Levelized scheduling
(impl-plan item 6, landed 2026-07-30) made the order canonical** and is the compiled default
(`SchedulerMode::env_default`, `copper-sim/src/executor.rs`; `COPPER_SCHEDULER` overrides it) — it
polls in a topological order and ignores `PollOrder` entirely. The fuzzer was **not** retired: it is
retained **pinned to `SchedulerMode::Fixpoint`**, because the fixpoint scheduler stays in-tree
permanently as the differential oracle levelized is validated against, and an oracle that became
order-dependent would silently cost that comparison its footing.

**Multiple domains — cross-domain interleave independence.** Independently-ticking clock domains have
no defined phase relationship. The generalized invariant: **a well-formed multi-clock design behaves
correctly under any relative tick interleaving/rate of its domains, provided every clock-domain
crossing goes through a synchronizer.** Two precise senses:

- For a *fixed* tick schedule, the result is poll-order-independent as above (per-domain phase keying
  is what decouples the domains).
- Across *different* relative rates, what is preserved is **functional correctness**, not the exact
  trace: a synchronized signal is monotone (no glitches), data is not corrupted, and events
  eventually propagate — while the exact cycle of an event legitimately shifts with the rate.

Worked example: `examples/cdc/two_domain_hierarchy.rs` checks this rate-independent CDC invariant
(monotone + eventually-asserts) across 2:1 / 3:1 / 1:1 interleavings, and
`tests/two_domain_hierarchy_cdc.rs` anchors the dual-clock timing to an independent hand-written SV
reference under a two-clock Verilator testbench. An **unsynchronized** crossing is rejected — by the
phantom domain types for compiled code, and by the transpiler's call-site CDC check for the
text-based path (impl-plan item 4).

## Allowed awaits

In `#[hardware]` code the only permitted `await` is a direct `clk.tick().await` (or a small set of
Copper-defined hardware waits). Every loop path must reach one — the reachability well-formedness
condition below, a hard compile error otherwise.

## The control-flow-graph (CFG) model

The informal invariants above are made precise as a **control-flow graph** over the module's
top-level `loop`. This is the authoritative model: it is what the shared analysis crate
`copper-analysis` builds (`Cfg`, `copper-analysis/src/cfg.rs`) and what **both** front-ends — the
`#[hardware]` macro and the transpiler `copper-codegen` — consume, so the simulator and the
synthesized netlist agree on registers and well-formedness *by construction*, not by two analyses
that happen to match.

The definitions are stated as properties of the **design**, independent of any particular FSM
*construction* (e.g. codegen's `match pc` flattening is one realization; it is not the semantics).

### The graph

A **sequential module** is an `async fn` whose body is a top-level `loop { … }`. Its CFG is:

- **Nodes** — program points at single-statement granularity, plus a distinguished **loop head**
  `H` (the point every iteration re-enters).
- **Edges**, of two kinds:
  - **`E_comb`** — ordinary same-cycle control flow (straight-line, `if`/`else`, `match` arms).
  - **`E_tick`** — the edge *out of* a `clk.tick().await`. It is a **clock-cycle boundary**, and
    is labeled with the **clock receiver identity** (which `Clock<D>` was ticked) — the hook a
    multi-clock design needs to tag *which* domain a boundary belongs to.
- Every "fall off the end of the body" and every trailing tick is a back-edge to `H`.

A **clock cycle** is a maximal `E_tick`-free region of an execution; each `clk.tick().await` ends
one cycle and begins the next. An **FSM state** is a program point at which execution can be
suspended across a boundary — i.e. a node with an incoming `E_tick` edge. (rustc's coroutine
lowering realizes exactly these states for the *simulation*; the semantics here define them
independently of that lowering — see `paper/threats_to_validity.md` T1.)

### Registers — the liveness rule

A local variable `v` is a **register** (a flip-flop; its value must survive a clock edge) iff:

1. `v` is **defined inside the loop** (a `let` binding or an assignment target within the body); and
2. `v` is **live across some `E_tick` edge** — its value produced on one side of a tick is read on
   the other before being overwritten.

Condition (2) is decided by a standard **backward-liveness** fixpoint over the CFG (edge kind is
irrelevant to liveness propagation; a value used after a tick is *live across* it —
`Cfg::registers`). Condition (1) is what distinguishes the two non-registers:

- a **pre-loop constant** read but never assigned in the loop (e.g. `lfsr`'s `xor_mask`) fails (1)
  — it is a combinational wire, even though it is trivially live across every tick;
- a **same-cycle combinational temp** (`let t = …; use t; tick`) fails (2) — it is redefined at the
  loop head before any post-tick use, so it is killed and does not cross the edge.

This is the **minimal synthesizable register set**, computed from control flow. It is deliberately
**not** read off rustc's `Future` layout, which is a conservative *superset* (rustc over-captures;
`paper/threats_to_validity.md` T1). The rule generalizes the earlier "pre-loop `let mut` reassigned
in the loop" heuristic to registers *born inside* the loop and live across an *interior* tick (e.g.
`mac_pipeline`'s pipeline registers `product`/`c_s`/`sum`) and to tuple-assignment targets
(`traffic_light`'s `(phase, timer)`).

### Reachability well-formedness

**Condition.** Delete every `E_tick` edge; the subgraph reachable from `H` must be **acyclic**.

Equivalently: **every cycle in the CFG crosses a tick**, so every path through the loop eventually
reaches `clk.tick().await`. A cycle that survives the deletion is a path that returns to the top of
the loop without ticking — a **zero-time combinational loop**, which would spin the simulator's
delta-settle forever. It is a hard, spanned **compile error** (a DFS back-edge check,
`Cfg::check_reachability`, enforced in both front-ends).

**Why shape-restriction soundness is not enough (worked example).** Before this analysis, the
invariant held only as an *accident* of the single-trailing-tick construction. Codegen's control
extraction is where the masked failure mode was visible: `lower_into`
(`copper-codegen/src/control_extract.rs`, the "fell through without ticking" case) once emitted
`pc = head` **unconditionally** when a branch reached the loop tail without ticking — silently
handing a zero-tick branch a *free phantom cycle* rather than rejecting it. The CFG check makes
"reaches a tick on every path" a real, checked property instead of a construction accident.

Two independent changes closed that hole. The reachability guard runs *before* control extraction,
so the fall-through is unreachable for malformed input; and the fall-through itself no longer
guesses — it discriminates the two cases that look identical at the call site (`FallThrough`) and are
a clock cycle apart. **`AfterTick`**: the body's own trailing tick was removed by the rotation that
builds a nested loop's head state, so the fall-through *stands for* that tick and `pc = head` is
exactly right. **`ZeroTime`**: nothing was removed, the source genuinely returns to the head in the
same cycle, and a `goto` marker is emitted instead — `pc = head` would spend a cycle the program does
not have. A real instance was caught in the wild: `det_010_awaits`'s
`else if` chain had no final `else`, so holding `rstn=1, in_i=1` spins with zero ticks — flagged by
the check and since fixed. Legitimate designs with *uneven* per-branch tick counts still pass
(every path still crosses *a* tick), which is the property shape-restriction could not express.

### Nested loops

A nested `for`/`while`/`loop` that contains a tick is handled two ways at once, both
rejection-sound: (i) in the **parent** graph it is folded into a single boundary node — a possible
0-iteration exit must not make the *outer* loop look tickless (so designs that only tick inside a
`for`/`while`, e.g. `uart_tx`, stay well-formed); and (ii) its body is *also* built as a real
sub-CFG (`Builder::nested_loop_cfg`, with `break`→exit and `continue`→head modeled) on which the
reachability condition is enforced **recursively** — so a tickless cycle *inside* a nested loop
(e.g. `loop { loop { if c { tick } } }`) is rejected. A tick-free nested loop is combinational
(unrolled) and is not subject to the must-tick rule.

### Combinational loops — the other graph

The CFG above is **intra-module control flow**. A combinational loop lives in a different graph, and
the distinction is not bookkeeping: *within* a module a comb cycle is unexpressible (Rust rejects the
use-before-def), so every real one is **cross-module** and structurally invisible to the CFG. It is
found instead in the **inter-module producer→consumer wiring DAG** that the levelized scheduler
builds.

`HardwareExecutor::comb_cycles()` (`copper-sim/src/executor.rs`) returns each strongly-connected
component of size ≥ 2 in that combinational dependency graph — mutual plain-`Out` feedback with no
`RegOut`, memory, or synchronizer to break it (each of those commits at the clock edge and so induces
no combinational edge). It is **static**: it inspects the wiring without running the simulation, and
is empty for a well-formed acyclic design.

**Detection is not rejection.** A *convergent* combinational loop — a set-dominant latch, say — is
legal hardware, and the levelized settle simulates it by iterating that component to a fixpoint
(`iterate_scc`) while walking the SCCs in topological order, so the acyclic remainder is unaffected.
Only a *non-convergent* loop fails, and only when the settle actually fails to converge: a component
still dirty after `OSCILLATION_THRESHOLD` passes panics, naming the whole SCC. This is the one place
the semantics are enforced at runtime rather than at compile time, because convergence is not a
property the wiring graph alone can decide.

### One analysis, both front-ends (the c2 architecture)

The CFG is keyed off `syn::ItemFn` — the representation both front-ends already hold — so it is a
single authoritative pass, not two that must agree. Its register output is validated (a) against
**independent hand-written SystemVerilog** (structural reg-for-reg match, `mac_fsm`/`det_010`/
`det_110101`/`lfsr`) and (b) against the **transpiler's own emitted flip-flops**
(`copper-codegen/tests/register_reconciliation.rs`: codegen ≡ this set + only its synthesized
phase/pc counter, corpus-wide). See `SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md` item 2.

## How these semantics are checked

Three mechanisms, deliberately independent of each other:

- **Independent hand-written Verilog.** `examples/basejump/` checks Copper modules against
  third-party BaseJump STL Verilog, and the hand-written goldens (`pattern_detector_010.sv`,
  `mac_fsm.sv`) anchor the rest. This is load-bearing: the shared analysis makes the simulator and
  the transpiler agree on registers and well-formedness *by construction*, so an independent
  reference is the only thing left that can catch a **timing** bug. Never the transpiler's own
  output — that is circular.
- **The corpus differential sweep.** `build.rs` generates one case per `#[hardware]` module in
  `tests/fixtures/` and `examples/` into `tests/corpus_generated.rs` — seeded random stimulus, the
  simulator versus the SystemVerilog that module transpiles to, under Verilator. A new module is
  therefore covered the moment it exists, with no harness to write; what cannot be inferred lives in
  three reviewed tables (`PARAMS` widths, `RESET`, `SKIP`), and `tools/regression.sh`'s **G-D** guard
  asserts the sweep covered the corpus and ran. Its first run found two defects that had been in the
  tree for weeks and were unreachable from the existing tests — a measured sim ≠ synth divergence in
  `branch_merge_explicit`, and emitted SystemVerilog that would not parse (a port named `event`, a
  keyword missing from the legalizer). See `design_docs/CORPUS_DIFFERENTIAL_SWEEP.md`.
- **The two schedulers.** `tests/levelized_differential.rs` steps a design under both
  `SchedulerMode::Levelized` and `SchedulerMode::Fixpoint` in lockstep and asserts **every wire holds
  an identical value after every phase of every cycle**, so a divergence is localized to the exact
  design, cycle, and phase. Its designs are chosen to span the topologies the levelized DAG must get
  right (combinational chain, fan-out/fan-in, plain-`Out` register feeding combinational logic, a
  `RegOut` pipeline, an independent two-clock design); corpus *breadth* under the levelized scheduler
  comes from `tests/golden_traces.rs` running the frozen Verilator-matched goldens under it as well.
  Fixpoint is retained permanently for exactly this reason: a scheduler change must be provably
  behavior-neutral, not argued to be.
