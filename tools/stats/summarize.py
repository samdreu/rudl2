#!/usr/bin/env python3
"""Turn paper/stats/*.csv into SUMMARY.md — paste-ready tables, with their scope."""
import csv, pathlib, statistics, datetime, sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
S = ROOT / "paper" / "stats"
read = lambda n: list(csv.DictReader(open(S / n))) if (S / n).exists() else []

loc, qor, ev, perf = read("loc.csv"), read("qor.csv"), read("evidence.csv"), read("perf.csv")
sperf = read("simperf.csv")
qsa = read("qor_same_author.csv")
ana = read("analysis.csv")
stamp = sys.argv[1] if len(sys.argv) > 1 else datetime.date.today().isoformat()
o = [f"# Evaluation numbers (generated {stamp})",
     "", "Regenerate with `tools/stats/collect.sh`. **Do not edit by hand** — every",
     "number here is derived from the repo, and a hand-copied number goes stale.", ""]

if ev:
    n = len(ev)
    f = lambda p: sum(1 for r in ev if p(r))
    o += ["## M4/M5 — evidence coverage", "",
          "| property | count |", "|---|---|",
          f"| example `#[hardware]` modules | {n} |",
          f"| transpile to SystemVerilog | {f(lambda r: r['transpiles']=='yes')}/{n} |",
          f"| covered by the differential sweep (sim vs Verilator, seeded random) | {f(lambda r: r['swept']=='yes')}/{n} |",
          f"| with a dedicated equivalence test | {f(lambda r: r['dedicated_test'])}/{n} |",
          f"| anchored to third-party hardware (BaseJump STL) | {f(lambda r: r['third_party_anchor'])}/{n} |",
          f"| **anchored AND differentially swept** | {f(lambda r: r['third_party_anchor'] and r['swept']=='yes')}/{n} |",
          "",
          "The last row is the transitive chain: the *generated* SystemVerilog is tied to",
          "hardware neither Copper nor its transpiler wrote.", "",
          "Not swept, each with the reviewed reason from `build.rs`'s `SKIP`:", ""]
    for r in ev:
        if r["swept"] == "no":
            o.append(f"* `{r['module']}` — {r['skip_reason']}")
    o += ["", "> **Scope.** The RISC-V CPUs are in that list. `tests/rv32i_integration.rs` is a",
          "> simulator self-check against known program results, **not** a sim≡synth check.",
          "> No sentence may place the CPU beside the equivalence claim.", ""]

if qor:
    ok = [r for r in qor if r["status"] == "ok"]
    o += ["## M2 — post-synthesis area vs the reference (Yosys, generic cells)", "",
          "| module | Copper cells | reference cells | ratio | FFs (Copper/ref) |", "|---|---|---|---|---|"]
    for r in ok:
        flag = "" if r["ff_match"] == "yes" else " ⚠"
        o.append(f"| `{r['module']}` | {r['copper_cells']} | {r['ref_cells']} | "
                 f"{r['cell_ratio']} | {r['copper_ffs']}/{r['ref_ffs']}{flag} |")
    if ok:
        ident = sum(1 for r in ok if r["copper_cells"] == r["ref_cells"])
        rats = [float(r["cell_ratio"]) for r in ok if r["cell_ratio"]]
        o += ["", f"**{ident}/{len(ok)} synthesise to an identical cell count** as the hand-written",
              f"BaseJump reference; mean ratio {statistics.mean(rats):.3f} "
              f"(range {min(rats):.3f}–{max(rats):.3f}).", "",
              "> **Scope.** Yosys `synth` to generic cells, not a vendor flow — comparative,",
              "> not absolute silicon. A ⚠ marks a flip-flop-count difference from the",
              "> reference, which is a finding to explain, not a failure: these designs pass",
              "> the differential equivalence check.", ""]
    for r in qor:
        if r["status"] != "ok":
            o.append(f"* `{r['module']}` — {r['status']}"
                     + (f": {r['note']}" if r.get("note") else ""))
    o.append("")

if qsa:
    ok = [r for r in qsa if r["status"] == "ok"]
    o += ["## M2b — post-synthesis area vs a SAME-AUTHOR reference", "",
          "| module | Copper cells | reference cells | ratio | FFs (Copper/ref) | source |",
          "|---|---|---|---|---|---|"]
    for r in ok:
        flag = "" if r["ff_match"] == "yes" else " \u26a0"
        o.append(f"| `{r['module']}` | {r['copper_cells']} | {r['ref_cells']} | "
                 f"{r['cell_ratio']} | {r['copper_ffs']}/{r['ref_ffs']}{flag} | `{r.get('source','')}` |")
    o += ["", "> **Scope.** Each reference here is a second spelling of the module by its OWN",
          "> author (`build.rs`'s REFERENCE table), not third-party hardware. It answers the",
          "> area question — does the lowering cost more logic than a human's? — but it is",
          "> **not** independent evidence: never average these into the BaseJump table",
          "> above, and never call one an anchor.", ""]
    for r in qsa:
        if r["status"] != "ok":
            o.append(f"* `{r['module']}` — {r['status']}"
                     + (f": {r['note']}" if r.get("note") else ""))
    o.append("")

