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
under Verilator. Critically, the simulator's cycle-level timing is itself **anchored to
independent, hand-written SystemVerilog references** for the primitive sequential constructs
(flip-flop, enabled register, synchronous-read RAM), so "cycle-accurate" means *matches
hardware* rather than *matches our own transpiler* — the same-source equivalence is a
correctness property, not a self-consistency check.

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

2. **A same-source correspondence between simulation and synthesis, checked by construction —
   and anchored to hardware.** The identical Rust source is executed for cycle-accurate
   simulation and transpiled to SystemVerilog, and we verify behavioral equivalence under
   Verilator. To keep this from being circular (two Copper artifacts agreeing with each other),
   the simulator's timing is *separately* validated against **independent, third-party
   SystemVerilog** — modules from the **BaseJump STL** hardware library, checked against both
   BaseJump's own module sources *and* the parameters/stimulus from BaseJump's own testbenches
   — as well as hand-written references for the primitive constructs (flip-flop `q <= d`,
   enabled register, synchronous-read block RAM). Because the reference DUTs and the test
   vectors both come from a third party, the equivalence is anchored to hardware neither we nor
   our transpiler authored. `[Evidence: sim≡BaseJump-Verilog in examples/basejump/ —
   bsg_dff_en, bsg_mux_one_hot, bsg_counter_up_down, bsg_encode_one_hot, bsg_gray_to_binary,
   bsg_adder_one_hot; sim≡hand-written-SV in examples/memory/dual_port_ram.rs,
   tests/timing_probe_investigation.rs, tests/mem_latency_probe.rs; sim≡transpiler in
   tests/*_equivalence.rs. TODO: expand the BaseJump set and the transpiler-verified set.]`

5. **A minimal, provably-necessary output-timing annotation.** Control-flow inference resolves
   register-vs-combinational timing for *internal* state (contribution 1), but we show — by a
   dual-convention executor experiment against hand-written Verilog — that no single global
   scheduling choice can make *output-port* timing correct for both registered and combinational
   outputs simultaneously: the two conventions are exact duals. Copper therefore infers output
   timing where it is derivable and requires exactly one explicit annotation (`RegOut`) at the
   provably-ambiguous boundary. `[Evidence: design_docs/EXECUTOR_CONVENTION_EXPERIMENT.md.]`

3. **Ownership-enforced single-driver and phantom-typed clock domains** as lightweight,
   pass-free structural guarantees discharged entirely by the Rust compiler. We position clock-
   domain typing relative to Clash/Arch (shared mechanism) and distinguish our move-based
   single-driver guarantee from Arch's dependency-graph analysis.

4. **A staged transpilation pipeline (FIR → CHIR → SHIR → VLIR → SystemVerilog)** that lowers
   Rust `async` hardware modules to Verilator-lint-clean SystemVerilog, with the Copper
   simulator — itself validated against independent hand-written Verilog (contribution 2) — as
   the semantic reference. `[Evidence: copper-codegen. Scope: single clock domain,
   flat modules, current example feature set — see TRANSPILATION_ROADMAP.md.]`

## Claims explicitly NOT made (guardrails)
- Not "the first HDL to type clock domains" (Clash, Arch precede us).
- Not "ownership-based CDC" (CDC is phantom-typed; ownership gives single-driver).
- Not "unified sim/synth" as a novelty (table stakes); the novelty is the *verified same-source
  correspondence*.
- No timing-safety guarantee in the Anvil sense; no pipeline-composition typing in the Filament sense.
- Not "zero timing annotations." We infer register-vs-combinational timing for internal state,
  but output-port timing requires one explicit annotation (`RegOut`) — and we argue that
  annotation is *minimal and provably necessary*, not a limitation (contribution 5). Framing the
  annotation as a characterized boundary is stronger and safer than a zero-annotation superlative.

## Open items before this is submittable
- [x] **MyHDL prior-art boundary — VERIFIED (2026-07-29), boundary holds.** Confirmed against the
      MyHDL 0.11 manual: the *convertible* subset is explicitly broader than the *RTL-synthesis*
      subset, and MyHDL's synthesizable sequential/FSM idiom is a single-edge `always_seq`/`always`
      process over an explicitly enumerated `enum` state (a hand-written FSM). Multi-`yield`
      cycle-sliced generators are convertible-only (to behavioral HDL), not RTL-synthesizable. So
      MyHDL does not offer the cycle-sliced coroutine algorithm as a *synthesizable* surface.
- [ ] **RE-SCOPE contribution 1 for Prost (LATTE '26) — NEW, load-bearing.** The "no async-based
      synthesizable Rust HDL" check surfaced **Prost** (Riedl/Scheipel/Baunach, LATTE '26), which
      independently proposes coroutines-as-synthesizable-FSM with locals=registers,
      suspension=cycle, procedural multi-cycle algorithm → Verilog, Rust-`async`-syntax-inspired,
      and the same loop-must-wait well-formedness rule. **The coroutine-as-FSM idea is thus no
      longer uniquely ours.** Contribution 1 must be re-worded so its novelty is the *realization*:
      embedded in Rust reusing `rustc`'s own `async` lowering (no bespoke compiler) + verified
      same-source sim/synth equivalence + third-party hardware anchoring — none of which Prost has
      (it is a bespoke language + compiler, a 3-page vision paper, no eval). Prost is now cited and
      distinguished in `related_work.md`; DECISION NEEDED on the exact contribution-1 phrasing.
- [ ] Expand transpiler coverage and the equivalence-verified example set (bounds the eval).
- [ ] Decide whether to add a soundness/semantics argument for the async-lowering ↔ transpiler
      correspondence, or lean on empirical equivalence (reviewer risk either way).
- [ ] Pick the target venue framing (PLDI-style PL contribution vs. systems/CAD-style artifact).
- [ ] Reconcile README/design-doc terminology with the code before an artifact evaluation.
