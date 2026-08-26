//! **The structural guard against the class this project has recorded five times:**
//! *a check that counts a syntactic feature (`clk.tick().await`) rather than the thing
//! it means (a clock phase), placed downstream of a pass that legitimately removes
//! that feature.*
//!
//! `control_extract` rewrites a body whose ticks live inside branches or loops into a
//! single-tick `match pc` FSM. Measured (`tests/timing_model_derivations.rs`): the
//! source and lowered phase counts agree on 58 corpus modules and disagree on 8 — and
//! every disagreement is an extracted module whose lowered count is **1**. So any
//! decision made in codegen by counting ticks or phases sees one where there are
//! several, and three such checks have already had to be moved to `copper-analysis`,
//! where they run on the source: `multi_phase_out_write`, `check_memory_staging`,
//! `memory_result_drives_plain_out`.
//!
//! Nothing stopped a fourth from being written. This is that stop: it pins every
//! function in the transpiler that both **reasons about clock phases** and **can
//! produce an error**, so a new one fails here and has to be justified in review.
//!
//! # What to do when this test fails
//!
//! Ask what the new site is:
//!
//! * **A semantic rule** — "this design is wrong / unsupported" — belongs in
//!   `copper-analysis`, on the `syn::ItemFn`, where the ticks are still visible and
//!   the sim front-end gets the same rule for free. Three precedents above.
//! * **A limitation of this lowering path** — "this pipeline cannot express that" —
//!   legitimately lives here, because it is a statement about the code generator and
//!   not about the language. Add it below with that reason.
//!
//! The distinction is not cosmetic: `shir_lower`'s trailing-statement refusal was
//! filed as the first and audited as the second — the extracted path accepted the same
//! shape and agreed with its SystemVerilog — and was resolved on 2026-08-25 by
//! deciding the semantics rather than by moving the check.

use std::collections::BTreeSet;

/// Tokens that mean "this code is reasoning about clock phases or tick structure".
/// Deliberately specific: a bare `tick` would match half the transpiler.
const PHASE_MARKERS: &[&str] = &[
    "split_at_ticks",
    "n_ticks",
    "AwaitTick",
    "phase_idx",
    "phases",
    "is_tick",
];

/// Every function in the transpiler that reasons about phases AND can fail, with why
/// it is allowed to. Sorted; `<file>::<fn>`.
///
/// A **limitation** is a statement about what this lowering path can express, and
/// belongs here. A **rule** is a statement about the language, and belongs in
/// `copper-analysis` — the three that moved are named in this file's header.
const EXPECTED: &[(&str, &str)] = &[
    (
        "chir_lower.rs::lower_expr_stmt",
        "lowering: statement dispatch, which refuses constructs CHIR has no node for. \
         Reaches ticks only to place them.",
    ),
    (
        "chir_lower.rs::lower_seq_body",
        "lowering: builds the sequential body and refuses a loop with no tick at all — \
         a structural precondition of the IR, not a phase count.",
    ),
    (
        "chir_lower.rs::nested_loop_error",
        "LIMITATION of the linear path: builds the diagnostic for a tick-bearing nested \
         loop, descending to the innermost one at fault. Reached only when \
         `control_extract` has declined the module.",
    ),
    (
        "chir_lower.rs::reject_tick_in_branch",
        "LIMITATION of the linear path: a tick inside a branch cannot be lowered \
         without the `pc` FSM. `control_extract` runs first and removes the shape, so \
         reaching this means extraction declined — a statement about this pipeline.",
    ),
    (
        "chir_lower.rs::validate_stmts",
        "lowering: rejects statements CHIR cannot represent. Tick-aware only to allow \
         them through.",
    ),
    (
        "shir_lower.rs::check_no_tick_in_branch",
        "LIMITATION of the linear path, mirroring `reject_tick_in_branch` one IR later.",
    ),
    (
        "shir_lower.rs::lower_seq_body",
        "lowering: the segment→phase construction itself. It carried the one open \
         instance of the class — the trailing-statement refusal — until the semantics \
         were decided on 2026-08-25 (SYNCHRONOUS_SEMANTICS.md, 'Trailing statements'): \
         those statements are in the head's cycle and now lower into phase 0, so the \
         refusal is gone and what is left here is construction.",
    ),
    (
        "shir_lower.rs::validate_seq_chir",
        "lowering: CHIR preconditions for the sequential path (a clock, a loop, a \
         tick). Structural, not a phase count.",
    ),
    (
        "vlir_lower.rs::lower_seq",
        "lowering: emits the phase-guarded `always_comb`/`always_ff`. The phase count \
         is the thing being emitted here, not a premise being tested.",
    ),
    (
        "vlir_lower.rs::reject_memory_driven_comb_outputs",
        "RULE, kept deliberately: it gives up on `phases.len() < 2` and so cannot see \
         an extracted module, which is why the same rule now also runs on the source \
         as `copper_analysis::memory_result_drives_plain_out`. Retained for the \
         non-extracted path it can see; the source copy is the authority.",
    ),
];

