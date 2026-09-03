#!/usr/bin/env bash
# Produce the paper's evaluation numbers under STATED conditions, twice, and
# check that the second run reproduces the first. Written for an exclusive
# cluster node; runs on a laptop too, with the caveats it records.
#
#   tools/stats/paper_numbers.sh [--runs N] [--cycles N] [--gap SECONDS]
#                                [--core K] [--max-load X] [--with-regression]
#                                [--tolerance PCT] [--force] [--quick]
#
# What it does, in order:
#   1. Refuses to start unless every tool is present (rustc, cargo, python3,
#      verilator, iverilog, yosys) and the 1-minute load average is below
#      --max-load (default: 2.0, or a sixth of the cores on a larger machine):
#      a loaded laptop produced a 2x swing in the
#      simulator-throughput numbers on 2026-09-01/02. `--force` overrides.
#   2. Records the machine and tool provenance to paper/stats/machine.txt:
#      OS, CPU model, cores, memory, frequency governor (Linux), whether the
#      collectors were pinned to one core, load at start, git commit, versions.
#   3. Optionally runs the full regression first (`--with-regression`) and stops
#      unless it prints REGRESSION OK — numbers from a red tree are not numbers.
#   4. Runs every collector (evidence, size, area, transpile time, simulation
#      throughput, attribute cost) as pass A, waits --gap seconds (default 3600),
#      runs them again as pass B, and compares the timing collectors between the
#      two passes. The final paper/stats/*.csv are pass B's; pass A is kept as
#      paper/stats/passA/. If any throughput or attribute-cost median differs by
#      more than --tolerance percent (default 10), the summary is stamped
#      NOT REPRODUCED and the script exits 3 — do not put those numbers in the
#      paper; find the load and rerun.
#   5. Pins the timing collectors to one core with `taskset -c K` where taskset
#      exists (Linux); on macOS there is no pinning and machine.txt says so.
#
# On a Slurm cluster, the intended invocation is an exclusive node:
#   sbatch --exclusive -N1 -c 4 -t 04:00:00 --wrap 'tools/stats/paper_numbers.sh --with-regression'
# and, if the site allows it, the performance governor:
#   sudo cpupower frequency-set -g performance
#
# `--quick` validates the script itself (tiny cycle counts, two runs, no gap)
# into paper/stats-quick/ and leaves paper/stats untouched.
set -uo pipefail
cd "$(git rev-parse --show-toplevel 2>/dev/null)" || cd "$(dirname "$0")/../.." || exit 2
unset VERILATOR_ROOT

RUNS=10; CYCLES=1000000; GAP=3600; CORE=""; MAX_LOAD=""; WITH_REG=0; TOL=10; FORCE=0; QUICK=0
while [ $# -gt 0 ]; do
  case "$1" in
    --runs) RUNS=$2; shift 2;;
    --cycles) CYCLES=$2; shift 2;;
    --gap) GAP=$2; shift 2;;
    --core) CORE=$2; shift 2;;
    --max-load) MAX_LOAD=$2; shift 2;;
    --tolerance) TOL=$2; shift 2;;
    --with-regression) WITH_REG=1; shift;;
    --force) FORCE=1; shift;;
    --quick) QUICK=1; RUNS=2; CYCLES=10000; GAP=0; shift;;
    *) echo "unknown argument: $1" >&2; exit 2;;
  esac
done

