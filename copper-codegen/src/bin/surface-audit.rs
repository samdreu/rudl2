//! What a `#[hardware]` fn may contain, versus what anything actually writes.
//!
//! The transpiler accepts a language shaped like Rust: `ExprType` in
//! `copper-core/src/frontend_ir.rs` has 33 variants, several of which
//! (`Async`, `Yield`, `Try`, `Closure`, `Macro`, …) can never be hardware. Every
//! downstream pass carries match arms for all of them, which is the structural
//! reason `copper-codegen` is 54% of the source.
//!
//! Shrinking that surface is only safe if you know what is IN USE. This walks every
//! `#[hardware]` module in the corpus and counts the syntax each one actually
//! contains, so "nothing writes this" is a measurement rather than an opinion.
//!
//! ```text
//! cargo run -q -p copper-codegen --bin surface-audit
//! ```
//!
//! It reads sources only — no transpilation, no build — so it stays honest about
//! what is WRITTEN rather than what happens to lower.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use syn::visit::Visit;

/// Every expression form the corpus could contain, named as `syn` names them so
/// the output lines up with `ExprType`'s variants one for one.
#[derive(Default)]
struct Surface {
    exprs: BTreeMap<&'static str, usize>,
    stmts: BTreeMap<&'static str, usize>,
    /// expression kind -> one module that contains it, for a place to start reading
    example: BTreeMap<&'static str, String>,
    current: String,
    /// Per module, the set of expression kinds it contains. This is what makes the
    /// cost of REMOVING a construct measurable: a module falls out of a candidate
    /// core exactly when its set is not a subset of that core.
    per_module: BTreeMap<String, std::collections::BTreeSet<&'static str>>,
}

impl Surface {
    fn note_expr(&mut self, k: &'static str) {
        *self.exprs.entry(k).or_default() += 1;
        self.example.entry(k).or_insert_with(|| self.current.clone());
        self.per_module.entry(self.current.clone()).or_default().insert(k);
    }
}

impl<'ast> Visit<'ast> for Surface {
    fn visit_expr(&mut self, e: &'ast syn::Expr) {
        let k = match e {
            syn::Expr::Array(_) => "Array",
            syn::Expr::Assign(_) => "Assign",
            syn::Expr::Async(_) => "Async",
            syn::Expr::Await(_) => "Await",
            syn::Expr::Binary(_) => "Binary",
            syn::Expr::Block(_) => "Block",
            syn::Expr::Break(_) => "Break",
            syn::Expr::Call(_) => "Call",
            syn::Expr::Cast(_) => "Cast",
            syn::Expr::Closure(_) => "Closure",
            syn::Expr::Const(_) => "Const",
            syn::Expr::Continue(_) => "Continue",
            syn::Expr::Field(_) => "Field",
            syn::Expr::ForLoop(_) => "ForLoop",
            syn::Expr::If(_) => "If",
            syn::Expr::Index(_) => "Index",
            syn::Expr::Let(_) => "Let",
            syn::Expr::Lit(_) => "Lit",
            syn::Expr::Loop(_) => "Loop",
            syn::Expr::Macro(_) => "Macro",
            syn::Expr::Match(_) => "Match",
            syn::Expr::MethodCall(_) => "MethodCall",
            syn::Expr::Paren(_) => "Paren",
            syn::Expr::Path(_) => "Path",
            syn::Expr::Range(_) => "Range",
            syn::Expr::Reference(_) => "Reference",
            syn::Expr::Repeat(_) => "Repeat",
            syn::Expr::Return(_) => "Return",
            syn::Expr::Struct(_) => "Struct",
            syn::Expr::Try(_) => "Try",
            syn::Expr::Tuple(_) => "Tuple",
            syn::Expr::Unary(_) => "Unary",
            syn::Expr::Unsafe(_) => "Unsafe",
            syn::Expr::While(_) => "While",
            syn::Expr::Yield(_) => "Yield",
            _ => "other",
        };
        self.note_expr(k);
        syn::visit::visit_expr(self, e);
    }

