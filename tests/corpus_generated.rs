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

/// **The guard.** Every `#[hardware]` module in `tests/fixtures/` and `examples/`
/// must have a case — running, or `#[ignore]`d with a reason. Asserted against a
/// fresh scan rather than against the generator's own bookkeeping, so a `build.rs`
/// that silently stops covering something (a parse it declines, a filter that grows
/// too wide, a directory it no longer walks) fails here instead of quietly shrinking
/// the sweep.
///
/// Same idea as `tools/regression.sh`'s G-A/G-B/G-C: in this repo "the check did not
/// run" has been a more expensive bug than "the check failed".
///
/// Keys are `<wrapper>::<module>`, not bare module names: two different modules
/// legitimately share a name (`fast_counter` exists in both `two_domain_counter.rs`
/// and `two_domain_hierarchy.rs`), and a set of bare names would silently merge them.
#[test]
fn every_corpus_module_has_a_generated_case() {
    use std::collections::BTreeSet;

    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                // `old/` is excluded here BECAUSE it is excluded from the sweep
                // (build.rs `collect_rs`, 2026-08-26): examples/cpu/old/ is
                // untracked scratch whose pre-subset spellings (`Vec` ports) the
                // admissible grammar rejects, so its cases could never compile.
                // The two scans must prune identically or this guard reports the
                // deliberate exclusion as silent coverage loss — which is exactly
                // what it did when only build.rs was changed, working as designed.
                if p.file_name().is_some_and(|n| n == "old") {
                    continue;
                }
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    walk(&root.join("tests/fixtures"), &mut files);
    walk(&root.join("examples"), &mut files);

    let mut found: BTreeSet<String> = BTreeSet::new();
    for path in files {
        let src = std::fs::read_to_string(&path).expect("source is readable");
        let file = syn::parse_file(&src)
            .unwrap_or_else(|e| panic!("{} does not parse standalone: {e}", path.display()));
        for item in &file.items {
            if let syn::Item::Fn(f) = item {
                if f.attrs.iter().any(|a| {
                    a.path().segments.last().is_some_and(|s| s.ident == "hardware")
                }) {
                    // Mirror build.rs's wrapper naming, so the two sets are comparable.
                    let rel = path.strip_prefix(root).unwrap_or(&path).with_extension("");
                    let mut w: String = rel
                        .to_string_lossy()
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                        .collect();
                    let is_example = path.components().any(|c| c.as_os_str() == "examples");
                    w = w
                        .trim_start_matches("tests_fixtures_")
                        .trim_start_matches("examples_")
                        .to_string();
                    let prefix = if is_example { "ex_" } else { "fx_" };
                    found.insert(format!("{prefix}{w}::{}", f.sig.ident));
                }
            }
        }
    }

    let covered: BTreeSet<String> = COVERED.iter().map(|s| s.to_string()).collect();
    let missing: Vec<&String> = found.difference(&covered).collect();
    let extra: Vec<&String> = covered.difference(&found).collect();
    assert!(
        missing.is_empty(),
        "corpus modules with no generated case — the sweep silently stopped covering \
         them: {missing:?}"
    );
    assert!(
        extra.is_empty(),
        "generated cases for modules that no longer exist (stale OUT_DIR?): {extra:?}"
    );
}
