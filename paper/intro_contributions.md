# Introduction & Contributions (draft)

> Claims here are limited to what the code supports (see `00_claims_audit.md`). No "first HDL"
> or "ownership-based CDC" phrasing. `[VERIFY]` / `[TODO]` mark gaps to close before submission.

## Introduction (draft prose)

Hardware description languages inherited their core abstractions from an era before modern
type systems. Verilog and VHDL let a designer write a state machine as a hand-enumerated set
of states and a case statement over the current state, a clock domain as an untyped wire, and
a register as a coding convention that a linter hopefully catches when violated. Whole classes
of bugs — inferred latches, multiply-driven nets, unsynchronized clock-domain crossings — are
not language errors but review findings, discovered late and often only after synthesis.

A recent line of HDLs embedded in or inspired by typed programming languages (Chisel, Spade,
Clash, Arch, Anvil) has shown that many of these bugs can instead be *typing* errors. Copper
extends this direction by asking a sharper question: **can the high-level control constructs a
software engineer already uses — `async`/`await` — describe sequential hardware directly, and
can the very same source be both simulated and synthesized, provably in agreement?**

Copper is a hardware description language embedded in Rust. A sequential module is an ordinary
`async fn`; a clock edge is `clk.tick().await`; the state that must survive an edge is exactly
the set of local variables live across that `.await`. Crucially, Copper does not implement a
state-machine compiler for this: Rust's own `async` lowering transforms the function into a
`Future` whose fields are the live-across-await variables — that generated state machine *is*
the FSM, and its fields *are* the registers. The same source text is compiled by `rustc` into
a cycle-accurate simulation and, independently, lowered by Copper's transpiler
(FIR → CHIR → SHIR → VLIR) to SystemVerilog; the two are checked for behavioral equivalence
under Verilator.

On top of this, Copper uses Rust's type and ownership systems for structural safety. Clock
domains are phantom type parameters (`Clock<D>`, `In<T,D>`, `Out<T,D>`), so a signal produced
in one domain cannot be consumed in another without going through an explicit synchronizer —
a plain type error, following the approach established by Clash and Arch. Output ports are
move-only (`Out<T,D>` is non-`Clone`), so Rust's borrow checker guarantees every wire has
exactly one driver, with no separate analysis pass.

## Contributions (draft list)

1. **`async`/`await` as an FSM description surface for hardware.** We show that a general-
   purpose language's coroutine lowering can serve directly as a hardware state-machine
   encoding: variables live across `clk.tick().await` become the registers of the synthesized
   FSM. `[Evidence: traffic_light_fsm, uart/rx; macro is validation-only — copper-macros/src/lib.rs.]`

2. **A same-source correspondence between simulation and synthesis, checked by construction.**
   The identical Rust source is executed for cycle-accurate simulation and transpiled to
   SystemVerilog, and we verify behavioral equivalence under Verilator with an automated
   harness. `[Evidence: tests/lfsr_equivalence.rs, tests/m1_counter_equivalence.rs. TODO: expand example set.]`

3. **Ownership-enforced single-driver and phantom-typed clock domains** as lightweight,
   pass-free structural guarantees discharged entirely by the Rust compiler. We position clock-
   domain typing relative to Clash/Arch (shared mechanism) and distinguish our move-based
   single-driver guarantee from Arch's dependency-graph analysis.

4. **A staged transpilation pipeline (FIR → CHIR → SHIR → VLIR → SystemVerilog)** that lowers
   Rust `async` hardware modules to Verilator-lint-clean SystemVerilog, with the Copper
   simulator as the semantic reference. `[Evidence: copper-codegen. Scope: single clock domain,
   flat modules, current example feature set — see TRANSPILATION_ROADMAP.md.]`

## Claims explicitly NOT made (guardrails)
- Not "the first HDL to type clock domains" (Clash, Arch precede us).
- Not "ownership-based CDC" (CDC is phantom-typed; ownership gives single-driver).
- Not "unified sim/synth" as a novelty (table stakes); the novelty is the *verified same-source
  correspondence*.
- No timing-safety guarantee in the Anvil sense; no pipeline-composition typing in the Filament sense.

## Open items before this is submittable
- [ ] Expand transpiler coverage and the equivalence-verified example set (bounds the eval).
- [ ] Decide whether to add a soundness/semantics argument for the async-lowering ↔ transpiler
      correspondence, or lean on empirical equivalence (reviewer risk either way).
- [ ] Pick the target venue framing (PLDI-style PL contribution vs. systems/CAD-style artifact).
- [ ] Reconcile README/design-doc terminology with the code before an artifact evaluation.
