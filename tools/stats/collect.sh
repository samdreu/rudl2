#!/usr/bin/env bash
# Regenerate every evaluation number for the paper. See tools/stats/README.md.
set -uo pipefail
unset VERILATOR_ROOT
cd "$(git rev-parse --show-toplevel 2>/dev/null)" 2>/dev/null || cd "$(dirname "$0")/../.." || exit 2
STAMP=$(date +%Y-%m-%d)
mkdir -p paper/stats
fail=0
for step in "M4/M5 evidence:equivalence.py" "M1 size:loc.py" "M2 area:qor.py" "M6 perf:perf.py" "M7 sim throughput:simperf.py" "M8 attribute cost:analysis.py"; do
  echo "── ${step%%:*} ─────────────────────────────────────────"
  python3 "tools/stats/${step##*:}" || { echo "  (collector failed)"; fail=1; }
  echo
done
python3 tools/stats/summarize.py "$STAMP"
[ $fail -eq 0 ] && echo "STATS OK" || echo "STATS PARTIAL — a collector failed above"
