//! The admissible grammar must never reject a module the transpiler lowers.
//!
//! `copper_analysis::check_admissible` is a POSITIVE grammar replacing a language
//! defined by subtraction — 108 refusal sites across four stages, 76 of them sharing
//! one error variant, 60 never fired. Building it incrementally is only safe with a
//! standing check that it has not overreached, because the failure mode is silent
//! until someone's working design stops compiling.
//!
//! The criterion is deliberately ASYMMETRIC:
//!
//! ```text
//!   admissible(m) == Err  ⟹  transpile(m) == Err        ENFORCED
//!   transpile(m) == Err   ⟹  admissible(m) == Err        the GOAL, reported not enforced
//! ```
//!
//! The first direction is the safety property: the grammar may only reject designs
//! that already fail. The second is the progress measure — every module that moves
//! into it is a late refusal that can become an early one, and eventually a deleted
//! downstream arm. The test prints that count so the direction of travel is visible
//! rather than remembered.
//!
//! Ground truth comes from running the transpiler, not from a list. A list would go
//! stale the first time a lowering gap was closed.

use std::path::{Path, PathBuf};

use syn::ItemFn;

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn grammar_never_rejects_what_the_transpiler_lowers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("workspace root");
    let mut files = Vec::new();
    for d in ["examples", "tests/fixtures", "src"] {
        collect(&root.join(d), &mut files);
    }
    files.sort();

    let mut overreach: Vec<String> = Vec::new();
    let (mut checked, mut caught_early, mut still_late) = (0usize, 0usize, 0usize);

    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        let Ok(file) = syn::parse_file(&src) else { continue };
        let hw: Vec<&ItemFn> = file
            .items
            .iter()
            .filter_map(|it| match it {
                syn::Item::Fn(f) if copper_codegen::is_hardware_fn(f) => Some(f),
                _ => None,
            })
            .collect();
        let multi = hw.len() > 1;

        for f in hw {
            let name = f.sig.ident.to_string();
            let module = if multi { Some(name.as_str()) } else { None };
            let where_ = format!(
                "{}::{name}",
                path.strip_prefix(root).unwrap_or(path).display()
            );

            let admissible = copper_analysis::check_admissible(f);
            let transpiles = copper_codegen::transpile_source(
                &src,
                module,
                &copper_codegen::EmitConfig::default(),
            )
            .is_ok();
            checked += 1;

            match (admissible.is_err(), transpiles) {
                // THE SAFETY PROPERTY. A module the transpiler lowers must not be
                // rejected by the grammar — that is a working design broken to tidy
                // up an error message.
                (true, true) => overreach.push(format!(
                    "{where_}: grammar REJECTS a module that transpiles — {}",
                    admissible.unwrap_err()
                )),
                (true, false) => caught_early += 1,
                (false, false) => still_late += 1,
                (false, true) => {}
            }
        }
    }

    println!(
        "admissible grammar: {checked} modules; {caught_early} refusals now caught EARLY, \
         {still_late} still refused only by the transpiler"
    );

    assert!(
        overreach.is_empty(),
        "the admissible grammar rejects {} module(s) that transpile:\n{}",
        overreach.len(),
        overreach.join("\n")
    );
    // The corpus must actually contain both kinds, or this test is vacuous — a
    // grammar that accepts everything would otherwise pass forever.
    assert!(
        checked > 100,
        "expected the whole corpus, only scanned {checked} modules"
    );
}
