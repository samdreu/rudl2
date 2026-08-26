//! Corpus differential sweep, **phase 2** — one generated case per `#[hardware]`
//! module in `tests/fixtures/`.
//!
//! The cases are written by `build.rs`, not by hand: simulator vs Verilated
//! emitted-SystemVerilog needs no reference model, so there is nothing per-module
//! for a person to supply, and coverage stops depending on who got round to it.
//! `tests/corpus_equivalence.rs` (phase 1) is the hand-written original of exactly
//! this wiring, kept for the `examples/` modules the generator does not yet reach.
//!
//! A module the sweep cannot run still gets a test, `#[ignore]`d with the reason —
//! from `build.rs`'s `SKIP` table, or because it is generic. `tools/regression.sh`
//! prints every ignored test on every run, so a skip stays visible instead of
//! becoming a silent hole.
//!
//! **Reading a failure:** `verilator: FAIL` is a transpiler bug — the simulator is
//! the semantic source of truth. The trace comparison cannot fail here (with no
//! model, the simulator's own outputs are the expected trace), so any failure is
//! either a real divergence, invalid emitted SystemVerilog, or a generator bug that
//! shows up as a C++ compile error.

mod common;

include!(concat!(env!("OUT_DIR"), "/corpus_generated.rs"));

/// **The guard.** Every `#[hardware]` module in `tests/fixtures/` must have a case —
/// running, or `#[ignore]`d with a reason. Asserted against a fresh scan rather than
/// against the generator's own bookkeeping, so a `build.rs` that silently stops
/// covering something (a parse it declines, a filter that grows too wide) fails here
/// instead of quietly shrinking the sweep.
///
/// Same idea as `tools/regression.sh`'s G-A/G-B/G-C: in this repo "the check did not
/// run" has been a more expensive bug than "the check failed".
#[test]
fn every_fixture_module_has_a_generated_case() {
    use std::collections::BTreeSet;

    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let mut found: BTreeSet<String> = BTreeSet::new();
    for entry in std::fs::read_dir(dir).expect("tests/fixtures is readable") {
        let path = entry.expect("readable entry").path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("fixture is readable");
        let file = syn::parse_file(&src).unwrap_or_else(|e| {
            panic!("{} does not parse standalone: {e}", path.display())
        });
        for item in &file.items {
            if let syn::Item::Fn(f) = item {
                if f.attrs.iter().any(|a| {
                    a.path().segments.last().is_some_and(|s| s.ident == "hardware")
                }) {
                    found.insert(f.sig.ident.to_string());
                }
            }
        }
    }

    let covered: BTreeSet<String> = COVERED.iter().map(|s| s.to_string()).collect();
    let missing: Vec<&String> = found.difference(&covered).collect();
    let extra: Vec<&String> = covered.difference(&found).collect();
    assert!(
        missing.is_empty(),
        "fixture modules with no generated case — the sweep silently stopped covering \
         them: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "generated cases for modules that no longer exist (stale OUT_DIR?): {extra:?}"
    );
}
