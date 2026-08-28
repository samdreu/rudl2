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
extends this direction by asking a sharper question: **if the high-level control constructs a
software engineer already uses — `async`/`await` — can describe sequential hardware, how much
compiler does that actually take?** Our answer is: none of your own. A general-purpose
compiler's existing coroutine lowering is enough, and the same source can then be both simulated
and synthesized, provably in agreement.

Copper is a hardware description language embedded in Rust. A sequential module is an ordinary
`async fn`; a clock edge is `clk.tick().await`; the state that must survive an edge is the set
of local variables live across that `.await`. The identification of coroutines with synchronous
circuits is not ours alone — Prost [LATTE '26] proposes it independently — so the question we
take up is not whether it can be done but what it costs to do: Prost builds a bespoke language
and compiler, and we show none is needed. Crucially, Copper does not implement a state-machine
compiler for *simulation*: Rust's own `async` lowering already transforms the
function into a `Future` whose state advances one clock cycle per `.await`, and Copper's
executor runs that coroutine directly as a cycle-accurate FSM. The synthesizable register set —
the values that must survive the edge — is a *property of the design*, computed by Copper's own
liveness analysis; it is deliberately **not** read off rustc's `Future` layout, which is a
conservative superset (retained-but-unread state that affects future *size* but not simulation
behavior; see §Threats T1). The same source text is then lowered **independently** by Copper's
transpiler (FIR → CHIR → SHIR → VLIR) to a SystemVerilog state machine, and the two are checked
for behavioral equivalence under Verilator — the transpiler *reconstructs* the FSM from the
source rather than reusing rustc's coroutine, which is precisely what makes the agreement a
cross-check between two independent derivations rather than a tautology. That check covers the
subset the transpiler supports; designs using `Memory`, array-typed ports, or the other
constructs tabulated in §Scope have no transpiled artifact to agree with, and we say so rather
than let the reader generalize. Critically, the
simulator's cycle-level timing is itself **anchored to independent, hand-written SystemVerilog
references** for the primitive sequential constructs (flip-flop, enabled register,
synchronous-read RAM), so "cycle-accurate" means *matches hardware* rather than *matches our own
transpiler* — the same-source equivalence is a correctness property, not a self-consistency
check.

On top of this, Copper uses Rust's type and ownership systems for structural safety. Clock
domains are phantom type parameters (`Clock<D>`, `In<T,D>`, `Out<T,D>`), so a signal produced
in one domain cannot be consumed in another without going through an explicit synchronizer —
a plain type error, following the approach established by Clash and Arch. Output ports are
move-only (`Out<T,D>` is non-`Clone`), so Rust's borrow checker guarantees every wire has
exactly one driver, with no separate analysis pass.

## Contributions (draft list)

1. **A general-purpose compiler's coroutine lowering is sufficient to describe synchronous
   hardware — no bespoke HDL compiler is required.** Coroutines as a synthesizable FSM surface
   has been proposed independently (Prost, LATTE '26), so the *idea* is not ours to claim. What
   we show is that realising it needs no new compiler: Copper is embedded in Rust and runs
   `rustc`'s own `async` lowering as the cycle-accurate simulation, with suspension points as
   clock edges and the values live across them as the design's registers. Prost is
   Rust-`async`-*inspired* but builds a bespoke language and compiler; the distinction is not
   cosmetic, because in Copper the coroutine transform is performed by a compiler that knows
   nothing about hardware and was not modified to accommodate it. The `#[hardware]` macro
   validates and injects a simulation barrier — it does not implement the state machine.

   Two scope boundaries keep this honest: (i) the *synthesizable* register set is refined by
   Copper's own liveness analysis, **not** read off rustc's over-capturing `Future` layout
   (§Threats T1); and (ii) the *synthesized* SystemVerilog FSM is produced by Copper's transpiler
   as an independent lowering (contribution 2), **not** by reusing rustc's coroutine. rustc's
   lowering is thus load-bearing for the hardware-anchored *reference simulation* and the semantic
   correspondence — not for the emitted netlist.

   A third boundary is worth stating because it cuts against us: reusing a general-purpose
   coroutine transform means inheriting a register/combinational boundary that is *inferred*
   rather than declared, and contribution 5 reports what that costs. The claim is that the reuse
   is sufficient, not that it is free.
   `[Evidence: traffic_light_fsm, uart/rx; the macro is validation-only —
   copper-macros/src/lib.rs. Prost cited and distinguished in related_work.md; the specific
   daylight is (a) embedded reuse vs bespoke compiler, (b) verified same-source equivalence and
   third-party hardware anchoring, neither of which Prost has (3-page vision paper, no
   implementation or evaluation).]`

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
   our transpiler authored. `[Evidence — CORRECTED 2026-08-22, two citations withdrawn:
   sim≡BaseJump-Verilog in examples/basejump/ — bsg_dff_en, bsg_mux_one_hot,
   bsg_counter_up_down, bsg_encode_one_hot, bsg_gray_to_binary, bsg_adder_one_hot, sipo_block;
   sim≡hand-written-SV in examples/memory/dual_port_ram.rs (with_verilog + assert_passed),
   examples/cdc/sv/sync_2ff_ref.sv and two_domain_hierarchy.sv via tests/cdc_synchronizer_anchor.rs
   and tests/two_domain_hierarchy_cdc.rs, and tests/det_010_independent_golden.rs;
   sim≡transpiler in tests/*_equivalence.rs.
   WITHDRAWN: tests/timing_probe_investigation.rs and tests/mem_latency_probe.rs were cited here
   as hand-written-SV evidence. They contain NO assertions — they are `eprintln!` diagnostics —
   and both are `#[ignore]`d, so they neither check anything nor run. Citing them overstated the
   evidence base. Do not re-add them without giving them assertions.
   SCOPE (updated 2026-08-27): the old subset bound is gone — every example module transpiles
   (34/34), `Memory` and `asr` included, and the pipelined RV32I CPU is itself an equivalence
   result (00_claims_audit.md §Re-verification; Threats T7 re-scoped). The remaining bound is
   language design (refused-by-construction shapes), plus the one operator gap `/`.
   TODO: expand the BaseJump set.]`

5. **A minimal, provably-necessary output-timing annotation — with a compile-time boundary on the
   residual.** Control-flow inference resolves register-vs-combinational timing for *internal* state
   (contribution 1), but we show — by a dual-convention executor experiment against hand-written
   Verilog — that no single global scheduling choice can make *output-port* timing correct for both
   registered and combinational outputs simultaneously: the two conventions are exact duals. This is
   the blocking/non-blocking (`=`/`<=`) distinction that event-driven HDL simulators require the
   author to write explicitly (see §Simulation semantics); Copper's `Out`/`RegOut` pair is its
   rediscovery — `Out` ≈ `=`, `RegOut` ≈ `<=`, each Verilator-verified against the corresponding
   `assign` / `always_ff`. Copper therefore infers output timing where derivable and requires
   exactly one explicit annotation (`RegOut`) at the provably-ambiguous boundary. The one shape that
   *neither* inference nor a single annotation makes sim-observable — a combinational `Out` written
   on both sides of one tick in a single region, which the coroutine executor collapses because it
   advances no observable time between the writes — is **rejected at compile time** rather than
   silently mis-simulated, exactly as MyHDL restricts its synthesizable subset to single-edge
   processes. Every accepted program thus preserves same-source sim ≡ synth.

   **REVISED 2026-08-22 — it is not one shape, it is a family of three, and that is the more
   honest and more interesting claim.** Two further members were found by measurement, and they
   resolved in *opposite* directions, which is what makes the family worth reporting rather than
   embarrassing:

   - **`Out`-hold semantics** — a sequential `Out` left unwritten on a path *holds*, making a
     conditional write an enabled register (verified `sim ≡ BaseJump` on `bsg_dff_en`).
   - **Multi-write around a tick** — rejected at compile time, as above.
   - **Pre-tick alignment** — a plain `Out` driven from a register in a pre-tick segment that also
     assigns a register with no preceding input read. The barrier that a leading read installs pins
     the segment's clock phase; with no read the segment runs a phase early and the observation is
     a cycle off. Measured `[2,3,4,…]` in simulation against the netlist's `[1,2,3,…]`, and
     adjudicated against an independent hand-written reference that sided with the netlist. Also
     **rejected at compile time**, pointing at `RegOut` or a post-tick update
     (`copper_analysis::unprotected_pretick_out_write`).
   - **A fourth was FIXED rather than restricted** — a combinational passthrough of a
     post-edge-produced signal lagged a cycle because its leading read was deferred unnecessarily.
     Adjudicated against independent Verilog, then fixed in the simulator at no corpus cost. Worth
     stating: not every member of this family has to be paid for by narrowing the language.

   **The claim to make is causal, not incidental.** These are not three unrelated bugs; they are
   the predictable bill for the one thing Copper does that no comparison HDL does — *inferring*
   the register/combinational boundary rather than declaring it. MyHDL (`sig` vs `sig.next`),
   Chisel (`Reg` vs `:=`), Amaranth (`m.d.sync` vs `m.d.comb`), Spade (`reg(clk) … = …`) and
   Bluespec (atomic rules) all make the current/next distinction syntactic, so the hazard is
   *unexpressible*. Verilog leaves it expressible and lints it (`BLKSEQ`), but those lints compare
   an author-written **marker** (`=` vs `<=`) against an author-written **block kind**
   (`always_comb` vs `always_ff`) — two declarations checked against each other. Copper has
   neither, which is precisely why its rules must infer both sides and why the boundary had to be
   found empirically rather than declared. Inference buys the ergonomics that are contribution 1;
   this is its cost, stated as such.
   `[Evidence: design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md (the full measurement record, including
   three rejected candidate fixes); design_docs/OUTDATED/EXECUTOR_CONVENTION_EXPERIMENT.md and
   OUTDATED/EXECUTION_MODEL_RECONCILIATION.md (historical); guardrails landed —
   `copper_analysis::multi_write_collapse` and `unprotected_pretick_out_write`, both wired into
   `#[hardware(sequential)]` and corpus-validated to flag no shipping design; §Threats T-align,
   T-align-2.]`

3. **Ownership-enforced single-driver and phantom-typed clock domains** as lightweight,
   pass-free structural guarantees discharged entirely by the Rust compiler. We position clock-
   domain typing relative to Clash/Arch (shared mechanism) and distinguish our move-based
   single-driver guarantee from Arch's dependency-graph analysis.

4. **A staged transpilation pipeline (FIR → CHIR → SHIR → VLIR → SystemVerilog)** that lowers
   Rust `async` hardware modules to Verilator-lint-clean SystemVerilog, with the Copper
   simulator — itself validated against independent hand-written Verilog (contribution 2) — as
   the semantic reference. `[Evidence: copper-codegen. Scope: single clock domain,
   flat modules, current example feature set. NOTE the roadmap doc is now historical:
   design_docs/OUTDATED/TRANSPILATION_ROADMAP.md. For what is actually outside the subset today
   see 00_claims_audit.md §Scope of the equivalence claim.]`

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
- **Not "coroutines as a synthesizable FSM surface" as a novel idea.** Prost (LATTE '26) states
  that thesis independently. Our claim is the realization — reuse of a general-purpose compiler's
  lowering, verified same-source equivalence, third-party anchoring — and the concession belongs
  in the first sentence, not a footnote.
- **Not "the same source simulates and synthesizes" without qualification — but the
  qualification changed (2026-08-27).** The bound is no longer capability (34/34 example modules
  transpile, `Memory` and the RISC-V CPU included — the CPU is an equivalence result) but
  language design: shapes the compile-time rules refuse. State the bound as refusal-by-design,
  not as a missing subset (§Threats T7 re-scoped, 00_claims_audit.md §Re-verification).
- **Not "X-accurate."** The simulator models 3-state logic, but that modelling is unverified
  against the reference simulator — Verilator is 2-state — and Copper's control *aborts* on X
  where 4-state Verilog takes the else branch (§Threats T8).
- **Not "every accepted program is verified equivalent."** Every accepted program *within the
  transpilable subset* preserves sim ≡ synth as far as our harness can check, and §Threats T9
  records that the harness itself had five defects able to make a check pass or vanish silently.

## Open items before this is submittable
- [x] **MyHDL prior-art boundary — VERIFIED (2026-07-29), boundary holds.** Confirmed against the
      MyHDL 0.11 manual: the *convertible* subset is explicitly broader than the *RTL-synthesis*
      subset, and MyHDL's synthesizable sequential/FSM idiom is a single-edge `always_seq`/`always`
      process over an explicitly enumerated `enum` state (a hand-written FSM). Multi-`yield`
      cycle-sliced generators are convertible-only (to behavioral HDL), not RTL-synthesizable. So
      MyHDL does not offer the cycle-sliced coroutine algorithm as a *synthesizable* surface.
- [x] **RE-SCOPE contribution 1 for Prost (LATTE '26) — DONE 2026-08-22.** Prost
      (Riedl/Scheipel/Baunach) independently proposes coroutines-as-synthesizable-FSM with
      locals=registers, suspension=cycle, procedural multi-cycle algorithm → Verilog, is
      Rust-`async`-syntax-inspired, and adopts the same loop-must-wait well-formedness rule this
      work enforces. The coroutine-as-FSM idea is therefore **conceded** in the first sentence of
      contribution 1, which now claims the *method*: a general-purpose compiler's lowering is
      SUFFICIENT — no bespoke HDL compiler needed. Prost cannot make that claim (it builds its own
      compiler), and it has no equivalence or anchoring story. Cited and distinguished in
      `related_work.md`.
- [x] **Bound the equivalence claim explicitly — DONE 2026-08-22.** The claim now reads "for the
      subset the transpiler supports", with the excluded set tabulated in
      `00_claims_audit.md` §Scope (Memory, array ports, `/`, asr, bit-width inference gaps,
      generics) and `Memory`'s absence written up as §Threats T7. Contribution 2's evidence list
      was also **corrected**: two cited files (`timing_probe_investigation.rs`,
      `mem_latency_probe.rs`) contain no assertions and are `#[ignore]`d, so they were withdrawn.
- [x] Expand transpiler coverage and the equivalence-verified example set — **DONE 2026-08-27**:
      34/34 example modules transpile, the corpus sweep covers 32/34 (both exceptions reasoned),
      `Memory` transpiles (declared and received), and the RISC-V CPU is an equivalence result
      (`tests/rv32i_pipelined_verilator.rs`, 13 programs cycle-for-cycle). Remaining eval-bound
      work is anchor breadth, not coverage.
- [ ] Decide whether to add a soundness/semantics argument for the async-lowering ↔ transpiler
      correspondence, or lean on empirical equivalence (reviewer risk either way).
- [ ] Pick the target venue framing (PLDI-style PL contribution vs. systems/CAD-style artifact).
- [ ] Reconcile README/design-doc terminology with the code before an artifact evaluation.
