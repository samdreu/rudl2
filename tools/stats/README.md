# Evaluation statistics harness

Generates the numbers for the paper's results/evaluation section. One command:

```bash
tools/stats/collect.sh          # writes paper/stats/*.csv + paper/stats/SUMMARY.md
```

Everything is regenerated from the repo on every run. **No number in the paper should
be typed by hand** — that is how the transpiler coverage count went stale (see `TODO`,
"STOP RE-MEASURING THIS BY HAND").

## What the literature reports, and what we therefore collect

Surveyed to pick these: Filament (PLDI'23), Calyx (ASPLOS'21), Kôika (PLDI'20),
Chisel (DAC'12), PyMTL, Clash, Spade (TRETS'26), Prost (LATTE'26), RHDL (LATTE'25).

| # | Metric family | Who reports it | Our collector |
|---|---|---|---|
| M1 | **Design size** — LOC of the design vs a reference implementation | near-universal; the standard "productivity" argument | `loc.py` |
| M2 | **QoR: area** — post-synthesis cells / FFs / LUTs vs a baseline | Filament (LUT/FF/DSP via Vivado), Chisel ("competitive in area and power"), Reticle, Aetherling | `qor.py` (Yosys) |
| M3 | **QoR: timing** — critical path / Fmax | Filament, Chisel, most FPGA-facing work | **not implemented** — needs a liberty file; see below |
| M4 | **Correctness evidence** — designs verified, property proved, bugs found | Filament (found 4 designs producing wrong output), Kôika (proved semantics) | `equivalence.py` |
| M5 | **Expressiveness / coverage** — how many designs, which constructs | everyone, usually as a case-study table | `equivalence.py` |
| M6 | **Compiler performance** — elaboration/compile time | Calyx, Filament | `perf.py` |
| M7 | **Simulation throughput** — cycles/s of the host-language simulator vs a compiled RTL simulator | Kôika (Cuttlesim vs Verilator), PyMTL, Clash; MyHDL/cocotb folklore | `simperf.py` |

Two findings from the survey worth keeping in mind while writing:

* **A language-design paper in this genre can ship with no quantitative evaluation
  at all.** Spade (TRETS'26) goes Implementation → Related Work → Target Audience →
  Future Work; there is no evaluation section and no table of numbers. So M1-M6 are
  an opportunity to be *more* rigorous than the closest published comparable, not a
  bar we must clear to be publishable.
* **The strongest evaluations in this line are correctness results, not QoR
  results.** Filament's headline evaluation number is that four Aetherling-generated
  designs produce incorrect output under the latencies Aetherling itself reports —
  a bug-finding claim. That is the shape of M4, and it is the family where Copper
  has something the comparables do not: an independent third-party hardware anchor
  (BaseJump STL) *plus* a machine-checked sim≡synth differential.

## The honesty rules these scripts enforce

Each collector emits a `scope` column and `SUMMARY.md` reprints it, because every
one of these numbers is bounded:

* **M1 LOC** is only meaningful where an independent reference implementation of the
  *same* design exists — the `examples/basejump/` pairs. A LOC ratio against Verilog
  we wrote ourselves would be measuring our own prose style.
* **M2/M3 QoR** compares *Copper-generated* SV against the *reference* Verilog for
  the same module. Yosys `synth` to generic cells, not a vendor flow, so the numbers
  are comparative, not absolute silicon.
* **M4** must never put the RISC-V CPU next to the equivalence claim: the CPU is a
  simulator self-check against known program results, **not** a sim≡synth check.
  `equivalence.py` classifies every module by *which* evidence it has, precisely so
  that sentence cannot be written by accident (`paper/00_claims_audit.md`, §Scope).

## Files

| script | metric | needs |
|---|---|---|
| `equivalence.py` | M4/M5 evidence census | the debug `copper-transpile` |
| `loc.py` | M1 design size | the debug `copper-transpile` |
| `qor.py` | M2 post-synthesis area | `yosys` (`brew install yosys`) |
| `perf.py` | M6 transpile time | the release `copper-transpile` |
| `simperf.py` | M7 simulation throughput | Verilator + Icarus (`brew install icarus-verilog`) — both required, the metric IS the comparison |
| `summarize.py` | rolls the CSVs into `SUMMARY.md` | — |

Outputs land in `paper/stats/`: one CSV per collector plus `SUMMARY.md`.

## Not implemented — do not invent these numbers

* **M3 timing / Fmax.** Yosys can report a critical path only against a liberty
  file; a generic-cell path length is not a frequency. Either add a liberty target
  (e.g. an open PDK) or drop the metric — do not report a unitless "levels of logic"
  number as if it were timing.

## M7 methodology (the fixed-cycle benchmark harness)

`simperf.py` drives `tests/sim_throughput.rs`, which for each benchmarked design
(a small sequential datapath, an FSM, a `Memory`-backed RAM, and the RV32I CPU
running a non-halting store/load/branch loop whose branch pattern depends on the
loaded data) times a fixed-cycle loop in the Copper simulator AND in two
independent simulators running the SystemVerilog transpiled from the same
module: Verilator (the compiled-simulator ceiling) and Icarus Verilog (the
interpreted event-driven baseline):

* **Self-checking by construction.** Both sides run identical deterministic
  stimulus and fold every cycle's post-edge outputs into a checksum; the harness
  asserts the two checksums are EQUAL. A throughput number is only reported for
  two simulations that provably computed the same thing — and each benchmark run
  is thereby also a longer-horizon differential check than the corpus sweep runs.
* **What is timed.** The cycle loop only — compilation, model construction, and
  boot/reset are excluded on every side; stimulus generation and the checksum
  fold are included on every side (identical trivial integer ops). Median of
  repeated runs after a warm-up. Single-threaded everywhere; Rust release
  profile, Verilator default + `-O2`, Icarus `iverilog -g2012`/`vvp`. The
  Verilator testbench is a counted loop, not the unrolled per-cycle testbench
  the equivalence checks generate — that one's compile time scales with the
  trace and would poison the measurement. Icarus has no in-process clock a
  testbench can read, so its number is vvp process wall-clock minus a
  `+cycles=0` baseline run of the same binary (startup, bytecode load, and boot
  cancel out) — meaningless at smoke-mode cycle counts, fine at 1M.
* **4-state honesty.** The emitted SV leaves registers uninitialized; 2-state
  Verilator zero-fills them (accidentally matching the simulator's zero inits),
  4-state Icarus keeps them X. The harness never folds a value before it has
  been architecturally written (reset cycles, a RAM-filling warm-up, 16
  post-reset cycles on the CPU), so the checksum equality is real on all three —
  an `x` in an Icarus checksum means the warm-up is wrong, not the parser.
* **What the number means.** Inputs are driven and outputs observed every cycle
  — the harness-in-the-loop throughput a testbench author experiences, not a
  free-running batch number. First three-way measurement (2026-09-01): the
  Copper simulator sits BETWEEN the two Verilog simulators — Verilator is
  4.6–15.1x faster than Copper, and Copper is 9.5–46.8x faster than Icarus,
  running the RV32I CPU at ~0.8M cycles/s (Verilator 12.2M, Icarus 0.03M). So
  the honest sentence is: within roughly one order of magnitude of the compiled
  simulator, one to two orders faster than the interpreted one — with no
  compile step and native debugging on top. Regenerate rather than quote these
  numbers.
* **Rot guard.** Under a bare `cargo test` the harness runs in smoke mode (small
  cycle count, checksum cross-check still enforced), so regression guard G-C
  keeps it from silently not running. The CSV is refused from a debug build.

## Known findings the harness surfaced

* `sipo_block` synthesises to **32 flip-flops against the reference's 30**, while
  passing the differential equivalence check — behaviourally equivalent, structurally
  not. It is also the only anchored design larger than its reference in SLOC (1.94x).
  Worth explaining before it appears in a table.
* `bsg_encode_one_hot` costs **1.36x** the reference's cells.
* `bsg_gray_to_binary`'s *reference* does not synthesise under Yosys, so it has no
  QoR row — a limitation of the baseline, not of Copper.