OUT=paper/stats
if [ $QUICK -eq 1 ]; then
  # The collectors write to paper/stats; park the real numbers and restore them after.
  BACKUP=$(mktemp -d); cp -R paper/stats/. "$BACKUP"/ 2>/dev/null
  # Restore IN PLACE: copy the parked files back over the quick outputs and
  # delete only what the quick run added. Never `rm -rf` the directory — on an
  # NFS home a file still held open leaves a placeholder and the removal fails.
  restore_stats() {
    rm -rf paper/stats-quick; mkdir -p paper/stats-quick; cp -R paper/stats/. paper/stats-quick/ 2>/dev/null
    cp -R "$BACKUP"/. paper/stats/
    for f in paper/stats/* paper/stats/.[!.]*; do
      [ -e "$f" ] || continue
      case "$(basename "$f")" in .nfs*) continue;; esac
      [ -e "$BACKUP/$(basename "$f")" ] || rm -rf "$f"
    done
    rm -rf "$BACKUP"
    echo "quick outputs in paper/stats-quick/; paper/stats restored"
  }
  trap restore_stats EXIT
fi

say() { printf '\n── %s ──────────────────────────────────────────\n' "$*"; }

# 1. Tools and idleness.
say "preflight"
missing=""
for t in rustc cargo python3 verilator iverilog yosys; do
  command -v "$t" >/dev/null 2>&1 || missing="$missing $t"
done
if [ -n "$missing" ]; then echo "missing tools:$missing" >&2; exit 2; fi
load1=$(uptime | sed -E 's/.*load averages?: *([0-9.]+).*/\1/' | tr -d ',')
# Idle means a free core and little contention, so the limit scales with the
# machine: 2.0 on a laptop, a sixth of the cores on a big shared server (a
# 96-core box at a load of 8 has 88 idle cores; the timing collectors run on
# one, pinned). `--max-load` overrides.
ncpu=$( (nproc 2>/dev/null || sysctl -n hw.ncpu) )
[ -z "$MAX_LOAD" ] && MAX_LOAD=$(python3 -c "print(max(2.0, $ncpu / 6))")
echo "load average (1 min): $load1   limit: $MAX_LOAD   (cores: $ncpu)"
if [ $FORCE -eq 0 ] && [ "$(python3 -c "print(1 if float('$load1') > float('$MAX_LOAD') else 0)")" = "1" ]; then
  echo "machine is not idle (load $load1 > $MAX_LOAD); quit other work or pass --force" >&2; exit 2
fi

# 2. Provenance.
say "provenance"
mkdir -p "$OUT"
PIN=""
if [ -n "$CORE" ] && command -v taskset >/dev/null 2>&1; then PIN="taskset -c $CORE"; fi
if [ -z "$CORE" ] && command -v taskset >/dev/null 2>&1; then CORE=1; PIN="taskset -c $CORE"; fi
{
  echo "generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "commit: $(git rev-parse --short HEAD) $(git status --porcelain | grep -q . && echo '(dirty)' || echo '(clean)')"
  echo "host: $(hostname)"
  echo "os: $(uname -srm)"
  if [ "$(uname)" = "Darwin" ]; then
    echo "cpu: $(sysctl -n machdep.cpu.brand_string) ($(sysctl -n hw.ncpu) logical cores)"
    echo "memory: $(( $(sysctl -n hw.memsize) / 1073741824 )) GB"
    echo "governor: n/a (macOS)"
    echo "pinning: none (macOS has no taskset)"
    echo "cooling: check — a fanless laptop throttles under the sustained load of this benchmark"
  else
    echo "cpu: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | sed 's/^ //') ($(nproc) logical cores)"
    echo "memory: $(( $(grep MemTotal /proc/meminfo | awk '{print $2}') / 1048576 )) GB"
    echo "governor: $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo unknown)"
    echo "pinning: ${PIN:-none}"
    [ -n "${SLURM_JOB_ID:-}" ] && echo "slurm job: $SLURM_JOB_ID exclusive=${SLURM_JOB_EXCLUSIVE:-unknown} node=${SLURMD_NODENAME:-}"
  fi
  echo "load at start: $load1"
  echo "runs: $RUNS   cycles: $CYCLES   gap: ${GAP}s   tolerance: ${TOL}%"
  echo "rustc: $(rustc --version)"
  echo "verilator: $(verilator --version)"
  echo "iverilog: $(iverilog -V 2>&1 | head -1)"
  echo "yosys: $(yosys -V 2>&1 | head -1)"
} | tee "$OUT/machine.txt"

# 3. Optional correctness gate.
if [ $WITH_REG -eq 1 ]; then
  say "regression gate"
  tools/regression.sh > "$OUT/regression.log" 2>&1
  if ! grep -q "^REGRESSION OK" "$OUT/regression.log"; then
    echo "regression did not pass; see $OUT/regression.log" >&2; exit 1
  fi
  tail -1 "$OUT/regression.log"
fi

# 4. Two passes.
collect() {
  local label=$1; local fail=0
  say "pass $label"
  python3 tools/stats/equivalence.py || fail=1
  python3 tools/stats/loc.py || fail=1
  python3 tools/stats/qor.py || fail=1
  $PIN python3 tools/stats/perf.py --runs "$RUNS" || fail=1
  $PIN python3 tools/stats/simperf.py --runs "$RUNS" --cycles "$CYCLES" || fail=1
  $PIN python3 tools/stats/analysis.py --runs "$RUNS" || fail=1
  return $fail
}
collect A || { echo "a collector failed in pass A" >&2; exit 1; }
rm -rf "$OUT/passA"; mkdir -p "$OUT/passA"; cp "$OUT"/*.csv "$OUT/passA"/
if [ "$GAP" -gt 0 ]; then say "waiting ${GAP}s between passes"; sleep "$GAP"; fi
collect B || { echo "a collector failed in pass B" >&2; exit 1; }

# 5. Reproduction check on the timing collectors.
say "reproduction check (pass B against pass A, tolerance ${TOL}%)"
python3 - "$OUT" "$TOL" <<'PY'
import csv, sys, pathlib
out, tol = pathlib.Path(sys.argv[1]), float(sys.argv[2])
worst = 0.0; bad = []
def rows(p, key):
    return {r[key]: r for r in csv.DictReader(open(p))}
import statistics
for name, key, cols in [("simperf.csv", "design", ["sim_cycles_per_sec", "verilator_cycles_per_sec", "iverilog_cycles_per_sec"]),
                        ("analysis.csv", "module", ["median_us"])]:
    a, b = rows(out/"passA"/name, key), rows(out/name, key)
    if name == "analysis.csv":
        # Sub-millisecond modules jitter by tens of percent between any two
        # runs and the paper reports none of them individually; compare the
        # corpus median and the modules that take at least a millisecond.
        ma = statistics.median(float(r["median_us"]) for r in a.values())
        mb = statistics.median(float(r["median_us"]) for r in b.values())
        d = abs(mb - ma) / ma * 100 if ma else 0.0
        worst = max(worst, d)
        # The paper rounds this to a tenth of a millisecond; allow twice the throughput tolerance.
        if d > 2 * tol: bad.append(f"{name} corpus median: A={ma:.0f} B={mb:.0f} ({d:.1f}%)")
        b = {k: r for k, r in b.items() if float(a.get(k, r)["median_us"]) >= 1000}
    for k in b:
        if k not in a: continue
        for c in cols:
            if c not in b[k] or not b[k][c] or not a[k][c]: continue
            x, y = float(a[k][c]), float(b[k][c])
            if x == 0: continue
            d = abs(y - x) / x * 100
            worst = max(worst, d)
            if d > tol: bad.append(f"{name} {k} {c}: A={x:.0f} B={y:.0f} ({d:.1f}%)")
print(f"largest pass-to-pass difference: {worst:.1f}%")
for line in bad: print("  NOT REPRODUCED:", line)
(out/"reproduction.txt").write_text(
    f"largest pass-to-pass difference: {worst:.1f}% (tolerance {tol}%)\n" +
    ("".join("NOT REPRODUCED: "+l+"\n" for l in bad) if bad else "reproduced\n"))
sys.exit(3 if bad else 0)
PY
repro=$?
python3 tools/stats/summarize.py "$(date +%Y-%m-%d)"
if [ $repro -ne 0 ]; then
  sed -i.bak '1s/$/ — NOT REPRODUCED between two passes, see reproduction.txt/' "$OUT/SUMMARY.md" && rm -f "$OUT/SUMMARY.md.bak"
  echo "NUMBERS NOT REPRODUCED — do not use; see $OUT/reproduction.txt" >&2
  exit 3
fi
echo "PAPER NUMBERS OK — $OUT/SUMMARY.md, provenance in $OUT/machine.txt"
