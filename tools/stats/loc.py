#!/usr/bin/env python3
"""M1 — design size: Copper source vs the independent reference vs the emitted SV.

Only the `examples/basejump/` pairs get a RATIO, because only there does an
independent implementation of the same design exist. Everything else reports its own
size and the size of what it lowers to, with no ratio, because there is nothing
honest to divide by.

Counts non-blank, non-comment lines, and counts only the MODULE — from the
`#[hardware(...)]` attribute to the closing brace of its `async fn`. An
`examples/*.rs` file also carries a full self-check harness (`main`, stimulus
vectors, a Verilator comparison); including that made the first version of this
script report Copper as 3.7x LARGER than the reference Verilog, which measures our
test harness against their design and is meaningless.
    tools/stats/loc.py            # -> paper/stats/loc.csv
"""
import csv, pathlib, re, subprocess, sys, tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "paper" / "stats"
CLI = ["cargo", "run", "-q", "-p", "copper-codegen", "--bin", "copper-transpile", "--"]

def hardware_module_lines(path):
    """The `#[hardware…] async fn …{…}` spans only, brace-matched."""
    lines = pathlib.Path(path).read_text(errors="replace").splitlines()
    out, i = [], 0
    while i < len(lines):
        if lines[i].lstrip().startswith("#[hardware"):
            depth, started, j = 0, False, i
            while j < len(lines):
                out.append(lines[j])
                depth += lines[j].count("{") - lines[j].count("}")
                if "{" in lines[j]: started = True
                if started and depth <= 0: break
                j += 1
            i = j + 1
            continue
        i += 1
    return out

def sloc(path, line_comment=("//",), only_module=False):
    n, in_block = 0, False
    src = hardware_module_lines(path) if only_module else \
          pathlib.Path(path).read_text(errors="replace").splitlines()
    for raw in src:
        s = raw.strip()
        if in_block:
            if "*/" in s: in_block = False
            continue
        if not s: continue
        if s.startswith("/*"):
            if "*/" not in s: in_block = True
            continue
        if any(s.startswith(c) for c in line_comment): continue
        n += 1
    return n

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="copper_loc_"))
    rows = []
    for rs in sorted((ROOT / "examples").rglob("*.rs")):
        rel = rs.relative_to(ROOT)
        name = rs.stem
        ref = ROOT / "examples" / "basejump" / "sv" / f"{name}.sv"
        gen = tmp / f"{name}.sv"
        ok = subprocess.run(CLI + [str(rs), "-o", str(gen)],
                            capture_output=True, cwd=ROOT).returncode == 0
        row = dict(module=name, path=str(rel), copper_sloc=sloc(rs, only_module=True),
                   emitted_sv_sloc=sloc(gen) if ok else "",
                   reference_sv_sloc=sloc(ref) if ref.exists() else "",
                   scope="third-party reference (BaseJump STL)" if ref.exists()
                         else "no independent reference — ratio not meaningful")
        if ref.exists():
            row["copper_vs_reference"] = round(row["copper_sloc"] / row["reference_sv_sloc"], 3)
        rows.append(row)

    cols = ["module", "path", "copper_sloc", "emitted_sv_sloc", "reference_sv_sloc",
            "copper_vs_reference", "scope"]
    with open(OUT / "loc.csv", "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=cols, extrasaction="ignore")
        w.writeheader(); w.writerows(rows)

    anchored = [r for r in rows if r.get("copper_vs_reference")]
    print(f"{'module':<26} {'copper':>7} {'ref':>6} {'ratio':>7} {'emitted SV':>11}")
    for r in anchored:
        print(f"{r['module']:<26} {r['copper_sloc']:>7} {r['reference_sv_sloc']:>6} "
              f"{r['copper_vs_reference']:>7} {str(r['emitted_sv_sloc']):>11}")
    if anchored:
        rat = sum(r["copper_vs_reference"] for r in anchored) / len(anchored)
        print(f"\nmean Copper/reference SLOC over {len(anchored)} anchored designs: {rat:.3f}")
    print(f"{len(rows)} example modules measured -> {OUT/'loc.csv'}")

main()
