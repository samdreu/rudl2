#!/usr/bin/env python3
"""Dead-code ledger from llvm-cov JSON, with v0-mangling normalised.

Two things inflate a naive count and both are handled here:
  * the crate DISAMBIGUATOR (`Cs<hash>_`) differs per compilation unit, so one
    function appears once per test binary and is "dead" in all the binaries that
    happen not to call it. Aggregate on the demangled path instead.
  * generic INSTANTIATIONS are separate records; a generic fn is dead only if
    every instantiation is.
"""
import json, re, sys, collections

def segments(mangled):
    """Pull the length-prefixed identifier segments out of a v0 symbol."""
    out, i, n = [], 0, len(mangled)
    while i < n:
        if mangled[i].isdigit():
            j = i
            while j < n and mangled[j].isdigit(): j += 1
            ln = int(mangled[i:j])
            seg = mangled[j:j+ln]
            if ln and len(seg) == ln and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", seg or ""):
                out.append(seg); i = j + ln; continue
            i = j
        else:
            i += 1
    return out

def demangle(name):
    segs = [s for s in segments(name) if not re.fullmatch(r"[0-9a-f]{16}", s)]
    # drop the crate-hash noise segments llvm leaves behind
    segs = [s for s in segs if s not in ("B", "E")]
    return "::".join(segs[-3:]) if segs else name

data = json.load(open(sys.argv[1]))["data"][0]
ROOT = sys.argv[2].rstrip("/") + "/"
rel = lambda p: p[len(ROOT):] if p.startswith(ROOT) else p
def keep(p):
    return not (p.startswith(("target/", "/")) or "registry" in p
                or "/tests/" in p or p.startswith("tests/") or p.startswith("examples/"))

files = {}
for f in data.get("files", []):
    p = rel(f["filename"])
    if not keep(p): continue
    s = f["summary"]
    files[p] = {"region_pct": s["regions"]["percent"],
                "uncovered": s["regions"]["notcovered"],
                "regions": s["regions"]["count"]}

agg = collections.defaultdict(lambda: {"count": 0, "regions": 0, "raw": ""})
for fn in data.get("functions", []):
    fl = [rel(x) for x in fn.get("filenames", [])]
    if not fl or not keep(fl[0]): continue
    p, dn = fl[0], demangle(fn["name"])
    if "tests" in dn.split("::"): continue          # unit-test helpers, not product code
    e = agg[(p, dn)]
    e["count"] += fn["count"]
    e["regions"] = max(e["regions"], len(fn.get("regions", [])))

dead = collections.defaultdict(list); live = collections.Counter()
for (p, dn), v in agg.items():
    (dead[p].append((dn, v["regions"])) if v["count"] == 0 else live.update([p]))

json.dump({"files": files,
           "dead": {k: sorted(v, key=lambda t: -t[1]) for k, v in dead.items()},
           "live": dict(live)}, open(sys.argv[3], "w"), indent=1)

print(f"{'file':<44} {'reg%':>6} {'uncov':>6} {'dead':>5} {'live':>5}")
for p, s in sorted(files.items(), key=lambda kv: -kv[1]["uncovered"]):
    print(f"{p:<44} {s['region_pct']:>6.1f} {s['uncovered']:>6} {len(dead.get(p,[])):>5} {live.get(p,0):>5}")
print(f"\nTOTAL dead functions: {sum(len(v) for v in dead.values())}")