if loc:
    anc = [r for r in loc if r.get("copper_vs_reference")]
    o += ["## M1 — design size vs the reference", "",
          "| module | Copper SLOC | reference SLOC | ratio | emitted SV SLOC |", "|---|---|---|---|---|"]
    for r in anc:
        o.append(f"| `{r['module']}` | {r['copper_sloc']} | {r['reference_sv_sloc']} | "
                 f"{r['copper_vs_reference']} | {r['emitted_sv_sloc']} |")
    if anc:
        m = statistics.mean(float(r["copper_vs_reference"]) for r in anc)
        o += ["", f"Mean Copper/reference SLOC over {len(anc)} anchored designs: **{m:.3f}**.", "",
              "> **Scope.** Counts the `#[hardware]` module only — not the self-check harness",
              "> that shares the file. Ratios are reported *only* where an independent",
              "> implementation of the same design exists; against Verilog we wrote ourselves",
              "> a ratio would measure our own prose style.", ""]

if perf:
    med = [float(r["median_ms"]) for r in perf]
    slow = max(perf, key=lambda r: float(r["median_ms"]))
    o += ["## M6 — transpiler performance", "",
          f"* {len(perf)} modules lowered to SystemVerilog",
          f"* median **{statistics.median(med):.1f} ms** per module "
          f"(min {min(med):.1f}, max {max(med):.1f})",
          f"* slowest: `{slow['module']}` at {slow['median_ms']} ms", "",
          "> **Scope.** Release binary, median of repeated runs after a warm-up.",
          "> Simulation throughput vs Verilator is M7 below, not this number.", ""]

if ana:
    amed = [float(r["median_us"]) for r in ana]
    aslow = max(ana, key=lambda r: float(r["median_us"]))
    o += ["## M8 — attribute cost: the analysis the `#[hardware]` macro runs", "",
          f"* {len(ana)} modules analysed",
          f"* median **{statistics.median(amed):.0f} µs** per module "
          f"(min {min(amed):.0f}, max {max(amed):.0f})",
          f"* slowest: `{aslow['module']}` at {float(aslow['median_us']):.0f} µs", "",
          "> **Scope.** Parse of the function plus the shared control-flow analysis and",
          "> every compile-time rule, in the macro's own order, timed in a release build",
          "> outside `rustc` (`copper-codegen/src/bin/analysis-time.rs`). Excludes the",
          "> token rewrite and `rustc`'s compilation of the generated coroutine, which is",
          "> compiling the design rather than attribute overhead.", ""]

if sperf:
    o += ["## M7 — simulation throughput vs Verilator and Icarus Verilog", "",
          "| design | cycles | sim (cycles/s) | Verilator (cycles/s) | Icarus (cycles/s) | Verilator/sim | sim/Icarus |",
          "|---|---|---|---|---|---|---|"]
    for r in sperf:
        iv = f"{float(r['iverilog_cycles_per_sec']):,.0f}" if r.get("iverilog_cycles_per_sec") else "—"
        ivr = f"{r['sim_over_iverilog']}x" if r.get("sim_over_iverilog") else "—"
        o.append(f"| `{r['design']}` | {int(r['cycles']):,} | "
                 f"{float(r['sim_cycles_per_sec']):,.0f} | "
                 f"{float(r['verilator_cycles_per_sec']):,.0f} | {iv} | "
                 f"{r['verilator_over_sim']}x | {ivr} |")
    o += ["",
          "> **Scope.** Fixed-cycle timed loop (`tests/sim_throughput.rs`), median of",
          "> repeated runs after a warm-up on every side; excludes compilation, model",
          "> construction, and boot/reset. Single-threaded everywhere; Rust release",
          "> profile, Verilator default + `-O2`, Icarus `iverilog -g2012`/`vvp` (process",
          "> wall-clock minus a `+cycles=0` baseline run — vvp has no in-process clock).",
          "> Identical deterministic stimulus on all sides, and the per-cycle output",
          "> checksums are asserted EQUAL — a row only exists where all simulations",
          "> provably computed the same thing. Both ratio columns read \"left is N×",
          "> faster\". This is the harness-in-the-loop number a testbench author",
          "> experiences (inputs driven and outputs observed every cycle), not a",
          "> free-running batch number.", ""]

(S / "SUMMARY.md").write_text("\n".join(o))
print(f"-> {S/'SUMMARY.md'} ({len(o)} lines)")
