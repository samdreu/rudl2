#!/usr/bin/env python3
"""M4/M5 — what evidence exists for each design, and what each piece of evidence proves.

The point of this census is to make it IMPOSSIBLE to write the sentence the claims
audit warns about: the RISC-V CPU is the most impressive design in the repo and is
NOT covered by the equivalence claim (it is a simulator self-check against known
program results). Every module is therefore classified by WHICH evidence it has:

  transpiles          the CLI emits SystemVerilog for it
  swept               the corpus differential sweep runs it (sim vs Verilator on
                      seeded random stimulus) — i.e. it is not in build.rs's SKIP
  dedicated_test      a hand-written tests/*_equivalence.rs targets it
  third_party_anchor  checked against independent hardware (BaseJump STL Verilog)

    tools/stats/equivalence.py        # -> paper/stats/evidence.csv
"""
import csv, pathlib, re, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "paper" / "stats"
CLI = ["cargo", "run", "-q", "-p", "copper-codegen", "--bin", "copper-transpile", "--"]

def skip_table():
    """build.rs's SKIP: module -> the reviewed sentence saying why it is not swept.

    Scanned rather than regex-matched over the whole tuple: the reasons are Rust
    string literals with `\\`-continuations, backticks and em-dashes, and a single
    clever pattern silently dropped three of the twelve entries.
    """
    src = (ROOT / "build.rs").read_text()
    body = src[src.index("const SKIP"):]
    body = body[:body.index("\n];")]
    names = [(m.start(), m.end(), m.group(1)) for m in re.finditer(r'\(\s*"([^"]+)"\s*,', body)]
    out = {}
    for i, (start, after, name) in enumerate(names):
        end = names[i + 1][0] if i + 1 < len(names) else len(body)
        chunk = body[after:end]
        parts = re.findall(r'"((?:[^"\\]|\\.)*)"', chunk, re.S)
        reason = " ".join(parts).replace("\\\n", " ")
        out[name.split("::")[-1]] = re.sub(r"\s+", " ", reason).strip()
    return out

BIN = ROOT / "target" / "debug" / "copper-transpile"

def modules_in(path):
    """Ask the CLI, never a hand-rolled attribute scan.

    A regex over `#[hardware...]` silently drops any module carrying a flag
    (`allow_pretick_alignment`, `structural`, ...) — the exact bug class CLAUDE.md
    records for `parse_args::<syn::Ident>()`, which has already cost this repo three
    silently-incomplete corpus scans.
    """
    r = subprocess.run([str(BIN), str(path), "--list"], capture_output=True, text=True, cwd=ROOT)
    if r.returncode != 0:
        return []
    return [ln.strip() for ln in r.stdout.splitlines()[1:] if ln.strip()]

def main():
    OUT.mkdir(parents=True, exist_ok=True)
    skips = skip_table()
    test_txt = {p: p.read_text(errors="replace")
                for p in list((ROOT/"tests").rglob("*.rs"))
                       + list(ROOT.glob("*/tests/*.rs"))}
    rows = []
    for rs in sorted((ROOT / "examples").rglob("*.rs")):
        mods = modules_in(rs)
        for mod in mods:
            cmd = CLI + [str(rs)] + (["--module", mod] if len(mods) > 1 else [])
            r = subprocess.run(cmd, capture_output=True, text=True, cwd=ROOT)
            transpiles = r.returncode == 0
            why = ""
            if not transpiles:
                tail = (r.stderr or r.stdout).strip().splitlines()
                why = re.sub(r"\s+", " ", tail[-1])[:120] if tail else ""
            dedicated = sorted({p.name for p, t in test_txt.items()
                                if re.search(rf"\b{re.escape(mod)}\b", t)
                                and "equivalence" in p.name})
            rows.append(dict(
                module=mod, file=str(rs.relative_to(ROOT)),
                transpiles="yes" if transpiles else "no",
                blocked_by=why,
                swept="no" if mod in skips else "yes",
                skip_reason=skips.get(mod, ""),
                dedicated_test=";".join(dedicated),
                third_party_anchor="BaseJump STL" if "basejump" in str(rs) else "",
            ))

    cols = ["module", "file", "transpiles", "blocked_by", "swept", "skip_reason",
            "dedicated_test", "third_party_anchor"]
    with open(OUT / "evidence.csv", "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=cols, extrasaction="ignore")
        w.writeheader(); w.writerows(rows)

    n = len(rows)
    tr = sum(r["transpiles"] == "yes" for r in rows)
    sw = sum(r["swept"] == "yes" for r in rows)
    dt = sum(bool(r["dedicated_test"]) for r in rows)
    an = sum(bool(r["third_party_anchor"]) for r in rows)
    both = sum(1 for r in rows if r["third_party_anchor"] and r["swept"] == "yes")
    print(f"example #[hardware] modules ............ {n}")
    print(f"  transpile to SystemVerilog .......... {tr}/{n}")
    print(f"  covered by the differential sweep ... {sw}/{n}   (sim vs Verilator, seeded random)")
    print(f"  with a dedicated equivalence test ... {dt}/{n}")
    print(f"  anchored to third-party hardware .... {an}/{n}   (BaseJump STL)")
    print(f"  BOTH anchored AND swept ............. {both}/{n}   <- the transitive chain")
    print("\nnot swept, with the reviewed reason:")
    for r in rows:
        if r["swept"] == "no":
            print(f"  {r['module']:<24} {r['skip_reason'][:88]}")
    print(f"\n-> {OUT/'evidence.csv'}")

main()
