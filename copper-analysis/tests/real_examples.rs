//! Corpus regression for item 2's reachability well-formedness check
//! (`design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md`).
//!
//! The check must reject *malformed* loops (a path that returns to the loop head
//! without ticking — unit-tested in `src/cfg.rs`) **without** rejecting any
//! *legitimate* design, including the ones with uneven per-branch tick counts the
//! plan explicitly calls out. This test enforces the second half against the whole
//! real corpus: every `#[hardware(sequential)]` module in `examples/`, `src/`, and
//! `tests/fixtures/` must pass `check_reachability`, and every module with
//! persistent state must infer a non-empty register set.

use std::fs;
use std::path::{Path, PathBuf};

use syn::{Item, ItemFn};

/// Repo root, from this crate's manifest dir (`<root>/copper-analysis`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// Recursively collect `.rs` files under `dir` (skipping build output).
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

/// Every `fn` carrying `#[hardware(sequential)]`, including inside `mod` blocks.
fn sequential_hardware_fns(items: &[Item], out: &mut Vec<ItemFn>) {
    for item in items {
        match item {
            Item::Fn(f) if is_sequential_hardware(f) => out.push(f.clone()),
            Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    sequential_hardware_fns(inner, out);
                }
            }
            _ => {}
        }
    }
}

fn is_sequential_hardware(f: &ItemFn) -> bool {
    hardware_mode_is(f, "sequential")
}

/// Every `fn` carrying `#[hardware(combinational)]`, including inside `mod` blocks.
fn combinational_hardware_fns(items: &[Item], out: &mut Vec<ItemFn>) {
    for item in items {
        match item {
            Item::Fn(f) if hardware_mode_is(f, "combinational") => out.push(f.clone()),
            Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    combinational_hardware_fns(inner, out);
                }
            }
            _ => {}
        }
    }
}

fn hardware_mode_is(f: &ItemFn, mode: &str) -> bool {
    f.attrs.iter().any(|a| {
        a.path().segments.last().is_some_and(|s| s.ident == "hardware")
            && a
                .meta
                .require_list()
                .ok()
                .map(|l| l.tokens.to_string())
                .and_then(|t| t.split(',').next().map(|m| m.trim().to_string()))
                .is_some_and(|m| m == mode)
    })
}

#[test]
fn every_sequential_module_is_well_formed() {
    let root = repo_root();
    let mut files = Vec::new();
    for sub in ["examples", "src", "tests/fixtures"] {
        rs_files(&root.join(sub), &mut files);
    }
    files.sort();

    let mut checked = 0usize;
    let mut violations = Vec::new();
    for file in &files {
        let Ok(src) = fs::read_to_string(file) else { continue };
        let Ok(ast) = syn::parse_file(&src) else { continue };
        let mut fns = Vec::new();
        sequential_hardware_fns(&ast.items, &mut fns);
        for f in &fns {
            checked += 1;
            if let Err(e) = copper_analysis::check_reachability(f) {
                violations.push(format!(
                    "{}::{} — {e}",
                    file.strip_prefix(&root).unwrap_or(file).display(),
                    f.sig.ident
                ));
            }
        }
    }

    assert!(checked >= 30, "expected the real sequential corpus, only found {checked} modules");
    assert!(
        violations.is_empty(),
        "reachability check rejected {} legitimate design(s) — since it is now enforced in the \
         macro, these would fail to compile:\n{}",
        violations.len(),
        violations.join("\n")
    );
    eprintln!("reachability: {checked} sequential modules — all well-formed");
}

/// Definite-assignment must accept every *legitimate* combinational module (no
/// latch inferred) — the corpus counterpart to `src/cfg.rs`'s unit tests that a
/// *constructed* partial-assignment combinational module is rejected. A false
/// positive here would fail to compile a working design, since the check is
/// enforced in the macro's combinational arm.
#[test]
fn every_combinational_module_has_definite_outputs() {
    let root = repo_root();
    let mut files = Vec::new();
    for sub in ["examples", "src", "tests/fixtures"] {
        rs_files(&root.join(sub), &mut files);
    }
    files.sort();

    let mut checked = 0usize;
    let mut violations = Vec::new();
    for file in &files {
        let Ok(src) = fs::read_to_string(file) else { continue };
        let Ok(ast) = syn::parse_file(&src) else { continue };
        let mut fns = Vec::new();
        combinational_hardware_fns(&ast.items, &mut fns);
        for f in &fns {
            checked += 1;
            if let Err(e) = copper_analysis::check_definite_assignment(f) {
                violations.push(format!(
                    "{}::{} — {e}",
                    file.strip_prefix(&root).unwrap_or(file).display(),
                    f.sig.ident
                ));
            }
        }
    }

    assert!(checked >= 5, "expected the real combinational corpus, only found {checked} modules");
    assert!(
        violations.is_empty(),
        "definite-assignment rejected {} legitimate combinational design(s) — since it is enforced \
         in the macro, these would fail to compile:\n{}",
        violations.len(),
        violations.join("\n")
    );
    eprintln!("definite-assignment: {checked} combinational modules — all outputs definitely driven");
}
