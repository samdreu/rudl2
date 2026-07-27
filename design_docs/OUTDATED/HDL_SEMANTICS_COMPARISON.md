# How Other HDLs Handle the Register/Combinational Boundary — vs Copper

**Status:** reference (2026-07-24). Written during the output-timing
investigation (EXECUTION_MODEL_RECONCILIATION.md, REGISTERED_OUTPUTS.md) to
position Copper's semantics against prior and recent research HDLs. Focus: **how
each language expresses the clock/cycle boundary and the register-vs-combinational
distinction** — the exact axis where Copper's sim and transpiler diverged.

## The axis that matters

Copper's difficulty came from one place: **it *infers* what is a register and what
is combinational from control flow** (values that live across a `clk.tick().await`
become registers; an output's register-vs-combinational nature is inferred from
whether it's driven on all paths). The sim and transpiler infer it *differently*,
so they disagree on held outputs.

The question for every other HDL: **is the register boundary explicit or inferred,
and where does the "cycle" live?** The short answer, across all of them: **explicit.**
Copper is the outlier.

## Copper (baseline)

- **Cycle boundary:** `clk.tick().await` inside an `async fn` loop. Implicit — a
  tick is a suspension point; the code between ticks is a cycle's logic.
- **Registers:** *inferred* — any `let mut`/value that lives across a `.await`
  becomes a flip-flop.
- **Output register vs combinational:** *inferred* (transpiler:
  `conditional_output_ports`; sim: everything immediate). This is the divergence.
- **Verification:** simulator vs transpiled Verilog (equivalence harness).

## Spade — explicit `reg`, closest structural analog

Spade is a Rust-flavoured HDL where **all expressions are combinational and `reg`
is the *only* sequential statement**. `reg(clk)` declares a state register with an
explicit clock (and optional reset). In a `pipeline`, a `reg` statement **separates
pipeline stages** and the compiler inserts registers for every live variable above
it. Units are typed by capability: `fn` (combinational only), `pipeline` (may
register). ([Spade paper](https://spade-lang.org/fpl2022.pdf),
[expression-based HDL with pipelines](https://arxiv.org/pdf/2304.03079))

**Relation to Copper:** Spade's pipeline `reg` is almost exactly Copper's tick — a
stage boundary that registers all crossing values — but **explicit and named**.
Where Copper writes `clk.tick().await` and infers which values become registers,
Spade writes `reg;` and the boundary *is* the register declaration. A registered
output in Spade is simply one driven from a `reg`; there is no inference to get
wrong. Spade is the "make Copper's tick explicit" point in the design space.

## PDL — imperative stages, sequential ≡ pipeline

PDL describes pipelined processors as an **imperative sequence of stages**, with
`---` as the **stage separator that abstracts pipeline registers**; statements
within a stage are combinational. Its headline guarantee is **"one instruction at a
time" semantics**: the generated pipeline provably behaves like the *sequential*
specification, with the compiler synthesizing bypass/stall logic for hazards.
([PDL, PLDI 2022](https://dl.acm.org/doi/10.1145/3519939.3523455))

**Relation to Copper:** PDL is the closest *correctness model*. Copper wants the
same property — the transpiled hardware behaves like the (sequential) simulator —
but PDL **guarantees** it by construction, with an explicit stage separator (`---`
≈ `.await`) and a compiler that owns the timing. Copper instead *checks* it after
the fact (sim-vs-Verilator) and infers the boundaries. PDL shows that an
imperative-looking source can be given rigorous cycle semantics — but the stage
boundary is explicit and the register placement is the compiler's job, not
inferred from data lifetime.

## Esterel / Lustre — synchronous, no register/combinational output choice

Synchronous languages run in **atomic instants** (the synchrony hypothesis). In
Esterel, outputs are **signals you `emit`** — combinational functions of the
control state within the instant; there is **no registered-vs-combinational output
concept**. State comes from two explicit sources: the **control position** (each
`pause` ≈ a tick is a held point, encoded as state registers) and the **`pre`
operator**, which reads the previous instant's value and **compiles to shift
registers**. Lustre is the dataflow dual, with `pre` and `->` for delay/init.
([synchronous hypothesis](https://www.researchgate.net/publication/248029346_The_Synchronous_Hypothesis_and_Synchronous_Languages),
[Esterel primer](https://www.college-de-france.fr/media/gerard-berry/UPL8106359781114103786_Esterelv5_primer.pdf))

**Relation to Copper:** Esterel is Copper's *theoretical template* (`pause` = tick,
`emit` = `out.write`). Its lesson is the opposite of Spade's: instead of *marking*
registers, **dissolve** the register/combinational output distinction — output
timing is fully determined by *where the emit sits in the pause structure*, and any
extra delay is an explicit `pre`. The catch for Copper is that this only works with
a true atomic-instant executor (Copper's "compressed execution" violates instant
atomicity) and a transpiler aligned to emit-in-instant timing rather than
Verilog-Moore combinational-from-state.

## Chisel / Clash — explicit register constructs

Chisel (Scala) makes registers explicit objects: `Reg`, `RegNext`, `RegEnable`. An
output is registered iff driven from one (`io.out := RegNext(x)`); otherwise
combinational. Clash (Haskell) treats signals as streams and uses the explicit
`register` primitive (a one-cycle delay). Neither infers registers from control
flow.

**Relation to Copper:** these are the mainstream "explicit register value" point —
the same philosophy as the proposed `RegOut`, but attached to an internal value
rather than a port. Copper's ports-passed-into-an-async-fn model has no natural
internal `Reg` to attach to, which is why a *port-typed* marker (`RegOut`) is the
Copper-shaped version of the Chisel/Clash idea.

## Bluespec / Kôika — guarded atomic rules

Bluespec (and its formal core Kôika) model hardware as **guarded atomic actions
(rules)**. State is explicit `Reg`s; a rule reads registers and schedules updates;
a clock cycle is **one atomic firing of a scheduled set of enabled rules**, with
register updates committed at the cycle boundary. Register vs combinational is
never ambiguous — reading a `Reg` is the stored value, computed logic is
combinational, updates (`<=` in a rule) commit at the edge.

**Relation to Copper:** Bluespec's "commit register writes atomically at the cycle
boundary" is exactly the two-phase evaluate-then-commit discipline Copper's
executor lacks. But Bluespec makes state and the commit explicit (rules + `Reg`),
whereas Copper infers them. Kôika additionally has a *formal* semantics with a
verified compiler — the rigor Copper's reconciliation is reaching for informally.

## Filament — timing in the type system (static)

Filament attaches **timeline types** to signals: each value carries the interval of
clock cycles during which it is available/valid, and the type checker enforces that
producers and consumers line up. Latencies and registers are **exposed explicitly**
to the designer; timing is **static** (constant cycle counts), and the type system
statically rules out timing/reuse hazards.
([Modular Hardware Design with Timeline Types](https://arxiv.org/pdf/2304.10646))

**Relation to Copper:** Filament makes *when a value is valid* a first-class,
checked property. Copper's entire reconciliation effort has been manually
recovering exactly this information (when is `out` valid? when is a read sampled?).
Filament suggests the ambitious direction: **put the cycle-timing in the types** and
check it, instead of inferring and then reconciling. Downside: static-only (no
data-dependent timing), which Copper's `while`/uart cases need.

## Anvil — timing contracts, dynamic, type-checked

Anvil (ASPLOS 2026) is a timing-safe HDL whose type system **fully exposes the
difference between registers and signals** and captures the timing relationship
between *register mutations* and *signal usages*, statically preventing timing
hazards — the bugs that arise "when the stored values in registers are mutated
while dependent signals are expected to remain constant across cycles." Crucially,
its contracts are **parametric over abstract time points that can vary at runtime**,
so it expresses **dynamic** timing safely.
([Anvil](https://arxiv.org/abs/2503.19447))

**Relation to Copper:** Anvil is the state of the art on exactly Copper's problem —
"values that stay unchanged over multiple cycles" (held signals/registers) — and it
solves it by **making the register/signal distinction explicit and type-checked**,
including dynamic timing (which Filament can't). It is the strongest evidence that
the field's answer to Copper's ambiguity is *explicit + checked*, not inferred; and
it's the closest prior art to where Copper would go if it pushed timing into types
while keeping data-dependent control flow.

## Synthesis — where Copper sits

| HDL | cycle boundary | register/comb | output timing | dynamic timing |
|---|---|---|---|---|
| **Copper** | `.await` (implicit) | **inferred** | **inferred** (the bug) | yes (control flow) |
| Spade | `reg;` (explicit stage) | explicit `reg(clk)` | driven-from-`reg` | limited |
| PDL | `---` (explicit stage) | compiler-placed | seq ≡ pipeline | yes (threads) |
| Esterel/Lustre | `pause` (explicit) | none — structural + `pre` | emit-in-instant | yes (reactive) |
| Chisel/Clash | — | explicit `Reg`/`register` | driven-from-`Reg` | yes |
| Bluespec/Kôika | rule firing | explicit `Reg` | rule commit at edge | yes (guards) |
| Filament | events/types | explicit, in types | timeline type | **no** (static) |
| Anvil | events/types | explicit, in types | timing contract | yes (parametric) |

**Two robust conclusions:**

1. **Copper is the only HDL here that *infers* the register/combinational
   boundary.** Every other language makes it explicit — as a keyword (`reg`, `---`,
   `pause`, `pre`), a value construct (`Reg`, `register`), a rule discipline, or a
   type. Copper's sim-vs-transpiler divergence is a direct consequence of inferring
   it twice, differently. This is the strongest argument for the explicit
   `RegOut` direction (REGISTERED_OUTPUTS.md).

2. **The two coherent directions for Copper map onto two camps.** *Add an explicit
   marker* → the Spade/Chisel/Clash camp (`RegOut`, small, pragmatic). *Dissolve the
   distinction via a true synchronous model* → the Esterel/Lustre camp (bigger,
   requires an atomic-instant executor and an aligned transpiler). The recent
   research frontier (Filament, Anvil) is a third, more ambitious camp: **move
   cycle-timing into the type system and check it** — which would subsume the whole
   reconciliation, at the cost of a much larger language change, and (for Filament)
   loses the data-dependent timing Copper needs; Anvil keeps it but with a heavier
   type system.

## Positioning note (for the paper)

Copper's novelty is the **implicit** async/await surface — "write FSMs naturally."
That implicitness is precisely what none of these share, and precisely what makes
the register/combinational boundary ambiguous. The honest framing is that Copper
trades the explicitness those languages rely on for ergonomics, and must therefore
either (a) reintroduce a *minimal* explicit marker where inference is genuinely
ambiguous (registered outputs), or (b) invest in a synchronous/type-based semantics
that recovers the guarantees the explicit languages get for free.
