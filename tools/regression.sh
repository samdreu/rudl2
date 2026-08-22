#!/usr/bin/env bash
# tools/regression.sh — the Copper regression runner.
#
# Copper is a Rust-embedded HDL that (a) simulates hardware in Rust and
# (b) transpiles `#[hardware]` modules to SystemVerilog, then proves the two agree by
# compiling the SV with Verilator and comparing cycle-by-cycle. "Does it work?" means
# that agreement holds — so this script drives all four surfaces that can break it:
#
#   1. cargo build --workspace     the crates compile
#   2. copper-transpile <file>     the standalone SV emitter / CLI
#   3. cargo test --workspace      every test target
#   4. cargo run --example <name>  Copper-sim vs Verilator equivalence
#
# THE DEFAULT IS THE FULL REGRESSION — bare, it runs everything. That is deliberate:
# an example's `main()` is a real self-check (several assert against independent
# BaseJump Verilog and `exit(1)` on mismatch), and `cargo test` only *builds*
# examples, it never runs them. A driver that quietly covered a subset would be the
# same failure mode this project keeps hitting.
#
# Usage:
#   tools/regression.sh                 FULL regression (default)
#   tools/regression.sh --quick         fast inner loop: build + CLI + a few examples
#   tools/regression.sh --no-examples   build + CLI + tests (no Verilator needed)
#   tools/regression.sh --no-test       skip `cargo test --workspace`
#   tools/regression.sh --example NAME  run only the named example(s)
#
# Anything less than the full run prints "PARTIAL", so a subset cannot be mistaken
# for a clean regression. Exits non-zero on the first failure.
#
# ── Why the guards exist ─────────────────────────────────────────────────────
# Repeatedly, the bug here has been *a check that silently did not run*: examples
# never executed, Verilator failures swallowed as "not installed", an `#[ignore]`
# whose stated reason had stopped being true, a test file with no binary. Each looked
# green. So this script does not just run things — it asserts that it ran them:
#
#   G-A  every examples/**.rs is registered as a [[example]] in Cargo.toml
#   G-B  every registered example actually ran
#   G-C  every tests/*.rs (root and per-crate) produced a test binary that ran
#
# and it prints the `#[ignore]`d tests every run, because a skipped check that prints
# nothing is indistinguishable from a passing one.
set -uo pipefail

# A stale VERILATOR_ROOT makes `verilator` refuse to run. The Rust harness clears it
# internally (see copper-sim `verilator_status`), but this script also probes the
# binary directly, so clear it here too.
unset VERILATOR_ROOT

cd "$(git rev-parse --show-toplevel 2>/dev/null || echo .)" || exit 2

# Representative fast set for --quick: combinational, generic monomorphization,
# sequential/FSM, and memory.
REP_EXAMPLES=(one_bit_comparator mux shift_register traffic_light_fsm dual_port_ram)

QUICK=0
RUN_TEST=1
NO_EXAMPLES=0
CUSTOM_EXAMPLES=()

while [ $# -gt 0 ]; do
  case "$1" in
    --quick)        QUICK=1; RUN_TEST=0 ;;
    --no-test)      RUN_TEST=0 ;;
    --no-examples)  NO_EXAMPLES=1 ;;
    --example)      shift; [ $# -gt 0 ] || { echo "--example needs a name" >&2; exit 2; }
                    CUSTOM_EXAMPLES+=("$1"); RUN_TEST=0 ;;
    -h|--help)      sed -n '2,32p' "$0"; exit 0 ;;
    *)              echo "unknown arg: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

PARTIAL=0
[ "$QUICK" -eq 1 ] && PARTIAL=1
[ "$NO_EXAMPLES" -eq 1 ] && PARTIAL=1
[ "$RUN_TEST" -eq 0 ] && PARTIAL=1
[ "${#CUSTOM_EXAMPLES[@]}" -gt 0 ] && PARTIAL=1

fail() { echo; echo "REGRESSION FAIL: $*" >&2; exit 1; }
step() { echo; echo "==== $* ===="; }

START=$(date +%s)
elapsed() { echo "$(( $(date +%s) - START ))s"; }

