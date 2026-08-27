//! Corpus-wide emitted-SystemVerilog baseline — gate A1 of
//! `design_docs/PAIRED_IMPLEMENTATION_SCOPE.md`.
//!
//! The cycle-dataflow migration's phase gates require that each codegen change
//! alters the emitted SV of **exactly the declared module set and nothing
//! else** (F1 predicts zero live modules for phase B). Observing that by eye is
//! how silent scope creep happens, so this makes it a diff:
//!
//! ```text
//! cargo run -q -p copper-codegen --bin sv-baseline -- snapshot   # before a change
//! cargo run -q -p copper-codegen --bin sv-baseline -- diff       # after it
//! ```
//!
//! `snapshot` transpiles every corpus module (same walk as `derivation-audit`:
//! `examples/`, `tests/`, `src/`, `old/` pruned) into
//! `target/sv-baseline/<module>.sv`; a module the transpiler refuses is
//! recorded as a `REFUSED: <error>` sentinel so a refusal appearing or
//! disappearing also shows up as a diff. `diff` re-transpiles and prints every
//! module whose emission changed, exiting non-zero if any did — the caller
//! checks the printed set against the phase's declared one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "old" || n == "target" || n == "obj_dir") {
                continue;
            }
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// module key -> emitted SV (or a REFUSED sentinel), over the whole corpus.
fn emit_all(root: &Path) -> BTreeMap<String, String> {
    let mut files = Vec::new();
    for d in ["examples", "tests", "src"] {
        collect(&root.join(d), &mut files);
    }
    files.sort();

    let mut out = BTreeMap::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        let Ok(file) = syn::parse_file(&src) else { continue };
        let names: Vec<String> = file
            .items
            .iter()
            .filter_map(|it| match it {
                syn::Item::Fn(f)
                    if f.attrs.iter().any(|a| {
                        a.path().segments.last().is_some_and(|s| s.ident == "hardware")
                    }) =>
                {
                    Some(f.sig.ident.to_string())
                }
                _ => None,
            })
            .collect();
        let multi = names.len() > 1;
        for name in names {
            let key = format!(
                "{}::{name}",
                path.strip_prefix(root).unwrap_or(path).display()
            )
            .replace(['/', ':'], "_");
            let sv = match copper_codegen::transpile_source(
                &src,
                if multi { Some(name.as_str()) } else { None },
                &copper_codegen::EmitConfig::default(),
            ) {
                Ok(sv) => sv,
                Err(e) => format!("REFUSED: {e}\n"),
            };
            out.insert(key, sv);
        }
    }
    out
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("workspace root");
    let base_dir = root.join("target/sv-baseline");

    match mode.as_str() {
        "snapshot" => {
            let _ = std::fs::remove_dir_all(&base_dir);
            std::fs::create_dir_all(&base_dir).unwrap();
            let all = emit_all(root);
            let n = all.len();
            for (key, sv) in all {
                std::fs::write(base_dir.join(format!("{key}.sv")), sv).unwrap();
            }
            println!("snapshot: {n} modules -> {}", base_dir.display());
        }
        "diff" => {
            assert!(
                base_dir.is_dir(),
                "no baseline at {} — run `sv-baseline snapshot` first",
                base_dir.display()
            );
            let all = emit_all(root);
            let mut changed = Vec::new();
            for (key, sv) in &all {
                let p = base_dir.join(format!("{key}.sv"));
                match std::fs::read_to_string(&p) {
                    Ok(old) if old == *sv => {}
                    Ok(_) => changed.push(format!("CHANGED {key}")),
                    Err(_) => changed.push(format!("NEW     {key}")),
                }
            }
            for entry in std::fs::read_dir(&base_dir).unwrap().flatten() {
                let stem = entry
                    .path()
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if !all.contains_key(&stem) {
                    changed.push(format!("REMOVED {stem}"));
                }
            }
            if changed.is_empty() {
                println!("sv-baseline: {} modules, no changes", all.len());
            } else {
                println!("sv-baseline: {} of {} modules differ:", changed.len(), all.len());
                for c in &changed {
                    println!("  {c}");
                }
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("usage: sv-baseline <snapshot|diff>");
            std::process::exit(2);
        }
    }
}
