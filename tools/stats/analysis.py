#!/usr/bin/env python3
"""M8 — the cost of the `#[hardware]` attribute: per-module wall-clock of the
analysis the macro runs (parse + the shared control-flow analysis + the compile-time
rules), measured by `copper-codegen/src/bin/analysis-time.rs` outside the compiler.

NOT measured here (do not invent these numbers):
  * rustc's cost of compiling the generated coroutine — that is compiling the
    design, not overhead the attribute adds;
  * the token rewrite itself — a single tree walk, not separable outside rustc.

    tools/stats/analysis.py [--runs N]        # -> paper/stats/analysis.csv
"""
import argparse, csv, io, pathlib, statistics, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "paper" / "stats"

ap = argparse.ArgumentParser()
ap.add_argument("--runs", type=int, default=20)
args = ap.parse_args()

print("building the release analysis-time binary ...", file=sys.stderr)
subprocess.run(["cargo", "build", "-q", "--release", "-p", "copper-codegen",
                "--bin", "analysis-time"], cwd=ROOT, check=True)
BIN = ROOT / "target" / "release" / "analysis-time"
r = subprocess.run([str(BIN), "--runs", str(args.runs)], capture_output=True, text=True, cwd=ROOT, check=True)
rows = list(csv.DictReader(io.StringIO(r.stdout)))

OUT.mkdir(parents=True, exist_ok=True)
with open(OUT / "analysis.csv", "w", newline="") as f:
    w = csv.DictWriter(f, fieldnames=["module", "file", "mode", "median_us", "min_us"])
    w.writeheader(); w.writerows(rows)

if rows:
    med = [float(x["median_us"]) for x in rows]
    slow = max(rows, key=lambda x: float(x["median_us"]))
    print(f"{len(rows)} modules analysed")
    print(f"  median  {statistics.median(med):.0f} us")
    print(f"  min/max {min(med):.0f} / {max(med):.0f} us")
    print(f"  slowest {slow['module']} at {float(slow['median_us']):.0f} us")
print(f"-> {OUT/'analysis.csv'}")
