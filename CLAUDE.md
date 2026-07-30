# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Copper is a Rust-embedded HDL. You write hardware as `#[hardware]`-annotated
async Rust functions; Copper both **simulates** them (a custom async executor)
and **transpiles** them to SystemVerilog. The project's correctness bar is that
the two agree: most examples run the Copper simulator, emit SystemVerilog + a
Verilator C++ testbench, compile it with Verilator, and assert the two match
cycle-by-cycle.

It is a **library/CLI workspace, not a GUI or server** — "does it work?" means
the example equivalence check passes, and there is nothing to screenshot.

## Commands

The driver script wraps build + CLI + representative examples and exits non-zero
on first failure (clean run ends with `SMOKE OK`):

```bash
.claude/skills/run-copper/smoke.sh
```

```bash
.claude/skills/run-copper/smoke.sh --no-examples   # build + CLI only, no Verilator
.claude/skills/run-copper/smoke.sh --example lfsr   # one named example
.claude/skills/run-copper/smoke.sh --all-examples   # every [[example]] (slow)
.claude/skills/run-copper/smoke.sh --test           # also cargo test --workspace
```

Underlying cargo commands:

```bash
cargo build --workspace
cargo test --workspace
cargo test -p copper-sim executor          # a single test by name filter
cargo run --example mux                     # run one example end-to-end (needs Verilator)
```

The `copper-transpile` CLI emits SystemVerilog for one **concrete** module:

```bash
cargo run -q -p copper-codegen --bin copper-transpile -- examples/combinational/one_bit_comparator.rs
```

Flags: `-o <out.sv>`, `--module <name>` (required when a file has >1 module),
`--profile verilator|generic|yosys`, `--list`. Example names are the
`[[example]]` entries in `Cargo.toml`.

### Environment gotchas (important)

- **Verilator is required** for examples and equivalence tests (`brew install
  verilator`). A stale `VERILATOR_ROOT` env var on this machine breaks it — the
  driver `unset`s it; when running `cargo run --example …` by hand, prefix with
  `env -u VERILATOR_ROOT`.
- **`copper-transpile` only handles concrete (non-generic) modules.** Generic
  modules (`const WIDTH_P: usize`, `Bits<W>`, `Clock<D>`) are monomorphized at
  example-run time by the `#[hardware]` macro, not by the standalone CLI — to
  see their SystemVerilog, run the example. Multi-module files need `--module`.

## Architecture

### The five crates

- **`copper-core`** — the type system and every IR in the pipeline. Hardware
  types (`Logic`, `Bits<N>`, `Clock<D>`, ports `In`/`Out`/`RegOut`, `Memory`),
  the phantom clock-domain machinery (`cdc.rs`), and the IR structs
  (`frontend_ir`, `chir`, `shir`, `vlir`). No proc-macro or codegen logic. A leaf
  crate (no workspace dependencies) — which is what lets both the proc-macro and
  the transpiler share analysis over it without a dependency cycle.
- **`copper-analysis`** — the shared compile-time control/liveness analysis (the
  c2 architecture; depends only on `copper-core` + `syn`). Keys off `syn::ItemFn`
  — the representation *both* front-ends already hold — so the sim macro and the
  transpiler consume **one** authoritative analysis rather than two that must
  agree. See `design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md` (item 2). Provides
  `Cfg` (a real control-flow graph over `syn::ItemFn`, `E_comb`/`E_tick` edges),
  `infer_registers` (backward-liveness register inference — a local is a register
  iff it is defined-in-loop ∧ live-across-a-tick), `check_reachability` (every loop
  path must reach a `clk.tick().await`, enforced as a hard spanned compile error in
  **both** front-ends), and the G2 structural reg-match helpers. Consumed by both
  `copper-macros` and `copper-codegen`.
- **`copper-macros`** — the `#[hardware(sequential|combinational|synchronizer)]`
  proc macro. Validates the signature, enforces CDC rules at compile time,
  injects per-read freshness guards, wraps combinational bodies in the
  `loop { …; delta_yield().await; }` shape, and makes the module a
  `HardwareModule` (so a bare `async fn` won't simulate).
- **`copper-codegen`** — the transpiler and the `copper-transpile` CLI
  (`bin/`, `main.rs`). Owns the lowering pipeline and Verilog emission.
- **`copper-sim`** — the async simulation executor and the Verilator
  equivalence verifier (`verification.rs`, `verify_with_verilator`). The executor
  visits tasks in a configurable `PollOrder` (default `Insertion`; `Reversed` /
  `Seeded` exist for the poll-order-independence fuzzer).

The **root `copper` crate** (`src/`) is the hardware standard library —
reusable `#[hardware]` modules like `sync_2ff` that must live *downstream* of
`copper-sim` because the macro expands to `::copper_sim::…` paths.

### Transpilation pipeline (codegen)

`transpile_fir` in `copper-codegen/src/lib.rs` is the spine:

```
Rust #[hardware] fn
  → FIR   (frontend_ir; source-shaped, pre-normalization; capture_frontend_ir)
  → control_extract  (FIR→FIR: flatten branch-nested ticks into an explicit `match pc` FSM)
  → CHIR  (chir_lower)
  → SHIR  (shir_lower)
  → VLIR  (vlir_lower; 1:1 with legal SystemVerilog — legalized names, concrete widths)
  → SystemVerilog text  (emit.rs)
```

### Execution / timing model (sim)

This is the semantic core; read `design_docs/SYNCHRONOUS_SEMANTICS.md`. The
rules the whole project is built around:

- Every `clk.tick().await` is a **clock-cycle boundary**; each suspension point
  becomes an FSM state; every value live across an await becomes a **register**.
- A tick has phases: **pre-edge settle → clock edge → post-edge settle →
  post-edge observation**. `HardwareExecutor::tick_clock` drives these; the
  current phase gates phase-aware futures.
- **Simulation must be independent of Rust async poll order** — a well-formed
  design simulates identically under any `poll_tasks` order. Do not introduce
  timing that depends on task ordering.
- `Out` vs `RegOut`: `RegOut<T,D>` is a registered output driven from
  `always_ff` (use for write-before-tick Moore outputs); plain `Out` otherwise.

### Clock-domain crossing (CDC)

Signals and clocks carry a **phantom domain type** (`Clock<D>`, `In<T,D>`,
`Out<T,D>`, `D: ClockDomain`). Wiring one domain into another port is a nominal
type mismatch rejected at `cargo build`. A crossing is only legal inside a
`#[hardware(synchronizer)]` module (e.g. `sync_2ff`), which is exempt from the
per-domain check. `copper-core/src/cdc.rs` has no runtime code — its
`compile_fail` doctests **are** the executable specification, so `cargo test`
fails if a guarantee regresses. A module may not *construct* a clock, only
receive/clone one.

## Conventions

- **Verify empirically, and check "which behavior is correct" against an
  independent reference, not by argument.** `examples/basejump/` checks Copper
  modules against independent BaseJump STL Verilog; that Verilog is under the
  Solderpad license and needs attribution.
- Design docs live in `design_docs/`. Most have been moved to
  `design_docs/OUTDATED/`; `SYNCHRONOUS_SEMANTICS.md` is the current one.
  Treat `OUTDATED/` as historical, not authoritative.
- Generated artifacts (`tb_*.cpp`, `obj_dir/`, `waveforms/*.vcd`) land in the
  repo root and subdirs and are gitignored.
- Do research on prior HDLs and hardware DSLs to get knowledge on what correct semantics
  should be and to get advice on design decisions.
