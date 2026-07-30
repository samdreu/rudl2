pre-edge settle
clock edge
post-edge settle
post-edge observation
Include examples for:

simple counter,
combinational passthrough,
register with enable,
two clk.tick().awaits,
Out vs RegOut,
memory read timing.


Every clk.tick().await is a clock-cycle boundary; every suspension point becomes an FSM state; every value live across an await becomes a register; every path through a hardware loop must eventually reach an await; and simulation must be independent of Rust async poll order.

Hardware timing must be defined by Copper’s FSM/cycle semantics, not by Rust future poll order.

poll_tasks order must be an implementation detail.
Well-formed Copper designs must simulate identically under any task order.

In #[hardware] code, the only allowed await should be direct clk.tick().await
or a small set of Copper-defined hardware waits such as channel.read().await.

Be careful about non-blocking channels and FIFOs

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

## Maybe?? — RESOLVED 2026-07-29 (see SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md)
> These were tentative musings toward making the **simulator consume FSM IR** (option b). That
> direction was **decided against**: the chosen architecture is **c2 + "just Rust"** — the default
> simulator always *executes plain Rust*; a shared CFG analysis *informs* it (register/timing
> facts) but the sim never *interprets* an IR. Reason: sim/transpiler independence is load-bearing
> (it is what makes the same-source equivalence non-circular — paper T6 — and is LEAD contribution
> #1). A CHIR interpreter may exist only as an optional **validation-only backend, never the
> default** (SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md item 5). The lines below are kept for history; read
> "FSM IR is the semantics" as *the FSM/cycle semantics are the reference the transpiler targets*,
> NOT as *the sim executes an IR*.

Rust async syntax is frontend notation.
FSM IR is the semantic core.
Simulator and Verilog backend both consume FSM IR.

On tick:
    state_reg and data_regs commit
    output combinational logic for the new state settles
    observations see the settled post-edge outputs

Might want to add
- blocking reads
- nonblocking reads

Async syntax is the frontend; FSM IR is the semantics.

