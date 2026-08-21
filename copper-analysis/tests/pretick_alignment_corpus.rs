//! Corpus regression for the **pre-tick alignment hazard** detector
//! (`copper_analysis::unprotected_pretick_out_write`;
//! `design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md`).
//!
//! This is gate **G2** of that plan, and the point is the second half: the rule must
//! catch every module measured to diverge **without flagging a single one that
//! doesn't**. A prior candidate rule matched all seven synthetic variants and still
//! rejected `mac_fsm` — the project's G2 name-exact register reference — which is why
//! the corpus verdict is pinned here as a test rather than checked by eye.
//!
//! The expectation is an **exact set**, so this fails in both directions: a newly
//! flagged module is a regression (or a real bug in that module), and a
//! no-longer-flagged one means the hazard was fixed and the list needs updating.
//!
//! Scope note: the detector examines only the pre-tick segment, so the multi-tick
//! `accum_2` class is a known false negative (plan Q5) and is deliberately absent.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use syn::{Item, ItemFn};

/// Repo root, from this crate's manifest dir (`<root>/copper-analysis`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target" || n == "obj_dir") {
                continue;
            }
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Every `fn` carrying `#[hardware(...)]` in a clocked mode, including inside `mod`s.
/// `combinational` has no tick and therefore no pre-tick segment.
fn clocked_hardware_fns(items: &[Item], out: &mut Vec<ItemFn>) {
    for item in items {
        match item {
            Item::Fn(f) => {
                let clocked = f.attrs.iter().any(|a| {
                    a.path().segments.last().is_some_and(|s| s.ident == "hardware")
                        && a.parse_args::<syn::Ident>()
                            .is_ok_and(|id| id == "sequential" || id == "synchronizer")
                });
                if clocked {
                    out.push(f.clone());
                }
            }
            Item::Mod(m) => {
                if let Some((_, items)) = &m.content {
                    clocked_hardware_fns(items, out);
                }
            }
            _ => {}
        }
    }
}

/// The modules measured to diverge, as `file::module`. Each is a plain-`Out` design
/// whose pre-tick segment assigns a register with no preceding `In` read.
///
/// * `fast_counter` — measured sim-vs-SV divergence, adjudicated against an
///   independent hand-written reference; three copies (two examples + one test).
/// * `add_then_write` / `fast_counter` in `sequential_forwarding_divergence.rs` — the
///   pinned fixtures for the divergence itself.
/// * `probe_fsm` — a pre-existing `#[ignore]`d divergence, measured to be the *same*
///   defect (a leading read on every path fixes it).
/// * `ram_prewrite` — flagged and plausible, but its only test (`probe_mem_latency`)
///   is `#[ignore]`d, so it has no behavioral verdict yet. Plan phase 0b.
const EXPECTED_FLAGGED: &[&str] = &[
    "examples/cdc/two_domain_counter.rs::fast_counter",
    "examples/cdc/two_domain_hierarchy.rs::fast_counter",
    "tests/fixtures/probe_timing_dut.rs::probe_fsm",
    "tests/mem_latency_probe.rs::ram_prewrite",
    "tests/sequential_forwarding_divergence.rs::add_then_write",
    "tests/sequential_forwarding_divergence.rs::fast_counter",
    "tests/two_domain_hierarchy_cdc.rs::fast_counter",
];

#[test]
fn pretick_alignment_hazard_flags_exactly_the_known_divergent_modules() {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in ["examples", "src", "tests"] {
        rs_files(&root.join(dir), &mut files);
    }
    files.sort();

    let mut scanned = 0usize;
    let mut flagged: BTreeSet<String> = BTreeSet::new();
    for path in &files {
        let Ok(src) = fs::read_to_string(path) else { continue };
        let Ok(file) = syn::parse_file(&src) else { continue };
        let mut fns = Vec::new();
        clocked_hardware_fns(&file.items, &mut fns);
        for f in &fns {
            scanned += 1;
            let ports = copper_analysis::unprotected_pretick_out_write(f);
            if !ports.is_empty() {
                let rel = path.strip_prefix(&root).unwrap_or(path).display();
                flagged.insert(format!("{rel}::{}", f.sig.ident));
            }
        }
    }

    assert!(scanned >= 40, "expected to scan the corpus, only saw {scanned} clocked modules");

    let expected: BTreeSet<String> = EXPECTED_FLAGGED.iter().map(|s| s.to_string()).collect();
    let unexpected: Vec<_> = flagged.difference(&expected).cloned().collect();
    let missing: Vec<_> = expected.difference(&flagged).cloned().collect();

    assert!(
        unexpected.is_empty(),
        "NEW module(s) flagged by the pre-tick alignment rule: {unexpected:#?}\n\
         Either that module has the divergence (check it against its transpiled SV), \
         or the rule has regressed into a false positive. Do NOT silence this by \
         adding to EXPECTED_FLAGGED without a measured verdict — a rule that rejects \
         a correct design is the failure mode this test exists to prevent."
    );
    assert!(
        missing.is_empty(),
        "module(s) no longer flagged: {missing:#?}\n\
         If the underlying divergence was fixed, drop them from EXPECTED_FLAGGED and \
         re-check design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md — several tests are \
         pinned to the current (divergent) behaviour and will need re-blessing."
    );

    eprintln!(
        "pre-tick alignment: {scanned} clocked modules scanned, {} flagged (all measured-divergent)",
        flagged.len()
    );
}
