#!/usr/bin/env python3
"""M7 — simulation throughput: Copper's simulator vs Verilator on the SAME design.

Thin driver over the harness in tests/sim_throughput.rs, which does the real
work: for each benchmarked design it runs a fixed-cycle timed loop in the Copper
simulator AND in a Verilated build of the SystemVerilog transpiled from that
same module, under identical deterministic stimulus, and asserts the per-cycle
output checksums of the two sides are EQUAL — so a throughput number is only
ever reported for two simulations that provably computed the same thing.

Excluded from the measurement on both sides: Verilator compilation, model
construction, and boot/reset. Included on both sides: stimulus generation and
the checksum fold (identical trivial integer ops). Single-threaded both sides;
release profile vs Verilator default + -O2.

Also runs the SAME transpiled SV under Icarus Verilog (`iverilog -g2012` + `vvp`)
— the interpreted event-driven baseline — with the same stimulus and the same
checksum requirement; its time is vvp process wall-clock minus a `+cycles=0`
baseline run, since vvp has no in-process clock.

Verilator AND Icarus are REQUIRED here — the metric is the comparison; there is
no honest partial table to write.

    tools/stats/simperf.py [--cycles N] [--runs K]    # -> paper/stats/simperf.csv
"""
import argparse, csv, os, pathlib, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "paper" / "stats"

ap = argparse.ArgumentParser()
ap.add_argument("--cycles", type=int, default=1_000_000)
ap.add_argument("--runs", type=int, default=5)
args = ap.parse_args()

env = os.environ.copy()
env.pop("VERILATOR_ROOT", None)  # the stale-VERILATOR_ROOT gotcha, same as everywhere

for tool, cmd, brew in [("Verilator", ["verilator", "--version"], "verilator"),
                        ("Icarus Verilog", ["iverilog", "-V"], "icarus-verilog")]:
    try:
        probe = subprocess.run(cmd, capture_output=True, env=env)
    except FileNotFoundError:
        sys.exit(f"simperf: {tool} is not installed, and the metric IS the "
                 f"comparison — install it (brew install {brew}).")
    if probe.returncode != 0:
        sys.exit(f"simperf: `{' '.join(cmd)}` failed — broken install, not a skip:\n"
                 + probe.stderr.decode())

OUT.mkdir(parents=True, exist_ok=True)
env["COPPER_BENCH_CYCLES"] = str(args.cycles)
env["COPPER_BENCH_RUNS"] = str(args.runs)
env["COPPER_BENCH_CSV"] = str(OUT / "simperf.csv")

print(f"running the fixed-cycle benchmark harness (release, {args.cycles} cycles, "
      f"{args.runs} runs + warmup per side) ...", file=sys.stderr)
r = subprocess.run(
    ["cargo", "test", "--release", "-q", "--test", "sim_throughput", "--", "--nocapture"],
    cwd=ROOT, env=env)
if r.returncode != 0:
    sys.exit("simperf: the benchmark harness failed — a checksum mismatch there is a "
             "sim≢SV divergence, not a benchmark problem")

rows = list(csv.DictReader(open(OUT / "simperf.csv")))
print(f"{len(rows)} designs benchmarked, checksums matched across all three simulators")
for r in rows:
    print(f"  {r['design']:<24} sim {float(r['sim_cycles_per_sec']):>12,.0f} cyc/s   "
          f"verilator {float(r['verilator_cycles_per_sec']):>12,.0f} ({r['verilator_over_sim']}x)   "
          f"iverilog {float(r['iverilog_cycles_per_sec']):>10,.0f} (sim {r['sim_over_iverilog']}x faster)")
print(f"-> {OUT/'simperf.csv'}")