TMP="$(mktemp -d)"
FAILURE_LOG="./regression-failure.log"
# Keep the log when something failed. A regression that deletes its own evidence
# forces you to reproduce before you can even read it — and an intermittent failure
# may not reproduce. (This is how the det_010 work-dir race was finally diagnosed.)
KEEP_LOG=0
cleanup() {
  if [ "$KEEP_LOG" -eq 1 ] && [ -f "$TMP/test.log" ]; then
    cp -f "$TMP/test.log" "$FAILURE_LOG" 2>/dev/null &&
      echo "failing log preserved at $FAILURE_LOG" >&2
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

# Every [[example]] name in Cargo.toml, in order.
# (macOS ships bash 3.2 — no `mapfile`, so read line by line.)
registered_examples() {
  grep -A1 '^\[\[example\]\]' Cargo.toml | grep 'name =' | sed 's/.*"\(.*\)".*/\1/'
}

# ── 1. build ─────────────────────────────────────────────────────────────────
step "1/4  build workspace"
cargo build --workspace 2>&1 | tail -3
[ "${PIPESTATUS[0]}" -eq 0 ] || fail "cargo build --workspace"

# ── 2. the standalone CLI ────────────────────────────────────────────────────
step "2/4  copper-transpile CLI"
BIN=target/debug/copper-transpile
[ -x "$BIN" ] || cargo build -q -p copper-codegen || fail "building copper-transpile"

# Capture into variables, then grep. Piping the binary straight into `grep -q` trips
# `set -o pipefail`: grep closes the pipe on the first match, the transpiler dies with
# SIGPIPE, and pipefail then fails the whole pipeline.
DUT=examples/combinational/one_bit_comparator.rs
LIST=$("$BIN" "$DUT" --list) || fail "--list exited non-zero"
echo "$LIST" | grep -q one_bit_comparator || fail "--list did not report the module"

SV=$("$BIN" "$DUT") || fail "transpile exited non-zero"
echo "$SV" | grep -q "^module one_bit_comparator" || fail "emitted SV missing the module"
echo "$SV" | grep -q "^endmodule" || fail "emitted SV missing 'endmodule'"
echo "  transpiled $(echo "$SV" | wc -l | tr -d ' ') lines of SystemVerilog — OK"

# ── 3. every test target ─────────────────────────────────────────────────────
if [ "$RUN_TEST" -eq 1 ]; then
  step "3/4  cargo test --workspace"
  cargo test --workspace > "$TMP/test.log" 2>&1
  TEST_RC=$?

  grep -E "^test result:" "$TMP/test.log" | awk '
    {for (i = 1; i <= NF; i++) {
       if ($i == "passed;")  p += $(i-1)
       if ($i == "failed;")  f += $(i-1)
       if ($i == "ignored;") g += $(i-1) }}
    END { printf "  %d passed, %d failed, %d ignored across %d test binaries\n", p, f, g, NR }'

  if [ "$TEST_RC" -ne 0 ]; then
    KEEP_LOG=1
    grep -E "^(test .* FAILED|error(\[|:))" "$TMP/test.log" | head -20
    fail "cargo test --workspace"
  fi

  # G-C — no orphaned test file. A tests/*.rs that never produced a binary is a
  # check sitting on disk doing nothing.
  MISSING=""
  for f in tests/*.rs */tests/*.rs; do
    [ -e "$f" ] || continue
    stem="$(basename "$f" .rs)"
    grep -q "Running tests/${stem}\.rs" "$TMP/test.log" || MISSING="$MISSING $f"
  done
  [ -z "$MISSING" ] || { echo "  never ran:$MISSING" >&2; fail "G-C: orphaned test file(s)"; }
  echo "  G-C ok: all $(grep -c 'Running tests/' "$TMP/test.log") test files ran"

  # Ignored tests stay VISIBLE. These are deliberate, but an ignore whose stated
  # reason has gone stale is invisible otherwise — exactly what happened to accum_2,
  # which sat disabled long after the divergence it described had been fixed.
  NIGN=$(grep -c "\.\.\. ignored" "$TMP/test.log")
  if [ "$NIGN" -gt 0 ]; then
    echo "  $NIGN ignored test(s) — deliberately not run, re-read these periodically:"
    grep "\.\.\. ignored" "$TMP/test.log" | sed 's/^test /    /' | cut -c1-140
  fi
else
  echo; echo "  (cargo test skipped)"
fi

# ── 4. examples: sim vs Verilator ────────────────────────────────────────────
if [ "$NO_EXAMPLES" -eq 1 ]; then
  echo; echo "  (examples skipped)"
else
  step "4/4  examples (Copper sim vs Verilator equivalence)"

  if [ "${#CUSTOM_EXAMPLES[@]}" -gt 0 ]; then
    EXAMPLES=("${CUSTOM_EXAMPLES[@]}")
  elif [ "$QUICK" -eq 1 ]; then
    EXAMPLES=("${REP_EXAMPLES[@]}")
    echo "  (--quick: representative subset only)"
  else
    EXAMPLES=()
    while IFS= read -r line; do EXAMPLES+=("$line"); done < <(registered_examples)

    # G-A — an examples/**.rs with no [[example]] entry can never be run by anything.
    UNREG=""
    for f in $(find examples -name '*.rs' | sort); do
      grep -q "path = \"$f\"" Cargo.toml || UNREG="$UNREG $f"
    done
    [ -z "$UNREG" ] || { echo "  unregistered:$UNREG" >&2; fail "G-A: example(s) that can never run"; }
    echo "  G-A ok: all $(find examples -name '*.rs' | wc -l | tr -d ' ') example files are registered"
  fi

  command -v verilator >/dev/null || fail "verilator not on PATH (brew install verilator)"

  PASS=0; TOTAL=0
  for ex in "${EXAMPLES[@]}"; do
    TOTAL=$((TOTAL + 1))
    printf '  %-22s ' "$ex"
    if cargo run -q --example "$ex" >"$TMP/ex.log" 2>&1; then
      echo "PASS"; PASS=$((PASS + 1))
    else
      echo "FAIL"
      sed 's/^/      /' "$TMP/ex.log" | tail -8
      echo "      (rerun: cargo run --example $ex)"
    fi
  done
  echo "  examples: $PASS/$TOTAL passed"
  [ "$PASS" -eq "$TOTAL" ] || fail "$((TOTAL - PASS)) example(s) failed"

  # G-B — every registered example actually ran.
  if [ "$QUICK" -eq 0 ] && [ "${#CUSTOM_EXAMPLES[@]}" -eq 0 ]; then
    NREG=$(registered_examples | wc -l | tr -d ' ')
    [ "$TOTAL" -eq "$NREG" ] || fail "G-B: $NREG registered but only $TOTAL ran"
    echo "  G-B ok: all $NREG registered examples ran"
  fi
fi

echo
if [ "$PARTIAL" -eq 1 ]; then
  echo "PARTIAL OK ($(elapsed)) — a subset only; run tools/regression.sh bare for the full regression"
else
  echo "REGRESSION OK ($(elapsed))"
fi
