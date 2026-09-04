# Evaluation numbers (generated 2026-09-03)

Regenerate with `tools/stats/collect.sh`. **Do not edit by hand** — every
number here is derived from the repo, and a hand-copied number goes stale.

## M4/M5 — evidence coverage

| property | count |
|---|---|
| example `#[hardware]` modules | 36 |
| transpile to SystemVerilog | 36/36 |
| covered by the differential sweep (sim vs Verilator, seeded random) | 34/36 |
| with a dedicated equivalence test | 23/36 |
| anchored to third-party hardware (BaseJump STL) | 7/36 |
| **anchored AND differentially swept** | 7/36 |

The last row is the transitive chain: the *generated* SystemVerilog is tied to
hardware neither Copper nor its transpiler wrote.

Not swept, each with the reviewed reason from `build.rs`'s `SKIP`:

* `two_domain_top` — a `#[hardware(structural)]` parent: transpile-only by design, with no simulatable body to drive (item 4 — the sim wires the hierarchy by hand)
* `rv32i_cpu_pipelined` — ANCHORED OUTSIDE THIS SWEEP as of 2026-08-27: tests/rv32i_pipelined_verilator.rs runs all 13 architectural programs on the simulator AND on the transpiled core Verilated under a hand-written owner (the received-memory ABI's parent, WriteFirst collision policy), comparing (program_counter, halted, a0) cycle-for-cycle through the halt. THIS sweep still cannot cover it — its `Memory` PARAMETER cannot be supplied by the harness (the Kind::Memory rule below) — so the dedicated lane is the behavioural gate. Every transpile cause is closed; see the lane's header for the timing rules it pinned (edge-form staging, ff-position array reads, the RegOut halt outputs)

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
| `sipo_block` | 37 | 36 | 1.028 | 30/30 |

**3/6 synthesise to an identical cell count** as the hand-written
BaseJump reference; mean ratio 1.077 (range 1.000–1.364).

> **Scope.** Yosys `synth` to generic cells, not a vendor flow — comparative,
> not absolute silicon. A ⚠ marks a flip-flop-count difference from the
> reference, which is a finding to explain, not a failure: these designs pass
> the differential equivalence check.

* `bsg_gray_to_binary` — reference synth failed

## M2b — post-synthesis area vs a SAME-AUTHOR reference

| module | Copper cells | reference cells | ratio | FFs (Copper/ref) | source |
|---|---|---|---|---|---|
| `ram_read_first` | 347 | 320 | 1.084 | 136/136 | `tests/fixtures/write_first_ram_dut.rs` |
| `bit_not_bits` | 32 | 32 | 1.0 | 0/0 | `tests/fixtures/bits_ops_dut.rs` |
| `handshake` | 38 | 43 | 0.884 | 18/18 | `examples/sequential/handshake.rs` |

> **Scope.** Each reference here is a second spelling of the module by its OWN
> author (`build.rs`'s REFERENCE table), not third-party hardware. It answers the
> area question — does the lowering cost more logic than a human's? — but it is
> **not** independent evidence: never average these into the BaseJump table
> above, and never call one an anchor.


## M1 — design size vs the reference

| module | Copper SLOC | reference SLOC | ratio | emitted SV SLOC |
|---|---|---|---|---|
| `bsg_adder_one_hot` | 18 | 27 | 0.667 | 25 |
| `bsg_counter_up_down` | 22 | 20 | 1.1 | 28 |
| `bsg_dff_en` | 14 | 15 | 0.933 | 12 |
| `bsg_encode_one_hot` | 18 | 40 | 0.45 | 25 |
| `bsg_gray_to_binary` | 10 | 50 | 0.2 | 18 |
| `bsg_mux_one_hot` | 16 | 22 | 0.727 | 23 |
| `sipo_block` | 33 | 17 | 1.941 | 78 |

Mean Copper/reference SLOC over 7 anchored designs: **0.860**.

> **Scope.** Counts the `#[hardware]` module only — not the self-check harness
> that shares the file. Ratios are reported *only* where an independent
> implementation of the same design exists; against Verilog we wrote ourselves
> a ratio would measure our own prose style.

## M6 — transpiler performance

* 36 modules lowered to SystemVerilog
* median **4.5 ms** per module (min 2.8, max 1767.4)
* slowest: `rv32i_cpu_transpilable` at 1767.4 ms

> **Scope.** Release binary, median of repeated runs after a warm-up.
> Simulation throughput vs Verilator is M7 below, not this number.

## M8 — attribute cost: the analysis the `#[hardware]` macro runs

* 36 modules analysed
* median **192 µs** per module (min 57, max 517103)
* slowest: `rv32i_cpu_transpilable` at 517103 µs

> **Scope.** Parse of the function plus the shared control-flow analysis and
> every compile-time rule, in the macro's own order, timed in a release build
> outside `rustc` (`copper-codegen/src/bin/analysis-time.rs`). Excludes the
> token rewrite and `rustc`'s compilation of the generated coroutine, which is
> compiling the design rather than attribute overhead.

## M7 — simulation throughput vs Verilator and Icarus Verilog

| design | cycles | sim (cycles/s) | Verilator (cycles/s) | Icarus (cycles/s) | Verilator/sim | sim/Icarus |
|---|---|---|---|---|---|---|
| `lfsr` | 1,000,000 | 2,158,446 | 14,360,623 | 392,799 | 6.7x | 5.5x |
| `det_110101` | 1,000,000 | 3,718,314 | 14,301,147 | 212,139 | 3.8x | 17.5x |
| `dual_port_ram` | 1,000,000 | 1,697,997 | 10,866,131 | 193,706 | 6.4x | 8.8x |
| `rv32i_cpu_transpilable` | 1,000,000 | 815,240 | 6,231,447 | 18,482 | 7.6x | 44.1x |

> **Scope.** Fixed-cycle timed loop (`tests/sim_throughput.rs`), median of
> repeated runs after a warm-up on every side; excludes compilation, model
> construction, and boot/reset. Single-threaded everywhere; Rust release
> profile, Verilator default + `-O2`, Icarus `iverilog -g2012`/`vvp` (process
> wall-clock minus a `+cycles=0` baseline run — vvp has no in-process clock).
> Identical deterministic stimulus on all sides, and the per-cycle output
> checksums are asserted EQUAL — a row only exists where all simulations
> provably computed the same thing. Both ratio columns read "left is N×
> faster". This is the harness-in-the-loop number a testbench author
> experiences (inputs driven and outputs observed every cycle), not a
> free-running batch number.
