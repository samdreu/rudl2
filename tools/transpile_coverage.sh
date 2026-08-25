#!/usr/bin/env bash
# Probe every `#[hardware]` module in examples/ through the copper-transpile CLI
# and report which ones the transpiler accepts.
#
# This number is quoted throughout `TODO` ("28/34"), and it was being re-measured
# by hand each time — which is how it went stale. Run this instead.
#
#   tools/transpile_coverage.sh            # summary + the failures, with causes
#   tools/transpile_coverage.sh --quiet    # just the tally
#
# NOTE it measures ACCEPTANCE, not correctness: a module can pass here and still
# fail the equivalence harness, which runs Verilator under -Wall. `TODO` records
# that distinction under "LINT-CLEAN IS NOT THE SAME AS TRANSPILES".
set -uo pipefail
cd "$(dirname "$0")/.."

quiet=0
[ "${1:-}" = "--quiet" ] && quiet=1

cargo build -q -p copper-codegen --bin copper-transpile || exit 1
BIN=target/debug/copper-transpile

ok=0; fail=0; failures=""
for f in $(find examples -name '*.rs' | sort); do
  mods=$("$BIN" "$f" --list 2>/dev/null | tail -n +2 | awk '{print $1}')
  for m in $mods; do
    if "$BIN" "$f" --module "$m" -o /dev/null >/dev/null 2>&1; then
      ok=$((ok+1))
    else
      fail=$((fail+1))
      failures+="  ${f#examples/} :: $m
      $("$BIN" "$f" --module "$m" 2>&1 | head -1 | cut -c1-160)
"
    fi
  done
done

if [ "$quiet" -eq 0 ] && [ -n "$failures" ]; then
  echo "STILL REFUSED:"
  printf '%s' "$failures"
fi
echo "transpiler coverage: $ok/$((ok+fail)) modules"
