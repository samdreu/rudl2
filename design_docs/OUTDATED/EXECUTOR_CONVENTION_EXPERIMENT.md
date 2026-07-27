# Executor phase convention — the measured dual-convention experiment

**Status:** findings recorded (2026-07-25), **direction undecided** (user is digesting).
This doc captures a decisive empirical experiment on how the simulator's executor
resolves `clk.tick()` relative to the clock edge, and what it means for hardware
accuracy. It supersedes the informal reasoning in EXECUTION_MODEL_RECONCILIATION.md
about which output timing is "correct".

## How we got here

`dual_port_ram` (the memory example) was the first example with an **independent
hand-written Verilog reference** for a *sequential* path. Fixing a pre-existing
filename typo (`.v` → `.sv`) turned its long-dead verilator check back on, and it
showed the atomic sim's `dob` is **one cycle later** than a textbook block RAM.
That triggered a controlled investigation against hand-written Verilog (not the
transpiler), reproduced by two ignored probe tests:

- `tests/timing_probe_investigation.rs` — DFF (two codings), enable-FF, mac_fsm, counter
- `tests/mem_latency_probe.rs` — block-RAM read latency, two DUT structures
- reference Verilog: `tests/fixtures/timing_probe_sv/*.sv`

Run with `cargo test --test <name> -- --ignored --nocapture`. The conclusion is
pinned as a live (non-ignored) regression test in
`tests/regout_postedge_probe.rs`: plain `Out` is combinational (`[1,2,3,…]`), the
same body on a `RegOut` is registered (`[0,1,2,…]`).

## The two executor conventions

`HardwareExecutor::tick_clock` runs one reaction per call in three phases:
pre-edge settle → `clk.advance()` (posedge, fires `on_posedge` listeners) →
post-edge settle. A `clk.tick()` resolves only in whichever phase
`set_tick_pre_edge(true)` marks. That choice decides where a reaction's
**post-tick code** runs relative to the edge:

- **Pre-edge continuation (ATOMIC, current):** ticks resolve in the pre-edge pass,
  so post-tick code runs in the *next* `tick_clock`, **before** its advance. A
  value written after a tick is observed **one cycle later** than the edge.
- **Post-edge continuation (PROTOTYPE):** ticks resolve in the post-edge pass, so
  post-tick code runs in the **same** `tick_clock`, **after** the advance. A value
  written after a tick is observed **in the edge's own cycle** — the standard
  synchronous-testbench convention.

The prototype is a two-line swap of the `set_tick_pre_edge(true/false)` calls in
`tick_clock` (measured, then reverted).

## Measured results (sim vs hand-written Verilog, same drive→posedge→sample harness)

| Construct | out.write position | Atomic (pre-edge) | Prototype (post-edge) |
|---|---|---|---|
| DFF `q<=d` | after tick | ❌ +1 late | ✅ match |
| enable-FF `if sel {q<=d}` (held) | after tick | ❌ +1 late | ✅ match |
| **block-RAM read** (`dual_port_ram`) | after tick | ❌ +1 late | ✅ **verilator PASS** |
| mac_fsm (3-state held FSM output) | before tick | ✅ match | ❌ +1 early |
| counter | before tick | `[0,1,2]` registered | `[1,2,3]` combinational |

Suite-wide: under the prototype the `copper-sim` counter unit tests go red (they
were re-baselined to the registered `[0,1,2]`), while the memory/DFF constructs go
green. The integration suite is dominated by already-`#[ignore]`d equivalence tests
so its pass count barely moves.

## Conclusion: the distinction is irreducible at the executor level

The two conventions are **exact duals** — each matches hardware for precisely the
cases the other misses, split cleanly by whether `out.write` sits **before** or
**after** the tick. **No single global phase choice is universally correct.** This
is the empirical, executor-level confirmation of REGISTERED_OUTPUTS.md's abstract
argument that register-vs-combinational output timing is a real, irreducible choice.

Corrected understandings from this experiment:

1. **Post-edge is the more hardware-*standard* base.** A register clocked at edge N
   is observable in cycle N; the prototype makes the *fundamental* building blocks —
   DFF, memory, enable-FF — match textbook Verilog out of the box (memory gets a
   real verilator PASS). The atomic model over-delays these write-after-tick outputs.
2. **mac_fsm matches under atomic only incidentally** — its output is written
   *before* its tick, so atomic's +1 lands it correctly. It is not evidence that
   atomic is universally right.
3. **`RegOut` is not superseded.** It only *adds* latency, so it cannot fix atomic's
   already-too-late DFF; but on a post-edge base it cleanly registers mac_fsm's held
   output. Post-edge-base + explicit `RegOut` for held-FSM outputs makes *both*
   classes match hardware — the two ideas compose. (Un-supersede REGISTERED_OUTPUTS.md.)
4. The memory `+1` is **not DUT-fixable**: "straddle" (drive dob after tick) and
   "prewrite" (drive dob before tick) give byte-identical traces. The Memory
   primitive itself is a faithful 1-cycle read (its unit tests call `advance()` +
   `data()` directly); the extra cycle is purely the executor convention.

## The open decision (undecided)

- **A. Switch to post-edge base + `RegOut`.** Most hardware-faithful for the common
  case; DFF/memory/enable-FF correct by construction; held-FSM outputs (mac_fsm) use
  explicit `RegOut`. Cost: re-migrate the executor, implement `RegOut` end-to-end,
  resolve mac_fsm, re-baseline the suite again.
- **B. Keep atomic, align references.** Treat DFF/memory as legitimate 1-cycle-
  *latency* (next-cycle) registered reads; fix the memory example's reference model +
  `.sv` to that latency and re-baseline the demos. Lowest churn, but the sim's basic
  DFF won't match textbook `q<=d` in a standard testbench — a documented wart.

The reference `.sv` and probe tests are committed so either direction can be
re-measured. See [[copper-timing-reconciliation-status]].
