//! The mechanical columns of the cycle-dataflow derivation table.
//!
//! `design_docs/CYCLE_DATAFLOW_SEMANTICS.md` §7 requires, before any code moves, a
//! per-module table over the whole corpus: each module's clock phases classified by
//! **anchor** (opening vs closing), with a predicted trace under the model versus
//! today. The facts half of that table must not be a hand scan — it comes from
//! `copper_analysis::derivation_facts`, the same CFG authority every timing rule
//! reads — and this bin prints it so the table regenerates instead of going stale:
//!
//! ```text
//! cargo run -q -p copper-codegen --bin derivation-audit
//! ```
//!
//! What is mechanical here and what is not:
//!
//! * **facts** (phases, anchors-first-cut, plain-`Out` writes, forwarding
//!   observability, today's guard verdicts, transpile ground truth) — computed.
//! * **the disposition column is a FIRST CUT**, not a derivation: `sv-changes` /
//!   `review` rows must each be hand-derived (and the interesting ones measured)
//!   in `design_docs/DERIVATION_TABLE.md` before they are believed. The anchor
//!   column over-approximates (comb-path reachability, not value dependence) and
//!   cannot see `RegOut`-commit input dependence, both documented on
//!   `Cfg::derivation_facts`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            // examples/cpu/old/ is untracked scratch with pre-subset spellings
            // (Vec ports); the sweep excludes it and so does this table.
            if p.file_name().is_some_and(|n| n == "old") {
                continue;
            }
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Mode ident and flags from the `#[hardware(…)]` attribute. Reads the raw token
/// stream — `parse_args::<syn::Ident>()` fails outright once a flag is present,
/// which is the exact bug class CLAUDE.md warns about (it silently dropped
/// opted-out modules from three corpus scans).
fn attr_mode(f: &syn::ItemFn) -> (String, bool) {
    for a in &f.attrs {
        if !a.path().segments.last().is_some_and(|s| s.ident == "hardware") {
            continue;
        }
        let toks = match &a.meta {
            syn::Meta::List(l) => l.tokens.to_string(),
            _ => String::new(),
        };
        let mode = toks
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .find(|s| !s.is_empty())
            .unwrap_or("sequential")
            .to_string();
        let optout = toks.contains("allow_pretick_alignment");
        return (mode, optout);
    }
    ("?".into(), false)
}

struct Row {
    module: String,
    mode: String,
    optout: bool,
    ports: String,
    ticks: String,
    phases: String,
    guards: Vec<&'static str>,
    transpiles: bool,
    disposition: &'static str,
    detail: String,
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("workspace root");
    let mut files = Vec::new();
    // All of tests/ (not just tests/fixtures/): the D1-family demonstration
    // witnesses live inline in tests/*.rs, and they are the rows the model must
    // give a defined meaning (or a derived refusal) to.
    for d in ["examples", "tests", "src"] {
        collect(&root.join(d), &mut files);
    }
    files.sort();

