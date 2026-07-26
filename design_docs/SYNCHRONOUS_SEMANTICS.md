# A Synchronous Semantics for Copper (sketch)

**Status:** sketch (2026-07-24, branch `synchronous-semantics`). Purpose: decide
whether Copper's async surface admits a clean synchronous semantics from which
**same-source sim ≡ synth** is a theorem rather than a test suite — the paper's
core claim. This is a design/semantics sketch, not the final formalism.

> **SUPERSEDED convention (2026-07-25).** The "atomic-instant / pre-edge
> continuation" model described below was measured against independent hand-written
> Verilog and found to over-delay write-after-tick outputs by one cycle (a basic
> DFF `q<=d` did not match). The executor now uses **post-edge continuation** — a
> `clk.tick()` resolves in the post-edge settle, so a register clocked at edge N is
> observable in cycle N, matching a standard synchronous testbench. The primitive
> constructs (flip-flop, enabled register, synchronous-read RAM / `dual_port_ram`)
> now match hand-written Verilog. Held/registered *output ports* that need one more
> cycle use an explicit `RegOut` annotation. The register-vs-combinational output
> distinction is irreducible (no single phase is correct for both) — see
> **EXECUTOR_CONVENTION_EXPERIMENT.md** for the measured dual-convention result.
> The core same-source ≡ theorem goal below is unchanged; only the executor phase
> convention changed.

## The goal, stated as a theorem

Let a module `M` be a Copper `#[hardware(sequential)] async fn`. It has two
backends:

- `Sim[M]` — the trace produced by running `M`'s Rust coroutine in the executor.
- `Synth[M]` — the trace produced by the transpiled SystemVerilog under Verilator.

We want a denotational meaning `[[M]]` — a function from input streams to output
streams — such that:

> **Theorem (target).** For all input streams `i`, `Sim[M](i) = [[M]](i) =
> Synth[M](i)`.

If we have `[[·]]` and both backends provably realize it, equivalence is
*definitional*, and the whole sim-vs-transpiler reconciliation dissolves into "does
each backend implement `[[·]]`?". That is a far stronger paper claim than the
current "we Verilated a handful of examples and they matched."

## The model: instants (the synchronous hypothesis for Copper)

Execution is a sequence of **atomic instants** `0, 1, 2, …`, one per clock cycle.
`clk.tick().await` is the **only** boundary between instants; it is Esterel's
`pause`. The straight-line code executed between two consecutive ticks is one
instant's **reaction** and is **combinational** — within an instant, all reads,
writes, and computation are simultaneous and take "zero time" (the synchrony
hypothesis). Time advances only at a tick.

This is exactly the structure rustc already produces: the `async fn` lowers to a
state machine whose **suspend points are the ticks** and whose **fields are the
values live across a suspend point**. Copper's novelty is that this host-compiler
coroutine lowering *is* the synchronous automaton — we don't build it, we give it a
synchronous meaning.

## Meaning of each construct

