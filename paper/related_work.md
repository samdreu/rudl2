# Related Work (draft)

> Positioning grounded in the code audit (`00_claims_audit.md`). Every "Copper differs by…"
> line maps to a verified mechanism, not a doc claim. `[VERIFY]` marks anything still to confirm.

Copper sits at the intersection of three lines of work: (i) HDLs embedded in or inspired by
modern typed programming languages, (ii) type systems that lift hardware bug classes to
compile time, and (iii) the synchronous-language tradition of deriving hardware from
high-level control constructs.

## Embedded / modern-language HDLs

**Chisel** and **SpinalHDL** embed hardware construction in Scala, generating Verilog via an
elaboration + IR flow (Chisel → FIRRTL/CIRCT). They raise the abstraction level but expose
clock and reset as implicit module-level handles rather than per-signal typed values, so
clock-domain crossings are not a typing error. **PyMTL3** [Jiang et al., IEEE Micro 2020]
embeds modeling, simulation, generation, and verification in Python around an in-memory IR
with a modular pass system — an architecture Copper's FIR→CHIR→SHIR→VLIR pipeline echoes,
though PyMTL3 targets multi-level modeling productivity rather than static safety guarantees.
**Spade** [Skarman & Gustafsson, FPL 2022; arXiv:2304.03079] is a standalone HDL with an
ML/Rust-inspired type system and *first-class pipelines*, where the compiler inserts
registers to keep pipeline stages synchronized.

*Copper differs by:* using **`async`/`await` as the FSM authoring surface**, delegating the
state-machine transform to the host compiler (rustc) rather than to a bespoke pipeline or
elaboration pass. Variables that remain live across a `.await` become fields of the
compiler-generated `Future` — i.e., the registers of the synthesized FSM — so the FSM
encoding is the language's own coroutine lowering.

## Coroutine- and generator-based hardware description (closest prior art)

This is where Copper's lead contribution must be defended most carefully: using a host
language's *coroutine suspension* as a hardware timing primitive, and simulating hardware by
*running the host language*, are **both independently precedented**. Neither is Copper's
novelty; the novelty is a specific, narrower combination.

**MyHDL** [Decaluwe] embeds hardware in Python as *cooperating generators communicating
through signals*, where a `yield` statement names a **resume condition** — a signal edge
(`clk.posedge`), a signal change, or a tuple thereof. MyHDL simulates by running these
generators under its own event scheduler and converts a synthesizable subset to Verilog/VHDL.
This is the canonical "coroutine-as-hardware + simulate-by-executing + convert-to-HDL" system
and predates Copper by ~two decades. **cocotb** likewise drives clocked stimulus with
`async`/`await` and `await RisingEdge(clk)` — the *exact syntactic surface* Copper uses — but
purely as a **testbench/verification harness** over an external simulator (it neither describes
the DUT nor emits RTL). **Migen/Amaranth** use Python generators (`yield` = advance one clock)
for testbenches, with an experimental generator→FSM tool (`fsm-gen-migen`) exploring
generators as a *synthesizable* surface.

