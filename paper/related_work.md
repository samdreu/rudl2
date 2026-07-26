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

*Copper differs by:* (a) its clock-domain safety uses the **same phantom-type-on-ports
mechanism** as Clash and Arch — Copper does *not* claim novelty here and cites these as the
established approach; (b) its genuine ownership contribution is a **single-driver guarantee
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
- Basu. *RHDL: Rust as a Hardware Description Language.* LATTE 2024/2025.