    let mut rows: Vec<Row> = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        let Ok(file) = syn::parse_file(&src) else { continue };
        let fns: Vec<&syn::ItemFn> = file
            .items
            .iter()
            .filter_map(|it| match it {
                syn::Item::Fn(f)
                    if f.attrs.iter().any(|a| {
                        a.path().segments.last().is_some_and(|s| s.ident == "hardware")
                    }) =>
                {
                    Some(f)
                }
                _ => None,
            })
            .collect();
        let multi = fns.len() > 1;
        for f in fns {
            let name = f.sig.ident.to_string();
            let module = format!(
                "{}::{name}",
                path.strip_prefix(root).unwrap_or(path).display()
            );
            let (mode, optout) = attr_mode(f);

            // Port kinds from the signature, by outer type name.
            let mut kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
            for arg in &f.sig.inputs {
                if let syn::FnArg::Typed(pt) = arg {
                    if let syn::Type::Path(tp) = &*pt.ty {
                        let k = match tp.path.segments.last().map(|s| s.ident.to_string()) {
                            Some(s) if s == "In" => "In",
                            Some(s) if s == "Out" => "Out",
                            Some(s) if s == "RegOut" => "RegOut",
                            Some(s) if s == "Clock" => "Clk",
                            Some(s) if s == "Memory" => "Mem",
                            _ => "other",
                        };
                        *kinds.entry(k).or_default() += 1;
                    }
                }
            }
            let ports = ["In", "Out", "RegOut", "Mem"]
                .iter()
                .filter_map(|k| kinds.get(*k).map(|n| format!("{n}{k}")))
                .collect::<Vec<_>>()
                .join("+");

            let transpiles = copper_codegen::transpile_source(
                &src,
                if multi { Some(name.as_str()) } else { None },
                &copper_codegen::EmitConfig::default(),
            )
            .is_ok();

            let facts = copper_analysis::derivation_facts(f);
            let Some(facts) = facts else {
                rows.push(Row {
                    module,
                    mode,
                    optout,
                    ports,
                    ticks: "-".into(),
                    phases: "-".into(),
                    guards: vec![],
                    transpiles,
                    disposition: "unaffected",
                    detail: "no clock".into(),
                });
                continue;
            };

            let mut guards = Vec::new();
            if !copper_analysis::unprotected_pretick_out_write(f).is_empty() {
                guards.push("D1");
            }
            if !copper_analysis::unprotected_trailing_out_write(f).is_empty() {
                guards.push("trail");
            }
            if !copper_analysis::multi_phase_out_write(f).is_empty() {
                guards.push("mphase");
            }
            if !copper_analysis::multi_write_collapse(f).is_empty() {
                guards.push("mwrite");
            }
            if !copper_analysis::pretick_out_write_before_update(f).is_empty() {
                guards.push("V8");
            }

            // Per-phase summary + disposition. With the V8 rule landed, the §4
            // commit-frontier taxonomy clears closing-phase writes mechanically:
            // a write is DERIVED-legal when its operands are frontier (constants,
            // or registers — the barrier-trapped update-after shape is guarded
            // corpus-wide by `pretick_out_write_before_update`). The one class
            // the taxonomy cannot clear is a write fed by a same-cycle input
            // WIRE (dual-anchor read, table §8 item 1) — reported as `input-fed`
            // and covered by a class derivation in DERIVATION_TABLE.md §4b.
            // Severity: guarded > sv-changes > review > input-fed > unchanged.
            let mut phase_bits = Vec::new();
            let mut sv_changes = false;
            let review = false;
            let mut input_fed = false;
            let mut read_retime = false;
            for ph in &facts.phases {
                let tag = if ph.is_head { "H" } else { "P" };
                // Closing-anchor evidence: an input feeds a commit (register or
                // memory staging), or an input read steers control (the implicit
                // `pc` commit).
                let closing = ph.input_reaches_commit || ph.control_input_read;
                let mut bit = format!("{tag}:{}", if closing { "close" } else { "open" });
                if ph.assigns_registers {
                    bit.push_str("+reg");
                }
                if ph.stages_memory {
                    bit.push_str("+mem");
                }
                if !ph.plain_out_writes.is_empty() {
                    bit.push_str(&format!("+out{}", ph.plain_out_writes.len()));
                }
                if !ph.input_fed_writes.is_empty() {
                    bit.push_str("[in-fed]");
                    input_fed = true;
                }
                if !ph.forwarded_observable.is_empty() {
                    bit.push_str("[fwd]");
                    if !closing {
                        // Opening-anchored: the model's emission is the forwarded
                        // expression; today's is unforwarded. SV trace changes.
                        sv_changes = true;
                    }
                    // Closing + [fwd] = write after the update: the forwarded
                    // value IS the committing one (V8b / lfsr, measured) —
                    // frontier, derived-legal.
                }
                // An input read not feeding any commit, in a phase that does
                // commit: today it samples Deferred (closing), the model samples
                // it at the opening. Read retiming to review.
                if ph.has_input_read
                    && !closing
                    && (ph.assigns_registers || ph.stages_memory)
                {
                    read_retime = true;
                }
                phase_bits.push(bit);
            }

            let disposition = if !guards.is_empty() {
                "guarded-today"
            } else if sv_changes {
                "sv-changes"
            } else if review || read_retime {
                "review"
            } else if input_fed {
                "input-fed"
            } else {
                "unchanged"
            };
            let mut detail = phase_bits.join(" ");
            if read_retime {
                detail.push_str(" (read-retime)");
            }
            if !facts.registers.is_empty() {
                detail.push_str(&format!(" regs={}", facts.registers.len()));
            }

            rows.push(Row {
                module,
                mode,
                optout,
                ports,
                ticks: format!(
                    "{}{}",
                    facts.tick_nodes,
                    if facts.multi_tick { "*" } else { "" }
                ),
                phases: facts.phases.len().to_string(),
                guards,
                transpiles,
                disposition,
                detail,
            });
        }
    }

    // ── The table ──────────────────────────────────────────────────────────
    println!("| module | mode | ports | ticks | ph | guards | SV? | first-cut | phase detail |");
    println!("|---|---|---|---|---|---|---|---|---|");
    for r in &rows {
        println!(
            "| {} | {}{} | {} | {} | {} | {} | {} | {} | {} |",
            r.module,
            r.mode,
            if r.optout { " (opt-out)" } else { "" },
            if r.ports.is_empty() { "-".into() } else { r.ports.clone() },
            r.ticks,
            r.phases,
            if r.guards.is_empty() { "-".into() } else { r.guards.join("+") },
            if r.transpiles { "yes" } else { "no" },
            r.disposition,
            r.detail,
        );
    }

    // ── Totals, so the doc's summary regenerates too ───────────────────────
    let mut tally: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &rows {
        *tally.entry(r.disposition).or_default() += 1;
    }
    println!("\ntotals: {} modules", rows.len());
    for (k, n) in &tally {
        println!("  {k:<14} {n}");
    }
    println!(
        "  (ticks column: direct tick nodes, '*' = crosses more than one edge per \
         iteration; 'ph' = Comb-component phases; dispositions past 'unchanged' \
         require hand derivation in design_docs/DERIVATION_TABLE.md)"
    );
}