/// Walk every `fn` in the file, including methods in `impl` blocks.
fn functions(file: &syn::File) -> Vec<(String, String)> {
    fn body_text(block: &syn::Block) -> String {
        use quote::ToTokens;
        block.to_token_stream().to_string()
    }
    let mut out = Vec::new();
    fn walk(items: &[syn::Item], out: &mut Vec<(String, String)>) {
        for item in items {
            match item {
                syn::Item::Fn(f) => out.push((f.sig.ident.to_string(), body_text(&f.block))),
                syn::Item::Impl(i) => {
                    for it in &i.items {
                        if let syn::ImplItem::Fn(f) = it {
                            out.push((f.sig.ident.to_string(), body_text(&f.block)));
                        }
                    }
                }
                syn::Item::Mod(m) => {
                    if let Some((_, items)) = &m.content {
                        walk(items, out);
                    }
                }
                _ => {}
            }
        }
    }
    walk(&file.items, &mut out);
    out
}

#[test]
fn no_new_phase_sensitive_check_in_the_transpiler() {
    // Test modules are excluded: a test that asserts on phase behaviour is not a
    // check the compiler applies to a design.
    let files = ["chir_lower.rs", "shir_lower.rs", "vlir_lower.rs", "emit.rs", "control_extract.rs"];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut scanned = 0usize;
    for name in files {
        let src = std::fs::read_to_string(dir.join(name))
            .unwrap_or_else(|e| panic!("{name} is readable: {e}"));
        let file = syn::parse_file(&src).unwrap_or_else(|e| panic!("{name} parses: {e}"));
        for (fn_name, body) in functions(&file) {
            scanned += 1;
            // `#[cfg(test)]` bodies come along in the parse; skip anything that is
            // plainly a test assertion rather than compiler logic.
            if body.contains("assert !") || body.contains("assert_eq !") {
                continue;
            }
            // A `Display`/`Debug` impl is message TEXT. `vlir_lower`'s mentions phases
            // because that is what the diagnostic says, which is not a decision.
            if fn_name == "fmt" {
                continue;
            }
            let phase_sensitive = PHASE_MARKERS.iter().any(|m| body.contains(m));
            let can_fail = body.contains("Err (") || body.contains("Error ::");
            if phase_sensitive && can_fail {
                found.insert(format!("{name}::{fn_name}"));
            }
        }
    }

    assert!(scanned > 100, "only scanned {scanned} functions — the walk stopped working");

    let expected: BTreeSet<String> = EXPECTED.iter().map(|(n, _)| n.to_string()).collect();
    let added: Vec<&String> = found.difference(&expected).collect();
    let gone: Vec<&String> = expected.difference(&found).collect();

    assert!(
        added.is_empty(),
        "NEW phase-sensitive failure site(s) in the transpiler: {added:?}\n\n\
         Decide which it is before adding it to EXPECTED:\n\
         · a RULE about the language — move it to copper-analysis, on the source, \
         where `control_extract` has not yet collapsed the phases. Three checks have \
         already had to make that move, and each was silently blind first.\n\
         · a LIMITATION of this lowering path — legitimate here; add it with that \
         reason.\n\
         See tests/timing_model_derivations.rs for the measurement that says a \
         phase count in codegen is 1 for every extracted module."
    );
    assert!(
        gone.is_empty(),
        "listed phase-sensitive site(s) no longer found — if one moved to \
         copper-analysis, delete its entry and say so in the commit: {gone:?}"
    );
}
