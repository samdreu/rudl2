#!/usr/bin/env python3
"""M2/M3 — post-synthesis quality of results, Copper-generated SV vs the reference.

For every Copper module that has an INDEPENDENT reference implementation, synthesise
both with Yosys to generic cells and compare. Comparing against Verilog we wrote
ourselves would measure our own style; the `examples/basejump/sv/` references are
third-party (BaseJump STL, Solderpad), which is what makes the comparison mean
anything.

Yosys `synth` to generic cells is deliberately NOT a vendor flow: the numbers are
comparative (does Copper's lowering cost more logic than a human's?), not absolute
silicon area.

    tools/stats/qor.py            # -> paper/stats/qor.csv
"""
import csv, os, re, subprocess, sys, tempfile, pathlib

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "paper" / "stats"
CLI = ["cargo", "run", "-q", "-p", "copper-codegen", "--bin", "copper-transpile", "--"]

FF_CELLS = re.compile(r"\$_(S?DFFE?|DFFSRE?|SDFFCE)_")

def yosys_stat(sv_path, top):
    """(total_cells, ff_bits, cell_histogram) after `synth -top`, or None if it fails."""
    script = f"read_verilog -sv {sv_path}; synth -top {top}; stat"
    try:
        r = subprocess.run(["yosys", "-p", script], capture_output=True, text=True, timeout=300)
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return None
    if r.returncode != 0:
        return None
    # the LAST "=== <top> ===" block is the post-synth one
    blocks = r.stdout.split(f"=== {top} ===")
    if len(blocks) < 2:
        return None
    tail, cells, ffs, hist = blocks[-1], 0, 0, {}
    for line in tail.splitlines():
        m = re.match(r"\s+(\d+)\s+cells\s*$", line)
        if m:
            cells = int(m.group(1)); continue
        m = re.match(r"\s+(\d+)\s+(\$[_A-Za-z0-9]+)\s*$", line)
        if m:
            n, cell = int(m.group(1)), m.group(2)
            hist[cell] = hist.get(cell, 0) + n
            if FF_CELLS.match(cell):
                ffs += n
    return cells, ffs, hist

def transpile(src, module, dst):
    cmd = CLI + [str(src), "-o", str(dst)]
    if module:
        cmd += ["--module", module]
    r = subprocess.run(cmd, capture_output=True, text=True, cwd=ROOT)
    return r.returncode == 0, (r.stderr or r.stdout).strip().splitlines()[-1:] or [""]

def main():
    if subprocess.run(["which", "yosys"], capture_output=True).returncode != 0:
        print("yosys not installed — skipping QoR (brew install yosys)", file=sys.stderr)
        return 1
    OUT.mkdir(parents=True, exist_ok=True)
    rows, tmp = [], pathlib.Path(tempfile.mkdtemp(prefix="copper_qor_"))

    for rs in sorted((ROOT / "examples" / "basejump").glob("*.rs")):
        name = rs.stem
        ref = ROOT / "examples" / "basejump" / "sv" / f"{name}.sv"
        if not ref.exists():
            continue
        gen = tmp / f"{name}.copper.sv"
        ok, err = transpile(rs, None, gen)
        if not ok:
            rows.append(dict(module=name, status="does not transpile", note=err[0][:90]))
            continue
        c, r_ = yosys_stat(gen, name), yosys_stat(ref, name)
        if c is None or r_ is None:
            rows.append(dict(module=name,
                             status="copper synth failed" if c is None else "reference synth failed"))
            continue
        (cc, cf, ch), (rc, rf, rh) = c, r_
        rows.append(dict(module=name, status="ok",
                         copper_cells=cc, ref_cells=rc,
                         cell_ratio=round(cc / rc, 3) if rc else "",
                         copper_ffs=cf, ref_ffs=rf,
                         ff_match="yes" if cf == rf else "NO",
                         copper_hist=" ".join(f"{k}x{v}" for k, v in sorted(ch.items())),
                         ref_hist=" ".join(f"{k}x{v}" for k, v in sorted(rh.items()))))

    cols = ["module", "status", "copper_cells", "ref_cells", "cell_ratio",
            "copper_ffs", "ref_ffs", "ff_match", "copper_hist", "ref_hist", "note"]
    with open(OUT / "qor.csv", "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=cols, extrasaction="ignore")
        w.writeheader(); w.writerows(rows)

    okr = [r for r in rows if r.get("status") == "ok"]
    print(f"{'module':<24} {'copper':>7} {'ref':>7} {'ratio':>7} {'FFs':>10}")
    for r in rows:
        if r.get("status") != "ok":
            print(f"{r['module']:<24}   {r['status']}"); continue
        print(f"{r['module']:<24} {r['copper_cells']:>7} {r['ref_cells']:>7} "
              f"{r['cell_ratio']:>7} {str(r['copper_ffs'])+'/'+str(r['ref_ffs']):>10}"
              f"{'' if r['ff_match']=='yes' else '   <- FF MISMATCH'}")
    if okr:
        idn = sum(1 for r in okr if r["copper_cells"] == r["ref_cells"])
        print(f"\n{idn}/{len(okr)} synthesise to an IDENTICAL cell count as the reference")
    print(f"-> {OUT/'qor.csv'}")
    return 0

if __name__ == "__main__":
    sys.exit(main())
