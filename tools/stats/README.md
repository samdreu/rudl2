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
| `summarize.py` | rolls the CSVs into `SUMMARY.md` | — |

Outputs land in `paper/stats/`: one CSV per collector plus `SUMMARY.md`.

## Not implemented — do not invent these numbers

* **M3 timing / Fmax.** Yosys can report a critical path only against a liberty
  file; a generic-cell path length is not a frequency. Either add a liberty target
  (e.g. an open PDK) or drop the metric — do not report a unitless "levels of logic"
  number as if it were timing.
* **Simulation throughput (cycles/s) vs Verilator.** Needs a fixed-cycle benchmark
  harness that does not exist. The example run times are NOT this number: they are
  dominated by Verilator compilation.

## Known findings the harness surfaced

* `sipo_block` synthesises to **32 flip-flops against the reference's 30**, while
  passing the differential equivalence check — behaviourally equivalent, structurally
  not. It is also the only anchored design larger than its reference in SLOC (1.94x).
  Worth explaining before it appears in a table.
* `bsg_encode_one_hot` costs **1.36x** the reference's cells.
* `bsg_gray_to_binary`'s *reference* does not synthesise under Yosys, so it has no
  QoR row — a limitation of the baseline, not of Copper.
