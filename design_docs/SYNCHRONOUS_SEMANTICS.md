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

Detection is `copper_analysis::unprotected_pretick_out_write`. Each clause has a measured witness:
the read must *precede* the assignment (a trailing read does not protect); mixed alignment does
**not** protect (a read on one branch leaves another branch exposed); `RegOut` is immune (changing
*only* the port type flips a diverging module to agreeing); and the write must read a **register** —
a constant write is idempotent across the phase shift, which is why `branch_merge_explicit`, driving
three plain `Out`s from an unprotected path, agrees. Known false negative: only the pre-tick segment
is examined, so the multi-tick `accum_2` class is not caught.

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

## Poll-order and cross-domain interleave independence

**Single domain — poll-order independence.** `poll_tasks` order is an implementation detail: a
well-formed design simulates **bit-identically** under any task order. Enforced by the poll-order
fuzzer (`tests/poll_order_fuzz.rs`: `Insertion` ≡ `Reversed` ≡ `Seeded`). Item 6 will make the order
canonical (levelized scheduling), retiring the fuzzer.

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
extraction still shows the failure mode it masks: `control_extract.rs::lower_into` (the "fell
through without ticking" case, ~line 232) emits `pc = 0` **unconditionally** when a branch reaches
the loop tail without
ticking — silently handing a zero-tick branch a *free phantom cycle* rather than rejecting it. The
CFG check makes "reaches a tick on every path" a real, checked property instead of a construction
accident; the reachability guard runs *before* control extraction, so that fall-through is now
unreachable for malformed input. A real instance was caught in the wild: `det_010_awaits`'s
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

### One analysis, both front-ends (the c2 architecture)

The CFG is keyed off `syn::ItemFn` — the representation both front-ends already hold — so it is a
single authoritative pass, not two that must agree. Its register output is validated (a) against
**independent hand-written SystemVerilog** (structural reg-for-reg match, `mac_fsm`/`det_010`/
`det_110101`/`lfsr`) and (b) against the **transpiler's own emitted flip-flops**
(`copper-codegen/tests/register_reconciliation.rs`: codegen ≡ this set + only its synthesized
phase/pc counter, corpus-wide). See `SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md` item 2.