**Prost** [Riedl, Scheipel & Baunach, LATTE '26] is the **closest prior art to contribution 1**
and postdates the start of this work — it must be cited and distinguished directly. Prost
proposes *coroutines as the fundamental abstraction of synchronous hardware*, with exactly the
identification Copper makes: "Local variables correspond to registers, control flow maps to
next-state logic … at each positive clock edge, we advance the coroutine until its next
suspension point." It targets **synthesizable** multi-cycle algorithms (it emits Verilog
next-state logic), is **syntactically inspired by Rust's `async`/`await`**, and even adopts the
same well-formedness rule this work's item-2 analysis enforces ("the compiler requires each loop
to contain at least one wait statement and to run for at least one iteration"). So Copper cannot
claim "coroutines as a synthesizable multi-cycle FSM surface" as novel in the abstract — Prost
states that thesis independently. **Copper's daylight from Prost is specific and defensible:**
(a) Prost is a **new standalone language with a bespoke compiler** that merely *borrows Rust's
syntax* ("Prost is an HDL based on coroutines … syntactically … inspired by Rust's async/await"),
whereas Copper is **embedded in Rust and reuses `rustc`'s own `async` lowering** as the FSM —
there is no bespoke coroutine compiler, and the register set is the *general-purpose compiler's*
captured live-across-suspension state; (b) Prost presents **no simulation/synthesis equivalence
and no hardware anchoring** — Copper runs the identical source under `rustc` and, independently,
transpiles it, checks the two equivalent under Verilator, and anchors timing to third-party
BaseJump hardware; (c) Prost is a **three-page vision paper** (single-clock, no combinational
cycles, hand-illustrated Verilog, no implementation or evaluation), whereas Copper reports a
working transpiler + simulator with an equivalence-checked example suite. The honest position:
Prost and Copper **converge on the same core idea from opposite starting points** (a new language
vs. reuse of an existing language's compiler), and Copper's contribution is the *embedded-reuse +
verified-same-source + hardware-anchored* realization, not the coroutine-as-FSM idea itself.

*Copper differs by — and only by — the following combination, stated at the resolution where
the daylight actually is:*

1. **The coroutine suspension is a clock-*cycle boundary in a multi-cycle algorithmic program*,
   not an event-sensitivity wait.** MyHDL's `yield clk.posedge` makes each synthesizable
   generator a single edge-sensitive **RTL process** (≈ one Verilog `always` block). MyHDL draws
   an explicit line between its **convertible** subset and its **RTL-synthesis** subset — the
   manual states the convertible subset "is much broader than the RTL synthesis subset which is
   an industry standard," and that "code written according to the RTL synthesis rules should
   always be convertible" (`docs.myhdl.org/.../conversion.html`). A *multi-`yield` behavioral*
   generator — one that reads like a sequential algorithm sliced into cycles — may therefore
   *convert* to (behavioral, multi-event-control) Verilog/VHDL, but lies **outside** the
   RTL-synthesizable subset. MyHDL's *synthesizable* sequential/FSM idiom is instead a
   single-edge `always_seq(clk.posedge, …)` (or `always(clk.posedge)`) process over an
   **explicitly enumerated** `enum` state with case dispatch (`docs.myhdl.org/.../rtl.html`) —
   i.e. a hand-written FSM, exactly what Copper does not require the designer to write. Copper's
   `clk.tick().await` slices an ordinary imperative algorithm into FSM states that *are*
   synthesizable, closer to the HLS / Handel-C / Silice "sequential program → FSM+datapath"
   model — but realized by the host compiler's coroutine transform rather than a bespoke
   compiler. (Verified against the MyHDL 0.11 manual, 2026-07-29.)
2. **The register set is the compiler-captured live-across-suspension state**, not a set of
   explicitly declared `Signal`s plus an edge-sensitive decorator template (MyHDL) — i.e., we
   claim an *identification* of rustc's `Future` fields with the FSM registers, not merely that
   coroutines model concurrent processes.
3. **`async`/`await` used for *synthesizable design*, not verification.** This is the line
   against cocotb: same surface syntax, opposite role — Copper transpiles the coroutine to RTL;
   cocotb never leaves the testbench.
4. **The same literal source is both run (simulation) and transpiled, checked equivalent against
   an external simulator** and anchored to third-party hardware (see below).

Honest bound on the claim: each ingredient above has prior art (MyHDL for 1–2 in Python at the
process level; cocotb for 3's syntax in verification; Clash/RHDL for simulate-by-running). The
contribution is the *conjunction realized through a general-purpose language's native
`async` lowering as a synthesizable multi-cycle FSM encoding* — which, to our knowledge, no
prior system does. We do **not** claim "coroutines for hardware," "simulate by running the
language," or "it's just Rust" as novel in isolation; the last is RHDL's own framing verbatim.
`[VERIFIED 2026-07-29: (i) MyHDL's synthesizable/RTL subset excludes multi-yield cycle-sliced
generators — confirmed against the 0.11 manual (convertible ⊋ RTL-synthesis; synthesizable
sequential = single-edge always_seq + explicit enum-state FSM). (ii) No async-based *embedded-in-
Rust* synthesizable HDL has appeared — RHDL does not use async/coroutines; the one new
coroutine-as-synthesizable-FSM system, Prost (LATTE '26), is a bespoke Rust-syntax-inspired
language, not embedded Rust reusing rustc's async lowering. Prost is now cited and distinguished
above; contribution 1 must be re-scoped so its novelty is the embedded-reuse + verified-same-
source + hardware-anchored realization, not the coroutine-as-FSM idea in the abstract.]`

## Simulation semantics: how the register/combinational split is resolved

The output-timing question behind contribution 5 — when does a write to an output become visible —
is one every hardware simulator must answer, and the established answers explain why it surfaces
for Copper specifically. **Event-driven Verilog/VHDL simulators** (Verilator, VCS, Questa, Icarus,
GHDL) resolve it *explicitly and structurally*: the author writes blocking `=` vs non-blocking `<=`
(VHDL: variable `:=` vs signal `<=`) inside an explicit `always_comb`/`always @*` or
`always @(posedge)` block, and a stratified event queue samples every non-blocking right-hand side
before applying any left-hand update, so a register update cannot race a combinational read.
Crucially, *simulation time advances between two writes that straddle a clock edge*
(`out = 0; @(posedge clk); out = 1;`), so the intermediate value occupies a full clock period and
is observable to testbench and waveform. **Cycle-based and dataflow tools** (Clash,
Chisel→FIRRTL/treadle, cycle simulators) make the split *unrepresentable*: the design *is* a pair
(combinational function, register set) evaluated once per cycle, so an output cannot be assigned
twice in one cycle. Clash's Mealy machines are pure `state → input → (state, output)` functions.

Copper's `Out`/`RegOut` distinction is a rediscovery of exactly the blocking/non-blocking pair:
plain `Out` writes its cell immediately (≈ blocking `=`), `RegOut` buffers and commits at the clock
edge (≈ non-blocking `<=`), and the two agree with the transpiled `assign` / `always_ff`
respectively under Verilator. What is *not* inherited is the event kernel's advancing time: the
Copper executor runs a coroutine's post-tick continuation immediately within one `tick_clock`, with
no observable step between a pre-tick and a post-tick write to the *same* combinational `Out`. A
single-write-per-cycle output (the common case — a Mealy decode, a counter's registered value) is
unaffected and matches hand-written Verilog; the residual is a combinational `Out` written on both
sides of one tick in a single control-flow region (`out.write(0); tick; out.write(1)`), where the
second write clobbers the first before observation.

The precedent for the residual is decisive. **MyHDL** — the closest coroutine analogue — resolves
it by *restricting the synthesizable subset*: a synthesizable MyHDL generator is a single
edge-sensitive process (≈ one `always` block); multi-`yield`, cycle-sliced behavioral generators
are convertible-only, never RTL-synthesizable (verified against the 0.11 manual, above). No prior
simulator both treats a *multi-tick* coroutine as the synthesizable form and simulates it by
running the coroutine, so none inherits this problem; Copper's control extraction is what
reintroduces it, and Copper follows the same discipline — the multi-write-across-a-tick pattern is
rejected at compile time (directing the author to `RegOut` for a registered output, or to explicit
per-state writes for a combinational one), so every accepted program preserves same-source
sim ≡ synth. `[Landed: `copper_analysis::multi_write_collapse` detects the pattern over the
shared CFG — a bare-tick output straddle with a leading (deferred) input read — and the macro
rejects it, pointing at `RegOut`. Corpus-validated to flag only the two control-extraction
probes, none of the shipping designs.]`

## Type systems for compile-time hardware safety

**Clash** [Baaij et al.] compiles a subset of Haskell to Verilog/VHDL and encodes **clock
domains at the type level**, requiring an explicit typed synchronizer to cross domains;
accidental crossings are compile-time type errors. **Arch** [arXiv:2604.05983, 2026]
declares every clock as `Clock<D>` with `D` a phantom domain parameter and tracks four
independent dimensions — bit widths, clock domains, port directions, and *signal ownership*
(a **single-driver rule** enforced via a dependency graph, explicitly not Rust-style
lifetimes) — turning latches, multiply-driven nets, CDC/RDC violations, and width mismatches
into compile errors; its principal thesis is AI-generatability. **Anvil**
[Yu et al., ASPLOS 2026; arXiv:2503.19447] introduces a type system for **timing safety** —
lifetimes, loan-times, and event graphs guaranteeing values stay stable across the cycles
they are used — and claims to be "the first HDL … that guarantees timing safety." **Filament**
[Nigam et al., PLDI 2023] uses **timeline types** to statically guarantee that composed,
pipelined modules never have resource or timing conflicts, for statically-fixed latencies.

**RHDL** [Basu, LATTE 2024/2025] is the most direct point of comparison: an embedded-Rust HDL
that, like Copper, insists all code be valid Rust ("it's just Rust") and lowers it through a
co-compiler running alongside rustc (RHIF SSA → RTL → flow-graph → Verilog). RHDL also encodes
**clock domains as a phantom marker type parameter** — `Signal<b4, Red>` vs `Signal<b4, Blue>`,
cross-domain operations a compile error — i.e. Copper's CDC mechanism is precedented *within
Rust itself*. Crucially, however, RHDL does **not** use `async`/`await` or coroutines for
sequential logic (it lowers combinational SSA and represents registers/clocking through explicit
constructs), and it simulates by interpreting its compiled IR rather than by executing the Rust
directly — so it shares Copper's CDC approach and "it's just Rust" framing but not the
coroutine-as-FSM surface or the run-the-Rust simulation model.

*Copper differs by:* (a) its clock-domain safety uses the **same phantom-type-on-ports
mechanism** as Clash, Arch, and RHDL — Copper does *not* claim novelty here and cites these as
the established approach; (b) its genuine ownership contribution is a **single-driver guarantee
enforced by Rust move semantics** — output ports (`Out<T,D>`) are non-`Clone`, so exactly one
module can own and drive a wire. This differs from Arch's dependency-graph single-driver
analysis in that it is discharged by the host language's borrow checker with no separate
pass. Copper does not currently target Anvil-style multi-cycle timing safety or Filament-style
pipeline-composition typing. `[VERIFY: whether const-generic Bits<N> width checking should be
positioned against Arch's width dimension.]`

## Synchronous languages and high-level control → hardware

**Esterel** [Berry & Gonthier, *Sci. Comput. Program.* 1992] established deriving circuits
from high-level concurrent control with a formal semantics, including a translation of Pure
Esterel to circuits. The broader synchronous family (Lustre, Signal) and classical
**FSMD**-based hardware compilation share Copper's goal of describing control at a high level
and lowering it to cycle-accurate hardware.

*Copper differs by:* expressing sequential control as ordinary Rust `async` code executed on
a cycle-accurate executor, where `clk.tick().await` marks the clock edge — reusing a
general-purpose language's concurrency mechanism rather than a dedicated reactive calculus,
and (unlike most of this tradition) checking the derived hardware against the *same source's*
simulation.

## Simulation/synthesis correspondence

Chisel, PyMTL3, Clash, and Bluespec all provide simulation and synthesis from one description,
so "unified sim/synth" alone is not a contribution. Copper's claim is narrower and stronger:
the **same literal source text** is compiled by rustc for cycle-accurate simulation *and* fed
to the transpiler for SystemVerilog, and the two are checked for behavioral equivalence under
Verilator (the equivalence harness in `tests/*_equivalence.rs`). A same-source sim/synth check
is only meaningful if the reference is anchored: two artifacts from one compiler can be made to
agree while both diverging from hardware. We therefore separately **validate the simulator's
cycle-level timing against independent, third-party SystemVerilog** — modules from the
**BaseJump STL** library [Taylor et al.], checked against BaseJump's own DUT sources and the
parameters/stimulus of BaseJump's own testbenches, plus hand-written references for the
primitive constructs (flip-flop, enabled register, synchronous-read RAM). Because both the
reference hardware and the test vectors originate outside Copper, the equivalence rests on a
hardware anchor rather than on internal self-consistency. This anchoring surfaced a substantive
semantic result: for *output-port* timing, no single executor scheduling convention matches
hardware for both registered and combinational outputs — the two are exact duals — which we
resolve with a single, provably-necessary `RegOut` annotation while inference continues to
handle internal state. `[VERIFY: current end-to-end equivalence is demonstrated for counter and
lfsr; hand-written-reference anchoring is demonstrated for FF/enff/RAM (dual_port_ram + timing
probes); scale both before framing as a general guarantee.]`

---

## References (to formalize in BibTeX)
- Jiang et al. *PyMTL3: A Python Framework for Open-Source Hardware Modeling, Generation,
  Simulation, and Verification.* IEEE Micro, 2020.
- Skarman & Gustafsson. *Spade: An Expression-Based HDL With Pipelines.* FPL 2022 / arXiv:2304.03079.
- Nigam, Azevedo de Amorim, Sampson. *Modular Hardware Design with Timeline Types (Filament).* PLDI 2023 / arXiv:2304.10646.
- Yu, Jha et al. *Anvil: A General-Purpose Timing-Safe Hardware Description Language.* ASPLOS 2026 / arXiv:2503.19447.
- *Arch: An AI-Native Hardware Description Language for Register-Transfer Clocked Hardware Design.* arXiv:2604.05983, 2026.
- Berry & Gonthier. *The Esterel Synchronous Programming Language: Design, Semantics, Implementation.* Sci. Comput. Program. 19(2), 1992.
- Baaij et al. *Clash: A Functional Hardware Description Language.* (clash-lang.org)
- Taylor et al. *BaseJump STL: SystemVerilog Standard Template Library.* bespoke-silicon-group/basejump_stl (Solderpad Hardware License v0.51). Used as independent hardware references for the equivalence eval.
- Bachrach et al. *Chisel: Constructing Hardware in a Scala Embedded Language.* DAC 2012.
- Nikhil. *Bluespec SystemVerilog.* MEMOCODE 2004.
- Basu. *RHDL: Rust as a Hardware Description Language.* LATTE 2024/2025. (Phantom-type CDC in
  Rust; "it's just Rust"; RHIF→RTL→flow-graph compiler — closest embedded-Rust comparison.)
- Riedl, Scheipel & Baunach. *Prost! Coroutine-based Hardware Description.* LATTE '26. — coroutines
  as the fundamental synchronous-circuit abstraction (locals=registers, suspension=cycle boundary),
  synthesized to Verilog next-state logic; Rust-`async`-syntax-inspired **bespoke language + bespoke
  compiler**. The **closest prior art to contribution 1**; distinguished from Copper by embedded-reuse
  of `rustc`'s lowering, verified same-source sim/synth equivalence, and hardware anchoring (none of
  which Prost has). 3-page vision paper, no implementation/evaluation.
- Decaluwe. *MyHDL: a Python-Based Hardware Description Language.* (myhdl.org) — generators +
  `yield`-as-resume-condition; simulate-by-running + convert to Verilog/VHDL. Convertible subset is
  broader than its RTL-synthesis subset; synthesizable sequential logic is single-edge `always_seq` +
  explicit enum-state FSM (multi-yield cycle-slicing is convertible-only, not RTL-synthesizable).
- cocotb: *Coroutine-based cosimulation testbench environment.* (docs.cocotb.org) —
  `async`/`await` + `await RisingEdge(clk)` as a verification harness (not synthesizable).
- Bourdeauducq et al. *Migen / Amaranth (nMigen).* (m-labs.hk / amaranth-lang.org) — Python
  generator testbenches; `fsm-gen-migen` experimental generator→FSM.
- Handel-C; Silice; Vivado HLS / Catapult / LegUp — sequential-program→FSM+datapath compilers
  (bespoke compilers, not host-language coroutine reuse); cite for contribution 1's lineage.