Let `σ_n` be the **state** entering instant `n`: the pair
`(pc_n, r_n)` where `pc_n` is the control point (which tick the coroutine is
suspended at — Esterel's control-state registers) and `r_n` is the tuple of
variables live across that tick (the coroutine's captured fields — the datapath
registers). Both are flip-flops.

- **Read.** `p.read()` in instant `n` denotes the input `i_n(p)` — the port's value
  *in that instant*. (The read fix already enforces this; the old bug read across
  the instant boundary.)
- **Combinational compute.** Ordinary Rust between ticks is a pure function of
  `σ_n` and `i_n`, evaluated within the instant.
- **Write.** `q.write(v)` in instant `n` sets output `o_n(q) = v` for *that*
  instant — Esterel's `emit`. An output written on every path of an instant is a
  combinational function of `(σ_n, i_n)`. An output not written in some instant
  **holds** its previous value — which makes it *state* (a register), and forces
  it into `σ`.
- **Tick / state update.** At the tick ending instant `n`, the next state
  `σ_{n+1} = (pc_{n+1}, r_{n+1})` is committed from the values computed in instant
  `n` (the two-phase / NBA discipline: evaluate the whole instant with `σ_n`, then
  commit `σ_{n+1}`). This is the atomic evaluate-then-commit that the current
  executor lacks (its "compressed execution" leaks a reaction across the boundary).

So `[[M]]` is a Mealy machine `(σ_n, i_n) ↦ (o_n, σ_{n+1})` where `σ` is
`(control point, live vars, held outputs)`. Every construct has exactly one
meaning; nothing is inferred *differently* by two backends.

## The two backends as realizations

- **Synth.** The transpiler already produces a Mealy/Moore FSM: `σ` are the
  registers (`phase_r`, datapath regs, held-output regs), the reaction is the
  `always_comb`, the commit is the `always_ff`. Claim: it realizes `[[M]]` **if**
  its register-vs-combinational choices match the model (held outputs → register;
  all-driven outputs → combinational).
- **Sim.** The executor must run **exactly one reaction per `tick_clock`** with
  evaluate-then-commit — no compression. Claim: with that discipline it realizes
  `[[M]]`. The read fix is the input half of this; an analogous output/commit
  discipline is the rest.

The metatheorem then factors as two lemmas ("Synth realizes `[[·]]`" and "Sim
realizes `[[·]]`"), each a structural induction over the instant model — a clean,
paper-shaped proof obligation.

## The one hard decision the model *forces* (and why that's the point)

The instant model pins down every timing question — including the one that has
been silently answered *differently* by the two backends. Consider a Moore output
written **after** the tick:

```rust
loop { state = next(state, in.read()); clk.tick().await; out.write(f(state)); }
```

- The transpiler emits `assign out = f(state_r)` — combinational, so `out` appears
  in the **same cycle** as the registered state.
- The instant model puts the `out.write` in the instant **after** the tick that
  updated `state`, so `out` appears **one cycle later**.

These disagree by a cycle. Today this is hidden: the *compressed* sim happens to
match the transpiler here (both same-cycle), while for a pre-tick held output
(`mac_fsm`) they diverge instead. The synchronous model refuses to hide it: it says
`out.write` after a tick is in the next instant, full stop — so **either** the
model is amended (outputs are computed with the *pre-tick* state of their instant,
making post-tick Moore outputs same-cycle) **or** the transpiler registers such
outputs. Whichever we choose, we choose it **once**, and both backends follow.

This is precisely the value of the synchronous framing for the paper: the
register/combinational and post-/pre-tick timing questions become a small, closed
set of **semantic decisions made once**, instead of an open-ended reconciliation
between two inference engines that keep disagreeing. (Contrast: the empirical
finding that no *ad-hoc* executor tweak fixes both `mac_fsm` and `det_010` — see
EXECUTION_MODEL_RECONCILIATION.md — is exactly the symptom of *not* having pinned
this down.)

### Working proposal for the decision

Define the reaction of instant `n` to read state as `σ_n` (the state *entering* the
instant) and to compute the next state and all outputs from `(σ_n, i_n)`, with the
control point advancing as part of `σ`. Under this reading a post-tick Moore output
`out = f(state)` uses the state *entering* the emit's instant, which is the state
just committed by the previous tick — i.e. same-cycle as the transpiler's
`assign out = f(state_r)`. A *held* output (written on some instants only) is part
of `σ`, so it is a register (+1), matching the transpiler's implicit-hold. This
proposal makes the model agree with the transpiler on both `det_010` (combinational
Moore, same-cycle) and `mac_fsm` (held, registered) — to be verified construct by
construct against the independent-Verilog references we already built.

## Relationship to Esterel/Lustre, and the novelty

The model *is* the synchronous hypothesis (atomic instants, `pause`/`emit`, state =
control point + `pre`-style carried values). What is new is the **carrier**: Copper
does not define a bespoke language and compiler — it reuses **rustc's async→state
machine lowering as the synchronous automaton**, and gives *one literal source* a
synchronous meaning that is realized by both a cycle-accurate simulator and
synthesizable SystemVerilog, with equivalence following from the semantics. Esterel
is standalone; Spade/Chisel/Clash make the registers explicit; Filament/Anvil push
timing into types. Copper's claim is the **implicit host-coroutine surface + a
synchronous semantics + provable same-source sim ≡ synth**.

## What implementation this implies

1. **Executor:** one reaction per `tick_clock`, evaluate-then-commit (atomic
   instants). This subsumes the read fix and should fix `mac_fsm`, the `if_tick`
   write-collapse, and the mid-phase read together — *if* done as a true instant
   model (the naive "resolve ticks at pre-edge" prototype was not, and broke
   `det_010`; the working proposal above is the guard against that).
2. **Register/output classification:** made once, per the model (held → register),
   and shared by sim and transpiler — or made **explicit** via `RegOut`
   (REGISTERED_OUTPUTS.md) as an escape hatch where the model still leaves a
   choice. The two are compatible: `RegOut` is "the programmer names the decision";
   the synchronous model is "the semantics names it."
3. **Validation:** re-run every construct in the verified map
   (EXECUTION_MODEL_RECONCILIATION.md) against its independent-Verilog reference
   under the new executor; each is a witness for one lemma of the theorem.

## Validation of the working proposal (2026-07-25)

Walked each construct through the instant model and compared `[[M]]` to the
transpiler and the independent-Verilog references. Result: the model is
**consistent** but does **not** fully match the current transpiler.

| module | output kind | `[[M]]` | transpiler | agree |
|---|---|---|---|---|
| `mac_fsm` | pre-tick held | registered, cycle 2 | registered, cycle 2 | ✅ |
| `cond_sum` | conditional computed | registered (enabled FF) | registered | ✅ |
| `counter` | written every instant | combinational | combinational | ✅ |
| `probe` | held (every other instant) | `[0,10,10,12,…]` registered | `[10,10,12,…]` aliased | ❌ |
| `mac_pipeline` | held (stage 3 only) | registered (+1 cycle) | aliased | ❌ |
| `det_010` | every instant but the first | `[init, f(s₁),…]` | `[f(s₀),…]` | ❌ (startup) |

`[[probe]] = [0,10,10,12,…]` matches the **registered** reference `probe_hand.sv`,
not the transpiler.

**The finding:** the instant model gives a clean semantics only by making one
uniform choice — *an output not written in every instant is a register* (+1
latency). That **is** the register-vs-combinational decision; the model doesn't
dissolve it, it just defaults it to "register". On genuinely-held outputs
(`mac_fsm`, `cond_sum`) that matches the transpiler; on outputs whose value already
lives in a register (`probe`, `mac_pipeline`) the transpiler **aliases** (`assign
out = reg`, no extra latency) while the model **registers** (+1). They differ by a
cycle, and it costs latency (`mac_pipeline` becomes 4-cycle instead of 3).

Crucially, an "independent reference" does **not** settle which is right, because
*building* the reference re-makes the choice: `probe_hand.sv` was written `out <= x`
(registered) and so matches the model; written `assign out = x_r` it would match the
transpiler. The register-vs-combinational output choice is **irreducible and
reference-dependent** — the same conclusion the executor prototypes reached, now
confirmed inside the synchronous model itself.

**Consequence for the two directions:** they converge. Either (a) accept
**"held ⇒ registered everywhere"** — a single simple rule, the fully-dissolved
clean theorem, but re-baselines `probe`/`mac_pipeline` with added latency; or (b)
keep the synchronous model as the **foundation** and add a **minimal explicit
marker** (`RegOut`) for the one free choice the semantics can't make for the user
(register vs alias, i.e. latency).

### For the *simulator specifically*, (a) is forced — not a choice (2026-07-25)

A transpiler can `assign out = reg` and publish a value *before* the `out.write`
that produces it. **A simulator cannot** — it runs the coroutine, and an output
only takes a value in the instant where `out.write` actually executes. So a
faithful simulator produces the **registered** trace for every held output
(`probe` = `[0,10,10,12,…]`, verified against the registered Verilog reference),
and there is *no aliasing* available to it. The two demos (`probe`, `mac_pipeline`
aliased-vs-registered, run under Verilator) make the +1 concrete.

Therefore the correct **simulation** semantics is unambiguous and marker-free:

> **The sim shows each output value in the instant its `out.write` runs, held
> otherwise.**

The current sim's aliased-looking `probe`/`mac_pipeline` output is **not** a
semantic choice — it is the compressed-execution bug publishing a later instant's
write one `tick_clock` early. An atomic-instant executor produces the registered
trace by construction. `RegOut`/aliasing is a *transpiler* concern (later); it does
not exist at the sim level. The precise executor design is in
ATOMIC_INSTANT_EXECUTOR.md.

## Status — atomic model locked in (2026-07-25)

The atomic-instant executor is now the sim's semantics and the suite is
re-baselined to it (`cargo test --tests --lib` + `-p copper-sim` + `--examples`:
green, with the documented `#[ignore]`s below).

- **One-cycle-latency rule (canonical).** A value that crosses a `clk.tick().await`
  is observed one cycle later than the compressed sim used to show it. Concretely:
  a Moore output `out.write(state)` *after* the tick reads `[0,1,2,…]` (the state
  before this iteration's post-tick update), not `[1,2,3,…]`; a held output shows
  its value in the instant its `out.write` runs, held otherwise. This is the single
  rule behind every re-baselined trace.
- **`synced_read` is KEPT.** It is no longer needed for input read-timing (the
  atomic tick-at-pre-edge resolution subsumes that), but it still provides the
  no-tick-spin guard (`same_call` term) that prevents an infinite settle loop when a
  reaction reads without ticking. Removing it is a separate cleanup, not part of
  this lock-in.
- **`RegOut` is superseded at the sim level** (held ⇒ registered by construction);
  see REGISTERED_OUTPUTS.md. The primitive remains in `port.rs`, unused by the sim,
  retained only as possible input to transpiler alignment.
- **Ignored tests** are the transpiler-alignment-blocked equivalence tests plus the
  understood-divergence cases (`det_010` vs `det_010_awaits`, mid-phase `accum_2`,
  composition +1 re-times). Each carries a reason pointing here or to
  ATOMIC_INSTANT_EXECUTOR.md. Un-ignoring them is the deferred transpiler-timing
  alignment task.

## Open questions

- Does the "state entering the instant" reading (working proposal) actually make
  *all* constructs agree with the transpiler, or does it move some? Must check
  `probe`, `mac_pipeline`, `counter`, `det_010`, `mac_fsm`, `cond_sum`, `if_tick`
  one by one.
- Multi-tick-per-iteration loops and ticks-in-branches (control extraction): the
  control point `pc` already models these; confirm the reaction mapping is
  well-defined when ticks are nested in `if`/`while`.
- Data-dependent timing (`while cond { tick }`, uart): the instant model handles it
  (the control point is just data-dependent), which is where Filament (static-only)
  can't follow — a point in Copper's favor.
