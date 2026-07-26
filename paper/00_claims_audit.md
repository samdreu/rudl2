# Copper — Code-Grounded Claims Audit & Positioning

**Purpose:** single source of truth for what Copper *actually does* (verified against the
crates, not the design docs, which are out of sync in several places) and how each claim
should be framed against prior work. Read this before writing any paper prose.

Last verified against source: 2026-07-16.

---

## What the code actually does (verified)

| Feature | Mechanism in source | Evidence |
|---|---|---|
| Clock domains | Phantom type `Clock<Domain: ClockDomain>` with `PhantomData<Domain>` | `copper-core/src/types.rs:822-839` |
| Clock-domain-crossing safety | **Phantom-type**, not ownership: `In<T,D>`/`Out<T,D>` carry `PhantomData<D>`; cross-domain pass = `E0308` type mismatch | `copper-core/src/port.rs:13-37`; `examples/cdc/two_domain_counter.rs:40-48` |
| Single-driver guarantee | `Out<T,D>` is **non-`Clone`** (move-only); `In<T,D>` is `Clone`. One writer per wire, by Rust move semantics | `copper-core/src/port.rs:12-49` |
| Async/await FSMs | `#[hardware]` macro only validates + injects a sim barrier; **rustc's own async→state-machine lowering is the FSM**. Vars live across `.await` ⇒ Future-struct fields ⇒ registers | `copper-macros/src/lib.rs:118-138`; `examples/sequential/traffic_light_fsm.rs:26-28` |
| Same-source sim = synth | DUT source included once as Rust (rustc sim) and once via `include_str!` → `copper_codegen::transpile_source` → SV; traces compared under Verilator | `tests/lfsr_equivalence.rs`, `tests/m1_counter_equivalence.rs` |
| Transpiler pipeline | FIR → CHIR → SHIR → VLIR → SV; entry `copper_codegen::transpile_source`; CLI `copper-transpile` | `copper-codegen/src/{parser,chir_lower,shir_lower,vlir_lower,emit}.rs`, `main.rs` |

## Doc/code mismatches to fix before publication
- README says `#[hardware(function_typed)]`; the macro only supports `sequential` / `combinational`.
- Several examples use bare `async fn` + `HardwareExecutor` with **no macro at all**.
- README's "first HDL to use ownership semantics for compile-time CDC" is **not** what the code does (CDC is phantom-typed) and is contested by Clash/Arch.

---

## Positioning decisions

**LEAD (implemented + differentiated):**
1. **async/await as the FSM surface, realized by the host compiler's coroutine lowering.**
   No competitor does this (Arch = declarative state blocks; Spade = pipeline stages;
   Clash = Mealy functions; Anvil = `wait`/events). Framing: *we don't build an FSM
   compiler — rustc's async transform is the FSM, and the live-across-await variables are
   the registers.*
2. **Same literal source → cycle-accurate sim AND SystemVerilog, verified equivalent.**
   Frame as a *correctness* property (backed by the equivalence harness), NOT as "unified
   sim/synth" (table stakes — Chisel/PyMTL/Clash/Bluespec all claim that).

**SUPPORT (real but not novel / narrower than claimed):**
3. Type-driven safety = phantom clock domains (**cite Clash & Arch as prior art — we match,
   not beat**) + **ownership-based single-driver** (move-only `Out` — the honest "Rust
   ownership" contribution, distinct from Arch's dependency-graph single-driver rule).

**RETIRE:** all "first HDL" / "ownership-based CDC" phrasing.

## Current status caveat (C2) — as of 2026-07-16 on branch `transpilation/fir-chir-shir`
The `counter` + `lfsr` equivalence tests (`tests/m1_counter_equivalence.rs`,
`tests/lfsr_equivalence.rs`) are **new, untracked, and currently FAILING**: the in-progress
parser refactor emits `ExprType::Path` for a bare-identifier `port.write()` receiver, but
`chir_lower.rs` still matches only `ExprType::Lit` (3 sites: `:696`, `:744`, `:804`), so no
`.write()`-using module transpiles. The README's "verified equivalent" wording describes the
intended/last-known-good state, not the current tree. **Transpiler fix is planned by the
author.** Any paper claim of demonstrated equivalence must wait until these tests are green.

## Executor-convention decision (2026-07-25) — anchors C2, adds a semantics sub-result
The simulator's cross-tick **output timing** was investigated against independent hand-written
Verilog (not the transpiler). Finding: the executor's tick-phase choice is an **irreducible
dual** — pre-edge continuation makes write-before-tick outputs (Moore FSM, e.g. `mac_fsm`)
match hardware but write-after-tick outputs (flip-flop, enabled register, sync-RAM read) off by
one cycle; post-edge continuation does the exact reverse. No single global convention is correct
for both. See `design_docs/EXECUTOR_CONVENTION_EXPERIMENT.md` (probes:
`tests/timing_probe_investigation.rs`, `tests/mem_latency_probe.rs`; refs:
`tests/fixtures/timing_probe_sv/*.sv`).

**Decision: adopt the post-edge base (the more hardware-standard convention — a register clocked
at edge N is observable in cycle N) + an explicit `RegOut` annotation for held/registered output
ports.** This makes the primitive constructs match hand-written Verilog, which is what lets C2
("cycle-accurate", "semantic reference") be a *hardware* claim rather than a self-consistency
check. Two paper consequences:
- **C2/C4 reframed** to "sim anchored to independent hand-written references," defusing the
  circularity attack ("you only proved your sim agrees with your transpiler").
- **New contribution 5**: output-timing inference is provably insufficient at exactly one point;
  `RegOut` is the minimal, characterized annotation. This is a partial answer to the
  "no formal semantics" weakness below.
- **Guardrail added**: do NOT claim "zero timing annotations."

## Known weaknesses a reviewer will attack
- **Transpiler coverage is thin** (counter + lfsr end-to-end, *once the fix lands*). The eval
  section is bounded by how many examples pass `transpile_source` + Verilator. Active TODO.
- **No formal semantics / soundness argument yet.** Filament (PLDI'23), Anvil (ASPLOS'26),
  Spade all carry one. Leading with a "correctness guarantee" invites demand for a proof
  sketch of the async-lowering ↔ transpiler correspondence, or a much larger verified set.
