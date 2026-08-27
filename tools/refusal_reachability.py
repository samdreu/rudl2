#!/usr/bin/env python3
"""Which refusal sites can any test or example actually reach?

For each `UnsupportedConstruct{...}` / error construction in the lowering stages,
look up the execution count of the covering region. Count 0 = a diagnostic no
input in the corpus can trigger.
"""
import json, re, sys, collections

data = json.load(open(sys.argv[1]))["data"][0]
ROOT = sys.argv[2].rstrip("/") + "/"
rel = lambda p: p[len(ROOT):] if p.startswith(ROOT) else p

# line -> best (max) count seen across every region covering it
cov = collections.defaultdict(dict)
for fn in data.get("functions", []):
    files = [rel(x) for x in fn.get("filenames", [])]
    for r in fn.get("regions", []):
        l1, _, l2, _, cnt = r[0], r[1], r[2], r[3], r[4]
        f = files[r[5]] if len(r) > 5 and r[5] < len(files) else files[0]
        d = cov[f]
        for ln in range(l1, min(l2, l1 + 60) + 1):
            d[ln] = max(d.get(ln, 0), cnt)

TARGETS = ["copper-codegen/src/chir_lower.rs", "copper-codegen/src/shir_lower.rs",
           "copper-codegen/src/vlir_lower.rs", "copper-codegen/src/parser.rs",
           "copper-codegen/src/control_extract.rs"]
PAT = re.compile(r"(UnsupportedConstruct|UnsupportedExpr|UnsupportedStmt|UnresolvableType|"
                 r"AmbiguousWidth|TickInsideBranch|RegisterWireConflict|NoTick|CrossClockTick|"
                 r"VLIRLowerError::\w+)\s*\{")

dead, live = [], 0
for f in TARGETS:
    try: lines = open(ROOT + f).read().splitlines()
    except OSError: continue
    for i, ln in enumerate(lines, 1):
        if not PAT.search(ln) or "pub enum" in ln: continue
        # A `Display` impl matches the same variant names; only a site that actually
        # CONSTRUCTS the error counts as a refusal.
        if "Err(" not in " ".join(lines[max(0, i-3):i+1]): continue
        c = cov[f].get(i)
        if c is None: continue
        if c == 0:
            ctx = " ".join(lines[i-1:i+3])[:150]
            msg = re.search(r'"([^"]{12,110})', ctx)
            dead.append((f, i, msg.group(1) if msg else ctx[:80]))
        else: live += 1

print(f"reachable refusal sites: {live}")
print(f"UNREACHABLE refusal sites: {len(dead)}\n")
for f, i, m in dead:
    print(f"  {f}:{i}\n      {m}")
