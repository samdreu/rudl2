# Evaluation numbers (generated 2026-08-26)

Regenerate with `tools/stats/collect.sh`. **Do not edit by hand** — every
number here is derived from the repo, and a hand-copied number goes stale.

## M4/M5 — evidence coverage

| property | count |
|---|---|
| example `#[hardware]` modules | 34 |
| transpile to SystemVerilog | 29/34 |
| covered by the differential sweep (sim vs Verilator, seeded random) | 28/34 |
| with a dedicated equivalence test | 22/34 |
| anchored to third-party hardware (BaseJump STL) | 7/34 |
| **anchored AND differentially swept** | 7/34 |

The last row is the transitive chain: the *generated* SystemVerilog is tied to
hardware neither Copper nor its transpiler wrote.

Not swept, each with the reviewed reason from `build.rs`'s `SKIP`:

* `two_domain_top` — a `#[hardware(structural)]` parent: transpile-only by design, with no simulatable body to drive (item 4 — the sim wires the hierarchy by hand)
* `ripple_carry_adder` — does not transpile: cause J-b, a tuple-returning helper (`let (s, c) = full_adder(…)`), reported as a width error. Pinned by transpile_inference_gaps.rs. The fixture copy is written without the helper and does sweep
* `rv32i_cpu` — does not transpile: cause F, a `Vec<Bits<32>>` port (TODO, TRANSPILER COVERAGE)
* `rv32i_cpu_pipelined` — does not transpile: cause F, a `Vec<Bits<32>>` port (TODO, TRANSPILER COVERAGE)
* `uart_tx` — does not transpile: cause H — `spawn_uart` in the same file has a hardware-looking signature with no `#[hardware]`, which `prepare_source` refuses at FILE level before either module is looked at
* `uart_rx` — does not transpile: cause H, the same file-level `spawn_uart` rejection as `uart_tx`

> **Scope.** The RISC-V CPUs are in that list. `tests/rv32i_integration.rs` is a
> simulator self-check against known program results, **not** a sim≡synth check.
> No sentence may place the CPU beside the equivalence claim.

## M2 — post-synthesis area vs the reference (Yosys, generic cells)

| module | Copper cells | reference cells | ratio | FFs (Copper/ref) |
|---|---|---|---|---|
| `bsg_adder_one_hot` | 25 | 25 | 1.0 | 0/0 |
| `bsg_counter_up_down` | 16 | 15 | 1.067 | 3/3 |
| `bsg_dff_en` | 8 | 8 | 1.0 | 8/8 |
| `bsg_encode_one_hot` | 15 | 11 | 1.364 | 0/0 |
| `bsg_mux_one_hot` | 20 | 20 | 1.0 | 0/0 |
| `sipo_block` | 32 | 36 | 0.889 | 32/30 ⚠ |

**3/6 synthesise to an identical cell count** as the hand-written
BaseJump reference; mean ratio 1.053 (range 0.889–1.364).

> **Scope.** Yosys `synth` to generic cells, not a vendor flow — comparative,
> not absolute silicon. A ⚠ marks a flip-flop-count difference from the
> reference, which is a finding to explain, not a failure: these designs pass
> the differential equivalence check.

* `bsg_gray_to_binary` — reference synth failed

## M1 — design size vs the reference

| module | Copper SLOC | reference SLOC | ratio | emitted SV SLOC |
|---|---|---|---|---|
| `bsg_adder_one_hot` | 18 | 27 | 0.667 | 25 |
| `bsg_counter_up_down` | 22 | 20 | 1.1 | 28 |
| `bsg_dff_en` | 14 | 15 | 0.933 | 12 |
| `bsg_encode_one_hot` | 18 | 40 | 0.45 | 25 |
| `bsg_gray_to_binary` | 10 | 50 | 0.2 | 18 |
| `bsg_mux_one_hot` | 16 | 22 | 0.727 | 23 |
| `sipo_block` | 33 | 17 | 1.941 | 75 |

Mean Copper/reference SLOC over 7 anchored designs: **0.860**.

> **Scope.** Counts the `#[hardware]` module only — not the self-check harness
> that shares the file. Ratios are reported *only* where an independent
> implementation of the same design exists; against Verilog we wrote ourselves
> a ratio would measure our own prose style.

## M6 — transpiler performance

* 29 modules lowered to SystemVerilog
* median **3.7 ms** per module (min 2.8, max 6.1)
* slowest: `bsg_counter_up_down` at 6.1 ms

> **Scope.** Release binary, median of repeated runs after a warm-up. Simulation
> throughput vs Verilator is **not** measured — no fixed-cycle benchmark harness
> exists yet.
