//! Structural guard: the transpiler may REFUSE any module, but it may never
//! panic on one.
//!
//! ## The bug this exists for
//!
//! `examples/cpu/rv32i_cpu.rs` crashed the `copper-transpile` CLI outright:
//!
//! ```text
//! thread 'main' panicked at copper-codegen/src/control_extract.rs:95:43:
//! gate guarantees at least one tick
//! ```
//!
//! The two halves of control extraction disagreed about where a tick can live.
//! `expr_contains_tick` (the gate) descends into `loop` / `while` / `for`;
//! `find_tick_in_expr` (the flattener's search) does not. A module whose tick sat
//! inside a nested `loop` — the "wait until ready" idiom the CPU uses throughout —
//! passed the gate and then hit an `.expect`. No span, no construct named, no way
//! for a user to act on it.
//!
//! ## Why a corpus sweep rather than one regression test
//!
//! A panic is a *class* of failure, not one construct. Pinning only the shape that
//! happened to be found leaves the next disagreement to be discovered the same way
//! — by someone running the CLI. This sweeps every `#[hardware]` module in
//! `examples/`, which is where the awkward real designs live, and asserts each one
//! either transpiles or returns a clean `Err`.
//!
//! An `Err` is a pass. This test is not about coverage — `TODO` TRANSPILATION
//! tracks which modules lower — it is about the failure *mode*.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use syn::ItemFn;

fn hardware_fn_names(src: &str) -> Vec<String> {
    let Ok(file) = syn::parse_file(src) else { return Vec::new() };
    file.items
        .iter()
        .filter_map(|i| match i {
            syn::Item::Fn(f) if is_hardware(f) => Some(f.sig.ident.to_string()),
            _ => None,
        })
        .collect()
}

fn is_hardware(f: &ItemFn) -> bool {
    f.attrs
        .iter()
        .any(|a| a.path().segments.last().is_some_and(|s| s.ident == "hardware"))
}

/// Every `.rs` under `examples/`, recursively.
fn example_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            example_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn transpiling_any_example_module_never_panics() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    example_files(&root.join("examples"), &mut files);
    files.sort();
    assert!(
        files.len() >= 20,
        "expected to sweep the example corpus, found only {} file(s)",
        files.len()
    );

    let mut checked = 0usize;
    let mut panicked = Vec::new();

    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        for name in hardware_fn_names(&src) {
            checked += 1;
            let result = catch_unwind(AssertUnwindSafe(|| {
                copper_codegen::transpile_source(
                    &src,
                    Some(&name),
                    &copper_codegen::EmitConfig::default(),
                )
                .map(|_| ())
                .map_err(|e| e.to_string())
            }));
            // Ok(Ok(_)) transpiled, Ok(Err(_)) refused cleanly — both fine.
            if result.is_err() {
                panicked.push(format!(
                    "{}::{name}",
                    path.strip_prefix(root).unwrap_or(path).display()
                ));
            }
        }
    }

    assert!(
        checked >= 30,
        "expected to reach the example modules, only tried {checked}"
    );
    assert!(
        panicked.is_empty(),
        "the transpiler PANICKED on {} module(s) — it may refuse a module, but never crash \
         on one (see this file's header):\n  {}",
        panicked.len(),
        panicked.join("\n  ")
    );
}
