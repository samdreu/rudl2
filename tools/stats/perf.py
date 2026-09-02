#!/usr/bin/env python3
"""M6 — compiler performance: wall-clock to lower one module to SystemVerilog.

Measured against the RELEASE binary and after a warm-up run, because a debug build
of the transpiler is roughly an order of magnitude slower and a cold first
invocation pays page-in cost. Reports the median of N runs.

NOT measured here (do not invent these numbers):
  * simulation throughput (cycles/s) vs Verilator — that is M7, tools/stats/simperf.py.
  * end-to-end synthesis time — dominated by the vendor tool, not by Copper.

    tools/stats/perf.py [--runs N]        # -> paper/stats/perf.csv
"""
import argparse, csv, pathlib, statistics, subprocess, sys, tempfile, time

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "paper" / "stats"

ap = argparse.ArgumentParser()
ap.add_argument("--runs", type=int, default=5)
args = ap.parse_args()

print("building the release transpiler ...", file=sys.stderr)
subprocess.run(["cargo", "build", "-q", "--release", "-p", "copper-codegen",
                "--bin", "copper-transpile"], cwd=ROOT, check=True)
BIN = ROOT / "target" / "release" / "copper-transpile"

def modules(path):
    r = subprocess.run([str(BIN), str(path), "--list"], capture_output=True, text=True, cwd=ROOT)
    return [l.strip() for l in r.stdout.splitlines()[1:] if l.strip()] if r.returncode == 0 else []

OUT.mkdir(parents=True, exist_ok=True)
tmp = pathlib.Path(tempfile.mkdtemp(prefix="copper_perf_"))
rows = []
for rs in sorted((ROOT / "examples").rglob("*.rs")):
    mods = modules(rs)
    for mod in mods:
        cmd = [str(BIN), str(rs), "-o", str(tmp / f"{mod}.sv")]
        if len(mods) > 1:
            cmd += ["--module", mod]
        if subprocess.run(cmd, capture_output=True, cwd=ROOT).returncode != 0:
            continue                                    # covered by evidence.csv
        ts = []
        for _ in range(args.runs):
            t0 = time.perf_counter()
            subprocess.run(cmd, capture_output=True, cwd=ROOT)
            ts.append((time.perf_counter() - t0) * 1000)
        sv = tmp / f"{mod}.sv"
        rows.append(dict(module=mod, file=str(rs.relative_to(ROOT)),
                         median_ms=round(statistics.median(ts), 1),
                         min_ms=round(min(ts), 1),
                         emitted_bytes=sv.stat().st_size if sv.exists() else 0))

with open(OUT / "perf.csv", "w", newline="") as f:
    w = csv.DictWriter(f, fieldnames=["module", "file", "median_ms", "min_ms", "emitted_bytes"])
    w.writeheader(); w.writerows(rows)

if rows:
    med = [r["median_ms"] for r in rows]
    print(f"{len(rows)} modules lowered to SystemVerilog")
    print(f"  median  {statistics.median(med):.1f} ms")
    print(f"  min/max {min(med):.1f} / {max(med):.1f} ms")
    slow = max(rows, key=lambda r: r["median_ms"])
    print(f"  slowest {slow['module']} at {slow['median_ms']} ms")
print(f"-> {OUT/'perf.csv'}")