    fn visit_stmt(&mut self, s: &'ast syn::Stmt) {
        let k = match s {
            syn::Stmt::Local(_) => "Local",
            syn::Stmt::Item(_) => "Item",
            syn::Stmt::Expr(_, Some(_)) => "Expr(;)",
            syn::Stmt::Expr(_, None) => "Expr(tail)",
            syn::Stmt::Macro(_) => "Macro",
        };
        *self.stmts.entry(k).or_default() += 1;
        syn::visit::visit_stmt(self, s);
    }
}

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

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("workspace root");
    let mut files = Vec::new();
    for d in ["examples", "tests/fixtures", "src"] {
        collect(&root.join(d), &mut files);
    }
    files.sort();

    let mut s = Surface::default();
    let mut modules = 0usize;
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        let Ok(file) = syn::parse_file(&src) else { continue };
        for item in &file.items {
            let syn::Item::Fn(f) = item else { continue };
            if !f.attrs.iter().any(|a| {
                a.path().segments.last().is_some_and(|seg| seg.ident == "hardware")
            }) {
                continue;
            }
            modules += 1;
            s.current = format!(
                "{}::{}",
                path.strip_prefix(root).unwrap_or(path).display(),
                f.sig.ident
            );
            s.visit_block(&f.block);
        }
    }

    // `ExprType`'s variants, so "accepted but unused" is a set difference rather
    // than a judgement. Kept in the same order the enum declares them.
    const FIR_VARIANTS: &[&str] = &[
        "Array", "Assign", "Async", "Await", "Binary", "Block", "Call", "Cast", "Closure",
        "Field", "Index", "If", "Let", "Lit", "Loop", "Match", "MethodCall", "Path", "Range",
        "Reference", "Repeat", "Return", "Struct", "Tuple", "Unary", "Break", "Continue",
        "While", "Yield", "Const", "Try", "Macro", "ForLoop",
    ];

    println!("scanned {modules} #[hardware] modules in {} files\n", files.len());

    println!("USED — expression forms the corpus actually contains");
    let mut used: Vec<_> = s.exprs.iter().collect();
    used.sort_by(|a, b| b.1.cmp(a.1));
    for (k, n) in &used {
        let fir = if FIR_VARIANTS.contains(k) { "" } else { "   (not a FIR variant)" };
        println!("  {:<12} {:>5}{fir}", k, n);
    }

    println!("\nACCEPTED BUT UNUSED — a FIR variant no #[hardware] module contains");
    let mut dead = 0usize;
    for v in FIR_VARIANTS {
        if !s.exprs.contains_key(v) {
            println!("  {v}");
            dead += 1;
        }
    }
    println!("  ({dead} of {} FIR expression variants)", FIR_VARIANTS.len());

    println!("\nRARE — used by 3 or fewer sites, so cheap to remove or to pin");
    for (k, n) in s.exprs.iter().filter(|(_, n)| **n <= 3) {
        println!("  {:<12} {:>3}   first seen: {}", k, n, s.example[k]);
    }

    println!("\nSTATEMENT forms");
    for (k, n) in &s.stmts {
        println!("  {:<12} {:>5}", k, n);
    }

    // ── Ground truth for the admissibility grammar ──────────────────────────
    //
    // A positive grammar in `copper-analysis` is only correct if it accepts
    // EXACTLY what the transpiler already lowers: stricter breaks working modules,
    // looser leaves the refusal spread across chir/shir/vlir where it is today.
    // So the criterion is a set equality, and this is the side of it that has to be
    // measured rather than asserted. Re-run after every grammar change.
    // ── The cost of removing each construct ────────────────────────────────
    //
    // Shrinking the language means rejecting things that work today, so the only
    // responsible way to choose a core is to know the bill in advance. For each
    // expression form: how many modules would fall outside a core that excluded it,
    // and which ones. A form with a cost of 0 is free to drop. A form used by 80
    // modules is load-bearing whatever anyone thinks of it.
    //
    // Read it as a menu, not a verdict — the cheap rows compose, so a core is
    // assembled by dropping rows until the running total is a cost you accept.
    println!("\n── COST OF DROPPING EACH CONSTRUCT (modules that would fall out) ──");
    let mut cost: Vec<(&'static str, Vec<&String>)> = s
        .exprs
        .keys()
        .map(|k| {
            let losers: Vec<&String> = s
                .per_module
                .iter()
                .filter(|(_, set)| set.contains(k))
                .map(|(m, _)| m)
                .collect();
            (*k, losers)
        })
        .collect();
    cost.sort_by_key(|(_, l)| l.len());
    let total = s.per_module.len();
    for (k, losers) in &cost {
        let pct = (losers.len() * 100) / total.max(1);
        print!("  {:<12} {:>4} modules ({:>2}%)", k, losers.len(), pct);
        if losers.len() <= 4 {
            let short: Vec<String> = losers
                .iter()
                .map(|m| m.rsplit('/').next().unwrap_or(m).to_string())
                .collect();
            print!("   {}", short.join(", "));
        }
        println!();
    }

    // ── The curve ──────────────────────────────────────────────────────────
    //
    // Per-construct costs do NOT add up: a module lost to two different drops is
    // lost once. So the useful shape is cumulative — drop the cheapest form, then
    // the next, and watch how fast the corpus falls away. That is the trade-off
    // curve for "how small a core", and it is the thing to choose from.
    //
    // `Paren` and `Block` are grouping, not constructs; dropping them is not a
    // language decision and they are held back so the curve stays meaningful.
    println!("\n── CUMULATIVE: drop the cheapest forms first, keep the rest ──");
    let held_back = ["Paren", "Block", "Path", "Lit"];
    let mut dropped: std::collections::BTreeSet<&'static str> = Default::default();
    for (k, _) in cost.iter().filter(|(k, _)| !held_back.contains(k)) {
        dropped.insert(k);
        let survivors = s
            .per_module
            .values()
            .filter(|set| set.iter().all(|u| !dropped.contains(u)))
            .count();
        println!(
            "  drop {:<12} -> core of {:>2} forms, {:>3}/{} modules survive ({}%)",
            k,
            s.exprs.len() - dropped.len(),
            survivors,
            total,
            (survivors * 100) / total.max(1)
        );
    }

    println!("\n── TRANSPILES? (ground truth the grammar must reproduce) ──");
    let mut refused: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut ok, mut no) = (0usize, 0usize);
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        let Ok(file) = syn::parse_file(&src) else { continue };
        let names: Vec<String> = file
            .items
            .iter()
            .filter_map(|it| match it {
                syn::Item::Fn(f)
                    if f.attrs.iter().any(|a| {
                        a.path().segments.last().is_some_and(|g| g.ident == "hardware")
                    }) =>
                {
                    Some(f.sig.ident.to_string())
                }
                _ => None,
            })
            .collect();
        let multi = names.len() > 1;
        for name in names {
            let module = if multi { Some(name.as_str()) } else { None };
            let where_ = format!(
                "{}::{name}",
                path.strip_prefix(root).unwrap_or(path).display()
            );
            match copper_codegen::transpile_source(
                &src,
                module,
                &copper_codegen::EmitConfig::default(),
            ) {
                Ok(_) => ok += 1,
                Err(e) => {
                    no += 1;
                    // Group by the error's SHAPE, not its text: a span and a name
                    // make every message unique and would turn this into a list.
                    let msg = e.to_string();
                    // Strip a leading `LINE:COL: ` span, which otherwise makes every
                    // message unique and turns the grouping back into a list.
                    let body = match msg.split_once(": ") {
                        Some((head, rest))
                            if head.split(':').all(|p| p.parse::<u32>().is_ok()) =>
                        {
                            rest
                        }
                        _ => msg.as_str(),
                    };
                    let key = body.chars().take(72).collect::<String>();
                    refused.entry(key).or_default().push(where_);
                }
            }
        }
    }
    println!("  {ok} transpile, {no} refused\n");
    let mut groups: Vec<_> = refused.iter().collect();
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    for (reason, mods) in groups {
        println!("  [{:>2}] {reason}", mods.len());
        for m in mods.iter().take(3) {
            println!("         {m}");
        }
        if mods.len() > 3 {
            println!("         … and {} more", mods.len() - 3);
        }
    }
}
