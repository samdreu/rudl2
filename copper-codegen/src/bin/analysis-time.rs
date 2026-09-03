//! M8 — the cost of the `#[hardware]` attribute: wall-clock per module for the
//! analysis the macro runs, measured outside the compiler.
//!
//! The attribute's work is (1) parsing its input tokens into a `syn::ItemFn`,
//! (2) the shared control-flow analysis and the compile-time rules of
//! `copper-analysis`, and (3) a token rewrite of the input reads and the module
//! wrapper. (1) and (2) are what this times, by reproducing the macro's own call
//! sequence on the same function it would see; (3) is a single walk over the
//! syntax tree and is not separately measurable outside `rustc`. What is NOT
//! measured, and must not be reported as this number, is `rustc`'s own cost of
//! compiling the generated coroutine — that is ordinary compilation of the
//! design, not overhead the attribute adds.
//!
//! ```text
//! cargo run -q --release -p copper-codegen --bin analysis-time -- [--runs N]
//! ```
//!
//! One CSV row per `#[hardware]` module under `examples/`, on stdout:
//! `module,file,mode,median_us,min_us`. `tools/stats/analysis.py` drives it.

use std::path::{Path, PathBuf};
use std::time::Instant;

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "old") {
                continue;
            }
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The mode ident, read from the first token of the attribute list (a flag such
/// as `allow_pretick_alignment` would defeat `parse_args::<Ident>`).
fn attr_mode(f: &syn::ItemFn) -> String {
    for a in &f.attrs {
        if !a.path().segments.last().is_some_and(|s| s.ident == "hardware") {
            continue;
        }
        let toks = match &a.meta {
            syn::Meta::List(l) => l.tokens.to_string(),
            _ => String::new(),
        };
        return toks
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .find(|s| !s.is_empty())
            .unwrap_or("sequential")
            .to_string();
    }
    "?".to_string()
}

/// One run of what the attribute does before it rewrites anything: parse the
/// function from its tokens, then the macro's checks in the macro's order.
/// Results are discarded; every module here compiles, so the checks pass.
fn one_run(tokens: &str, mode: &str) {
    let f: syn::ItemFn = syn::parse_str(tokens).expect("re-parse");
    let _ = copper_analysis::check_admissible(&f);
    match mode {
        "combinational" => {
            let _ = copper_analysis::check_definite_assignment(&f);
        }
        "structural" => {}
        _ => {
            // The macro's sequence since 2026-09-03: one graph, every check a
            // method on it, the register set cached on the graph.
            let cfg = copper_analysis::Cfg::build(&f);
            if let Some(c) = cfg.as_ref() {
                let _ = c.check_reachability();
                let _ = c.multi_write_collapse();
                let _ = c.unprotected_pretick_out_write();
                let _ = c.unprotected_trailing_out_write();
                let _ = c.pretick_out_write_before_update();
                let _ = c.multi_phase_out_write();
                let _ = c.registers();
            }
            let _ = copper_analysis::classify_reads_with(&f, cfg.as_ref());
        }
    }
}

/// `--breakdown <module>`: time each check of `one_run` separately, once, for
/// one module, so a slow module can be attributed to the rule that costs it.
fn breakdown(tokens: &str, mode: &str) {
    let t0 = Instant::now();
    let f: syn::ItemFn = syn::parse_str(tokens).expect("re-parse");
    println!("parse: {:.1} ms", t0.elapsed().as_secs_f64() * 1e3);
    macro_rules! timed {
        ($name:literal, $e:expr) => {{
            let t0 = Instant::now();
            let _ = $e;
            println!("{}: {:.1} ms", $name, t0.elapsed().as_secs_f64() * 1e3);
        }};
    }
    timed!("check_admissible", copper_analysis::check_admissible(&f));
    if mode == "combinational" {
        timed!("check_definite_assignment", copper_analysis::check_definite_assignment(&f));
        return;
    }
    timed!("check_reachability", copper_analysis::check_reachability(&f));
    timed!("multi_write_collapse", copper_analysis::multi_write_collapse(&f));
    timed!("unprotected_pretick_out_write", copper_analysis::unprotected_pretick_out_write(&f));
    timed!("unprotected_trailing_out_write", copper_analysis::unprotected_trailing_out_write(&f));
    timed!("pretick_out_write_before_update", copper_analysis::pretick_out_write_before_update(&f));
    timed!("multi_phase_out_write", copper_analysis::multi_phase_out_write(&f));
    timed!("infer_registers", copper_analysis::infer_registers(&f));
    timed!("classify_reads", copper_analysis::classify_reads(&f));
    // The same work with one graph: how much of the above is rebuilding it.
    let t0 = Instant::now();
    let cfg = copper_analysis::Cfg::build(&f);
    println!("Cfg::build alone: {:.1} ms", t0.elapsed().as_secs_f64() * 1e3);
    if let Some(cfg) = cfg {
        timed!("  registers() on the built graph", cfg.registers());
        timed!("  registers() again", cfg.registers());
        timed!("  unprotected_pretick_out_write on the built graph", cfg.unprotected_pretick_out_write());
        timed!("  pretick_out_write_before_update on the built graph", cfg.pretick_out_write_before_update());
        timed!("  check_reachability on the built graph", cfg.check_reachability());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let only: Option<String> = args.iter().position(|a| a == "--breakdown").map(|i| args[i + 1].clone());
    let runs: usize = std::env::args()
        .skip_while(|a| a != "--runs")
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(20);
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("workspace root");
    let mut files = Vec::new();
    collect(&root.join("examples"), &mut files);
    files.sort();

    if only.is_none() {
        println!("module,file,mode,median_us,min_us");
    }
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        let Ok(file) = syn::parse_file(&src) else { continue };
        for item in &file.items {
            let syn::Item::Fn(f) = item else { continue };
            if !f.attrs.iter().any(|a| a.path().segments.last().is_some_and(|s| s.ident == "hardware")) {
                continue;
            }
            let mode = attr_mode(f);
            // The tokens the macro receives: the function with its attributes.
            let tokens = quote::quote!(#f).to_string();
            if let Some(name) = &only {
                if f.sig.ident == name {
                    breakdown(&tokens, &mode);
                }
                continue;
            }
            one_run(&tokens, &mode); // warm-up
            let mut us: Vec<f64> = Vec::with_capacity(runs);
            for _ in 0..runs {
                let t0 = Instant::now();
                one_run(&tokens, &mode);
                us.push(t0.elapsed().as_secs_f64() * 1e6);
            }
            us.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = us[us.len() / 2];
            let rel = path.strip_prefix(root).unwrap_or(path).display();
            println!("{},{},{},{:.1},{:.1}", f.sig.ident, rel, mode, median, us[0]);
        }
    }
}
