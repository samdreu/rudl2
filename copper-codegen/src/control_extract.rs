//! Control extraction — straight-line + `if`/`else`/`match` branching (increment
//! A), and repeating waits: `loop { … clk.tick().await; … }` with `break`
//! (increment B).
//!
//! Rewrites an async control-flow loop body whose `clk.tick().await`s appear
//! *inside branches* into an explicit **single-tick FSM**:
//!
//! ```text
//! loop { <async control flow with ticks in branches> }
//!         ↓
//! let mut pc: u8 = 0;
//! loop {
//!     match pc { 0 => {..; pc = ..}, 1 => {..}, _ => {} }
//!     clk.tick().await;                 // exactly one tick
//! }
//! ```
//!
//! The output is the same shape the existing single-tick pipeline already lowers
//! correctly (`mac_fsm`, `det_010`): a `match` on a program-counter register with
//! one unconditional trailing tick. So all of CHIR/SHIR/VLIR/emit are reused
//! unchanged; this pass is a pure FIR→FIR transform.
//!
//! This is a *source-level* pass. It runs before `lower_to_chir` and only fires
//! when a tick is nested inside a branch or a nested loop — linear
//! (top-level-tick) modules keep the proven lowering path untouched. See
//! `design_docs/CONTROL_EXTRACTION.md`.
//!
//! ## Repeating waits (increment B)
//!
//! ```text
//! loop { if ready { break; } clk.tick().await; }   // wait until ready
//! ```
//!
//! A nested `loop` gets a **head state** H, and its body is lowered once with H as
//! the back-edge target: a tick at the end of the body emits `pc = H` (stay), and
//! `break` inlines the loop's continuation (leave, in the same cycle — breaking is
//! not a clock boundary). Entering the loop must not cost a cycle either, so the
//! *already-lowered* body is cloned into the entry point. Cloning the lowered form
//! rather than re-lowering the source matters: the two copies then share the
//! sub-states their ticks allocated, so a loop costs ONE extra state, not a
//! doubling.
//!
//! `continue` is not handled: jumping to the head mid-cycle would need the head's
//! lowered body at a point where it does not exist yet. It is refused up front
//! rather than mis-lowered.

use copper_core::frontend_ir::{
    ExprAssign, ExprBinary, ExprBlock, ExprBreak, ExprIf, ExprLit, ExprLoop, ExprMatch,
    ExprMatchArm, ExprPath, ExprStmt, ExprUnary,
    ExprType, FrontendModuleIR, LocalStmt, RawStmt, RawStmtKind, RawTypeRef, SourceSpan,
};

/// The synthesized program-counter register name — a PLACEHOLDER during
/// extraction, renamed to its final spelling at the end of `extract_control`:
/// `pc` when the module doesn't already use that name, `pc_1` (`pc_2`, …)
/// when it does. The pipelined CPU has its own 32-bit `pc` register, and both
/// emitting under one name was a duplicate declaration in the SystemVerilog.
const PC: &str = "__cx_pc";

/// The `pc` register's Rust type. `match`-arm literals carry the matching suffix
/// so they lower to the same width as `pc` (else Verilator flags `WIDTHEXPAND` on
/// the `case` items, e.g. `64'd0` vs an 8-bit scrutinee).
/// The `pc` register's Rust type, chosen to hold `states` distinct values.
///
/// It was a fixed `u8`, and nothing checked the state count against it. The UART
/// receiver flattens to 788 states, so `pc = 256` wrapped onto `0` and the emitted
/// `case` had overlapping arms — a module that reads as well-formed SystemVerilog
/// and runs the wrong state. Verilator's `CASEOVERLAP` caught it here; with that
/// lint off it would simply have executed the wrong arm.
///
/// `match`-arm literals carry the matching suffix so they lower to the same width
/// as `pc` (else Verilator flags `WIDTHEXPAND` on the `case` items, e.g. `64'd0`
/// against an 8-bit scrutinee) — which is why the type and the suffix are one
/// decision and not two.
fn pc_ty(states: usize) -> &'static str {
    match states {
        0..=256 => "u8",
        257..=65536 => "u16",
        _ => "u32",
    }
}

/// Marker for a **zero-time** transition to another state — a jump that must not
/// cost a clock cycle, so it cannot be a `pc` assignment.
///
/// `pc = H` is a state transition, and every state transition in the extracted FSM
/// (`loop { match pc { … }; clk.tick().await; }`) spends exactly one cycle. A
/// `continue` spends none: it re-enters the loop head in the SAME cycle. So it is
/// emitted as `__copper_goto = H` and `splice_zero_time_gotos` later replaces it
/// with state H's own statements — the same "inline the continuation rather than
/// transition to it" rule `break` already follows, applied to the back edge.
///
/// The name is reserved and never reaches CHIR; `extract_control` declines the
/// module rather than emit one that survived the splice, so a leak cannot become
/// a stray assignment in the output.
const GOTO: &str = "__copper_goto";

/// Rewrite `while <cond> { … clk.tick().await; }` into the repeating-wait shape
/// this pass already flattens: `loop { if !<cond> { break; } … }`.
///
/// The two are the same program. `while` tests before each iteration and the
/// tick is the last statement of the body, which is exactly the supported
/// ordering — the one where the test reads outside the window that makes a
/// simulator and a flip-flop disagree (see `body_ends_at_a_clock_boundary`). So this is
/// sugar, not a new control-flow construct: after the rewrite the wait lands on
/// machinery that is already verified end-to-end.
///
/// Only a **tick-bearing** `while` is rewritten. A `while` with no tick is a
/// combinational loop, which has to be fully unrolled to be hardware and so
/// needs a compile-time trip count; `for` is how that is spelled, and rewriting
/// it here would bury the point under a generic "nested loop never terminates"
/// error. It keeps its own diagnostic instead.
pub fn desugar_tick_waits(fir: &mut FrontendModuleIR) {
    let mut body = std::mem::take(&mut fir.raw_statements);
    desugar_stmts(&mut body);
    fir.raw_statements = body;
}

fn desugar_stmts(stmts: &mut Vec<RawStmt>) {
    for s in stmts.iter_mut() {
        match &mut s.kind {
            RawStmtKind::Expr(es) => desugar_expr(&mut es.expr),
            RawStmtKind::Local(l) => {
                if let Some(init) = l.init.as_mut() {
                    desugar_expr(init);
                }
            }
            RawStmtKind::Item(_) => {}
        }
    }
}

fn desugar_expr(e: &mut ExprType) {
    match e {
        ExprType::While(w) if stmts_contain_tick(&w.body) => {
            desugar_stmts(&mut w.body);
            let span = w.span;
            // `if !<cond> { break; }` — the loop's exit test, hoisted to the top
            // of the body so the tick stays last.
            let guard = expr_stmt(
                ExprType::If(ExprIf {
                    condition: Box::new(ExprType::Unary(ExprUnary {
                        op: "!".to_string(),
                        expr: w.condition.clone(),
                        span,
                    })),
                    then_block: vec![expr_stmt(
                        ExprType::Break(ExprBreak { label: None, expr: None, span }),
                        span,
                    )],
                    else_branch: None,
                    span,
                }),
                span,
            );
            let mut body = vec![guard];
            body.extend(w.body.iter().cloned());
            *e = ExprType::Loop(ExprLoop { body, span });
        }
        ExprType::While(w) => desugar_stmts(&mut w.body),
        ExprType::Loop(l) => desugar_stmts(&mut l.body),
        ExprType::ForLoop(f) => desugar_stmts(&mut f.body),
        ExprType::Block(b) => desugar_stmts(&mut b.stmts),
        ExprType::If(f) => {
            desugar_stmts(&mut f.then_block);
            if let Some(eb) = f.else_branch.as_mut() {
                desugar_expr(eb);
            }
        }
        ExprType::Match(m) => {
            for a in m.arms.iter_mut() {
                desugar_expr(&mut a.body);
            }
        }
        _ => {}
    }
}

// ── Counted repetition ────────────────────────────────────────────────────────

/// The synthesized counter for a `for _ in …` whose variable is `_`.
const ANON_COUNTER: &str = "__copper_ctr";

/// The counter's Rust type. 32 bits, matching what a `for`-loop variable already
/// lowers to (`chir_lower` seeds one as `u32`) and what a `const` bound emits as
/// (`localparam int`), so the comparison against `<end>` needs no widening.
const COUNTER_TY: &str = "u32";

/// Rewrite the counted `for`s **inside the module's top-level `loop`**, and only
/// those.
///
/// The restriction is not cosmetic. The rewrite replaces a `for` statement with a
/// `let` and a `loop`, and `extract_control` finds the module's own loop by taking
/// the FIRST top-level `loop` statement — so a `for` rewritten OUTSIDE that loop
/// would put a synthesized bounded loop ahead of it and the wrong one would be
/// extracted. A tick outside the top-level loop is not something the phase FSM
/// supports in any case, and `chir_lower` already says so; leaving it untouched
/// keeps that message.
pub fn desugar_counted_loops_in(fir: &mut FrontendModuleIR) {
    let Some(idx) = fir.raw_statements.iter().position(is_loop_stmt) else {
        return;
    };
    let RawStmtKind::Expr(es) = &mut fir.raw_statements[idx].kind else {
        return;
    };
    let ExprType::Loop(l) = &mut es.expr else {
        return;
    };
    let mut next_counter = 0usize;
    desugar_counted_loops(&mut l.body, &mut next_counter);
}

/// Rewrite `for <var> in <start>..<end> { <B>; clk.tick().await; }` into the
/// counted `loop` this pass already flattens:
///
/// ```text
/// let mut <var>: u32 = <start>;
/// loop {
///     if <var> >= <end> { break; }      // empty range, and the loop's own test
///     <B>
///     <var> = <var> + 1;
///     if <var> >= <end> { break; }      // leave BEFORE the last iteration's tick
///     clk.tick().await;
/// }
/// clk.tick().await;                     // …which is this one
/// ```
///
/// **A tick inside a `for` is counted REPETITION, not unrolling.** A UART bit
/// period is 434 cycles; unrolling it would be 434 states. The counter is a
/// register and the loop is one state, exactly as a hand-written wait is — which
/// is why this is a desugar and not a new state shape: afterwards the loop lands
/// on `lower_into`'s nested-loop path, already verified end to end.
///
/// # Why the last tick is moved OUT of the loop
///
/// This is the part that is not obvious, and the naive shape — one test at the
/// top, tick last, nothing after the loop — is measurably wrong by a cycle.
///
/// Breaking out of a nested loop is not a clock boundary: `lower_into` inlines the
/// enclosing loop's continuation into the breaking path and carries on in the same
/// cycle. But when that continuation contains no boundary of its own, the lowering
/// falls off the end and emits `pc = <enclosing head>` — a state transition, and
/// the FSM spends a cycle on it. With the tick left inside, a counted delay's
/// continuation IS empty, so every pass through the loop cost one cycle more than
/// the source asks for (measured on `for _ in 0..3`: the simulator rewrote the
/// output every 3rd cycle, the transpiled module every 4th).
///
/// That is `TODO` cause K's back edge, which its own entry records as unverified
/// because no well-formed hand-written program could reach it. Leaving the final
/// tick in the ENCLOSING statement list means the break always continues into a
/// real clock boundary, so the shape is never produced — the same disposition as
/// the tick-ordering rule: make the divergent form unwritable rather than teach
/// the flattener to special-case it.
///
/// The second test is what pays for that: the loop leaves before the final tick,
/// and the hoisted one supplies it. `B` is not duplicated.
///
/// # `>=` rather than `==`, and the empty range
///
/// `for i in 5..3` yields nothing in Rust; `i == 3` would never fire and the loop
/// would spin forever, while `i >= 3` breaks at once. A statically empty range is
/// dropped outright, since the hoisted tick would otherwise cost a cycle the
/// source does not have. When the bounds are not literals — `0..CLKS_PER_BIT` —
/// emptiness is not decidable here, and an empty one would produce that spurious
/// cycle. `copper_analysis::may_exit_without_tick` has the same hole for the same
/// reason (it treats a `for` as a guaranteed clock boundary, which an empty range
/// is not) and both need const evaluation to close.
fn desugar_counted_loops(stmts: &mut Vec<RawStmt>, next_counter: &mut usize) {
    let mut out: Vec<RawStmt> = Vec::with_capacity(stmts.len());
    for mut s in std::mem::take(stmts) {
        // Descend first: an inner `for` must already be a `loop` with its tick
        // hoisted before the outer one looks at where its own body ends.
        descend_counted(&mut s, next_counter);

        let expanded = match &s.kind {
            RawStmtKind::Expr(es) => match &es.expr {
                ExprType::ForLoop(f) if stmts_contain_tick(&f.body) => {
                    expand_counted_for(f, s.order, next_counter)
                }
                _ => None,
            },
            _ => None,
        };
        match expanded {
            Some(rewritten) => out.extend(rewritten),
            None => out.push(s),
        }
    }
    *stmts = out;
}

/// Recurse into every statement form that can contain a `for`.
fn descend_counted(s: &mut RawStmt, next_counter: &mut usize) {
    match &mut s.kind {
        RawStmtKind::Expr(es) => descend_counted_expr(&mut es.expr, next_counter),
        RawStmtKind::Local(l) => {
            if let Some(init) = l.init.as_mut() {
                descend_counted_expr(init, next_counter);
            }
        }
        RawStmtKind::Item(_) => {}
    }
}

fn descend_counted_expr(e: &mut ExprType, next_counter: &mut usize) {
    match e {
        ExprType::Loop(l) => desugar_counted_loops(&mut l.body, next_counter),
        ExprType::While(w) => desugar_counted_loops(&mut w.body, next_counter),
        ExprType::ForLoop(f) => desugar_counted_loops(&mut f.body, next_counter),
        ExprType::Block(b) => desugar_counted_loops(&mut b.stmts, next_counter),
        ExprType::If(f) => {
            desugar_counted_loops(&mut f.then_block, next_counter);
            if let Some(eb) = f.else_branch.as_mut() {
                descend_counted_expr(eb, next_counter);
            }
        }
        ExprType::Match(m) => {
            for a in m.arms.iter_mut() {
                descend_counted_expr(&mut a.body, next_counter);
            }
        }
        _ => {}
    }
}

/// The statements a tick-bearing counted `for` becomes, or `None` when this pass
/// declines it — in which case the `for` is left exactly as written and
/// `chir_lower` reports it with its own span and message.
///
/// Declined shapes:
/// * a header that is not an exclusive `start..end` range;
/// * a body whose last statement is not `clk.tick().await`. The tick has to be
///   last for the same reason it does in a `loop` (see
///   `body_ends_at_a_clock_boundary`), and after the recursive descent above an
///   inner counted `for` already ends in one — so what is left here is a body
///   ending in a hand-written nested loop, whose own boundary this rewrite has no
///   tick to hoist.
fn expand_counted_for(
    f: &copper_core::frontend_ir::ExprForLoop,
    order: usize,
    next_counter: &mut usize,
) -> Option<Vec<RawStmt>> {
    let span = f.span;
    let (start, end) = match &*f.iter {
        ExprType::Range(r) if !r.inclusive => {
            let start = match &r.start {
                Some(s) => (**s).clone(),
                None => ExprType::Lit(ExprLit { text: "0".to_string(), span }),
            };
            (start, (*r.end.as_ref()?).as_ref().clone())
        }
        _ => return None,
    };

    // The boundary has to be the body's last statement, so that hoisting it out
    // leaves a well-formed loop behind.
    let boundary = f.body.last().filter(|s| is_tick_stmt(s))?.clone();
    let work = &f.body[..f.body.len() - 1];

    // A statically empty range is zero cycles and zero work: drop it, rather than
    // emit a loop that breaks at once and then takes the hoisted tick anyway.
    if let (Some(a), Some(b)) = (int_literal(&start), int_literal(&end)) {
        if a >= b {
            return Some(Vec::new());
        }
    }

    let pat = f.pat_text.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    // `for _ in …` still needs a counter, it just has no name to reuse. Numbering
    // is per module, so two anonymous delays never share a register.
    let var = if pat == "_" || pat.is_empty() {
        let n = *next_counter;
        *next_counter += 1;
        format!("{ANON_COUNTER}{n}")
    } else {
        pat
    };

    let exit_test = || {
        expr_stmt(
            ExprType::If(ExprIf {
                condition: Box::new(ExprType::Binary(ExprBinary {
                    left: Box::new(path_expr(&var, span)),
                    op: ">=".to_string(),
                    right: Box::new(end.clone()),
                    span,
                })),
                then_block: vec![expr_stmt(
                    ExprType::Break(ExprBreak { label: None, expr: None, span }),
                    span,
                )],
                else_branch: None,
                span,
            }),
            span,
        )
    };
    let incr = expr_stmt(
        ExprType::Assign(ExprAssign {
            left: Box::new(path_expr(&var, span)),
            right: Box::new(ExprType::Binary(ExprBinary {
                left: Box::new(path_expr(&var, span)),
                op: "+".to_string(),
                right: Box::new(ExprType::Lit(ExprLit { text: "1".to_string(), span })),
                span,
            })),
            span,
        }),
        span,
    );

    let mut loop_body = vec![exit_test()];
    loop_body.extend(work.iter().cloned());
    loop_body.push(incr);
    loop_body.push(exit_test());
    loop_body.push(boundary.clone());

    let decl = RawStmt {
        order,
        kind: RawStmtKind::Local(LocalStmt {
            is_mut: true,
            ty: Some(RawTypeRef { ty_text: COUNTER_TY.to_string(), span }),
            name: var,
            init: Some(start),
            attrs: Vec::new(),
            span,
        }),
        text: String::new(),
        span,
    };
    let loop_stmt = RawStmt {
        order,
        kind: RawStmtKind::Expr(ExprStmt {
            expr: ExprType::Loop(ExprLoop { body: loop_body, span }),
            has_semi: false,
            span,
        }),
        text: String::new(),
        span,
    };
    Some(vec![decl, loop_stmt, boundary])
}

/// A range bound that is a plain integer literal, for the empty-range check. A
/// named `const` deliberately does NOT resolve here: this pass has no constant
/// environment, and inventing a partial one is how two halves of a lowering start
/// disagreeing.
fn int_literal(e: &ExprType) -> Option<i128> {
    match e {
        ExprType::Lit(l) => l
            .text
            .trim_end_matches(|c: char| c.is_alphabetic() || c == '_')
            .parse::<i128>()
            .ok(),
        _ => None,
    }
}

/// If the module's top-level `loop` has a tick nested inside a branch, flatten
/// it in place to a single-tick `match pc` FSM. No-op otherwise.
pub fn extract_control(fir: &mut FrontendModuleIR) {
    // Find the top-level `loop { .. }` statement and clone its body.
    let Some(loop_idx) = fir.raw_statements.iter().position(is_loop_stmt) else {
        return;
    };
    let loop_body = match &fir.raw_statements[loop_idx].kind {
        RawStmtKind::Expr(es) => match &es.expr {
            ExprType::Loop(l) => l.body.clone(),
            _ => return,
        },
        _ => return,
    };

    // Gate: only fire when a tick lives inside a branch. A module whose ticks are
    // all top-level loop statements is already handled by the linear phase FSM.
    if !loop_needs_extraction(&loop_body) {
        return;
    }

    // ...and only when every tick sits somewhere this pass can actually flatten.
    // DECLINE rather than transform: the linear path downstream then reports the
    // offending construct with its own span, which is a far better diagnostic than
    // anything this pass could produce.
    //
    // This gate also stops the two halves of the pass from disagreeing again:
    // `expr_contains_tick` descends into every loop form, and anything it finds
    // that the flattener cannot handle used to reach an `.expect` — a raw panic on
    // user input, with no span and no name for the construct at fault.
    if contains_unflattenable_control(&loop_body).is_some() {
        return;
    }

    let span = fir.raw_statements[loop_idx].span;

    // Flatten the CFG into per-state segment bodies (state 0 = loop head).
    let mut sm = StateMachine::new();
    let mut state0 = Vec::new();
    lower_into(&loop_body, &mut state0, &mut sm, &LoopCtx::module());
    sm.set_body(0, state0);

    // A `let` in one state that is read in another cannot stay where it is: a
    // match arm scopes its locals, so the reader sees an undefined name. Hoist
    // those to pre-loop `let mut` declarations, which the existing register path
    // then handles — the same treatment `pc` itself gets.
    let hoisted = hoist_cross_state_locals(&mut sm);

    // `continue` left a zero-time goto in place of a state transition; every state
    // body exists now, so the target can be inlined. Declining on failure keeps the
    // rule this pass is built on: never mis-lower, always fall back to the linear
    // path's refusal.
    if !splice_zero_time_gotos(&mut sm) {
        return;
    }

    // Build `match pc { 0 => {..}, .., _ => {} }`. The state count is final here,
    // so this is the first point `pc`'s width can be chosen correctly.
    let pc_suffix = pc_ty(sm.states.len());
    let mut arms = Vec::new();
    for (i, body) in sm.states.iter().enumerate() {
        arms.push(ExprMatchArm {
            pattern_text: format!("{i}{pc_suffix}"),
            guard: None,
            body: Box::new(block_expr(body.clone(), span)),
            span,
        });
    }
    arms.push(ExprMatchArm {
        pattern_text: "_".to_string(),
        guard: None,
        body: Box::new(block_expr(Vec::new(), span)),
        span,
    });
    let match_stmt = expr_stmt(
        ExprType::Match(ExprMatch {
            scrutinee: Box::new(path_expr(PC, span)),
            arms,
            span,
        }),
        span,
    );

    // The single unconditional trailing tick — reuse a real `clk.tick().await`
    // node from the source so it is byte-for-byte the shape CHIR expects.
    //
    // Declining here is defence in depth: the gate above should guarantee a tick
    // this walk can find, and if the two ever drift apart again the pass must fall
    // back to leaving the module alone, never crash.
    let Some(tick) = find_tick_stmt(&loop_body) else {
        return;
    };

    let new_loop = RawStmt {
        order: fir.raw_statements[loop_idx].order,
        kind: RawStmtKind::Expr(ExprStmt {
            expr: ExprType::Loop(ExprLoop {
                body: vec![match_stmt, tick],
                span,
            }),
            has_semi: false,
            span,
        }),
        text: String::new(),
        span,
    };

    // `let mut pc: u8 = 0;` immediately before the loop → the existing register
    // promotion turns it into a register (it crosses the one tick).
    let pc_decl = RawStmt {
        order: fir.raw_statements[loop_idx].order,
        kind: RawStmtKind::Local(LocalStmt {
            is_mut: true,
            ty: Some(RawTypeRef {
                ty_text: pc_suffix.to_string(),
                span,
            }),
            name: PC.to_string(),
            init: Some(ExprType::Lit(ExprLit {
                text: "0".to_string(),
                span,
            })),
            attrs: Vec::new(),
            span,
        }),
        text: String::new(),
        span,
    };

    fir.raw_statements[loop_idx] = new_loop;
    fir.raw_statements.insert(loop_idx, pc_decl);
    // Ahead of `pc`, so an init that mentions another hoisted local still reads
    // in source order.
    for decl in hoisted.into_iter().rev() {
        fir.raw_statements.insert(loop_idx, decl);
    }

    // Final name for the state counter — see the `PC` doc.
    let declared = declared_names(fir);
    let mut chosen = "pc".to_string();
    let mut n = 0usize;
    while declared.contains(&chosen) {
        n += 1;
        chosen = format!("pc_{n}");
    }
    if chosen != PC {
        let subst: std::collections::HashMap<String, ExprType> = [(
            PC.to_string(),
            ExprType::Path(ExprPath { path_text: chosen.clone(), span }),
        )]
        .into();
        crate::chir_lower::subst_in_stmts(&mut fir.raw_statements, &subst);
        rename_locals(&mut fir.raw_statements, PC, &chosen);
    }
}

/// Every name the module declares — params, generics, file consts, and locals
/// at any depth — i.e. anything the synthesized counter must not collide with.
fn declared_names(fir: &FrontendModuleIR) -> std::collections::HashSet<String> {
    let mut out: std::collections::HashSet<String> = std::collections::HashSet::new();
    for p in &fir.signature.params {
        out.insert(p.name.clone());
    }
    for g in &fir.signature.generics {
        out.insert(g.name.clone());
    }
    for c in &fir.file_consts {
        out.insert(c.name.clone());
    }
    fn walk_stmts(stmts: &[RawStmt], out: &mut std::collections::HashSet<String>) {
        for s in stmts {
            match &s.kind {
                RawStmtKind::Local(l) => {
                    out.insert(l.name.clone());
                    if let Some(init) = &l.init {
                        walk_expr(init, out);
                    }
                }
                RawStmtKind::Expr(es) => walk_expr(&es.expr, out),
                RawStmtKind::Item(_) => {}
            }
        }
    }
    fn walk_expr(e: &ExprType, out: &mut std::collections::HashSet<String>) {
        match e {
            ExprType::Loop(l) => walk_stmts(&l.body, out),
            ExprType::If(f) => {
                walk_stmts(&f.then_block, out);
                if let Some(else_br) = &f.else_branch {
                    walk_expr(else_br, out);
                }
            }
            ExprType::Match(m) => {
                for arm in &m.arms {
                    walk_expr(&arm.body, out);
                }
            }
            ExprType::Block(b) => walk_stmts(&b.stmts, out),
            _ => {}
        }
    }
    walk_stmts(&fir.raw_statements, &mut out);
    out
}

/// Rename `Local` declarations of `from` to `to`, at any statement depth.
fn rename_locals(stmts: &mut [RawStmt], from: &str, to: &str) {
    for s in stmts {
        match &mut s.kind {
            RawStmtKind::Local(l) => {
                if l.name == from {
                    l.name = to.to_string();
                }
            }
            RawStmtKind::Expr(es) => rename_locals_in_expr(&mut es.expr, from, to),
            RawStmtKind::Item(_) => {}
        }
    }
}

fn rename_locals_in_expr(e: &mut ExprType, from: &str, to: &str) {
    match e {
        ExprType::Loop(l) => rename_locals(&mut l.body, from, to),
        ExprType::If(f) => {
            rename_locals(&mut f.then_block, from, to);
            if let Some(else_br) = &mut f.else_branch {
                rename_locals_in_expr(else_br, from, to);
            }
        }
        ExprType::Match(m) => {
            for arm in &mut m.arms {
                rename_locals_in_expr(&mut arm.body, from, to);
            }
        }
        ExprType::Block(b) => rename_locals(&mut b.stmts, from, to),
        _ => {}
    }
}

// ── Zero-time transitions ─────────────────────────────────────────────────────

/// Replace every `__copper_goto = H` with state `H`'s own statements.
///
/// A `continue` re-enters the loop head **without consuming a clock cycle**, and
/// the extracted FSM has no way to say that: its shape is
/// `loop { match pc { … }; clk.tick().await; }`, so reaching another state always
/// costs exactly one tick. The only way to spend no cycle is to be in the same
/// state body — i.e. to inline the target. That is the rule `break` already
/// follows (it inlines the enclosing loop's continuation rather than transitioning
/// to it); this applies it to the back edge.
///
/// **Why it is a separate pass.** `lower_into` cannot do the inlining itself: at
/// the point it lowers a `continue`, the head's body is the thing currently being
/// lowered, further up its own call stack. Every state body exists by the time
/// `lower_into` returns, so the substitution is deferred to here — which is what
/// `TODO` cause O records as the work, and what cause M's entry means by "a second
/// pass that splices state bodies into marked zero-time transitions".
///
/// Runs AFTER `hoist_cross_state_locals`, and the order matters: splicing first
/// would copy the target's `let`s into another state, and a temporary that is
/// local to one state would then look cross-state and be promoted to a
/// flip-flop nobody asked for. Hoisting first leaves each copy self-contained.
///
/// Returns `false` if a goto cycle was found — a chain of zero-time transitions
/// that returns to where it started, i.e. a loop that runs in no time at all.
/// `copper_analysis::check_reachability` rejects that program before codegen sees
/// it, so reaching this is the two analyses disagreeing; the caller DECLINES the
/// module rather than emit anything, and the linear path downstream refuses it.
fn splice_zero_time_gotos(sm: &mut StateMachine) -> bool {
    // Nothing to do for the overwhelmingly common case — no `continue` anywhere.
    if !sm.states.iter().any(|b| stmts_contain_goto(b)) {
        return true;
    }
    let originals = sm.states.clone();
    for i in 0..sm.states.len() {
        let mut body = std::mem::take(&mut sm.states[i]);
        let mut path = vec![i];
        if !splice_in_stmts(&mut body, &originals, &mut path) {
            return false;
        }
        sm.states[i] = body;
    }
    // Defence in depth: a marker that survived would become an assignment to an
    // undeclared name downstream, which is exactly the kind of quiet leak this
    // pass must not produce.
    !sm.states.iter().any(|b| stmts_contain_goto(b))
}

/// `path` is the chain of states already inlined at this point, so a goto back
/// into one of them is a zero-time cycle rather than an infinite expansion.
fn splice_in_stmts(
    stmts: &mut Vec<RawStmt>,
    originals: &[Vec<RawStmt>],
    path: &mut Vec<usize>,
) -> bool {
    let mut out: Vec<RawStmt> = Vec::with_capacity(stmts.len());
    for mut s in std::mem::take(stmts) {
        if let Some(target) = goto_target(&s) {
            if path.contains(&target) || target >= originals.len() {
                return false;
            }
            let mut inlined = originals[target].clone();
            path.push(target);
            let ok = splice_in_stmts(&mut inlined, originals, path);
            path.pop();
            if !ok {
                return false;
            }
            out.extend(inlined);
            // A goto ends its statement list — whatever followed is unreachable.
            *stmts = out;
            return true;
        }
        if !splice_in_stmt_branches(&mut s, originals, path) {
            return false;
        }
        out.push(s);
    }
    *stmts = out;
    true
}

/// Descend into the branch forms a state body can contain. Nested `loop`/`while`/
/// `for` are deliberately NOT visited: control extraction has already flattened
/// every loop it accepted, so a loop surviving in a state body carries no goto.
fn splice_in_stmt_branches(
    s: &mut RawStmt,
    originals: &[Vec<RawStmt>],
    path: &mut Vec<usize>,
) -> bool {
    let RawStmtKind::Expr(es) = &mut s.kind else { return true };
    splice_in_expr(&mut es.expr, originals, path)
}

fn splice_in_expr(e: &mut ExprType, originals: &[Vec<RawStmt>], path: &mut Vec<usize>) -> bool {
    match e {
        ExprType::If(f) => {
            if !splice_in_stmts(&mut f.then_block, originals, path) {
                return false;
            }
            match f.else_branch.as_mut() {
                Some(eb) => splice_in_expr(eb, originals, path),
                None => true,
            }
        }
        ExprType::Block(b) => splice_in_stmts(&mut b.stmts, originals, path),
        ExprType::Match(m) => m
            .arms
            .iter_mut()
            .all(|a| splice_in_expr(&mut a.body, originals, path)),
        _ => true,
    }
}

fn stmts_contain_goto(stmts: &[RawStmt]) -> bool {
    stmts.iter().any(|s| {
        if goto_target(s).is_some() {
            return true;
        }
        match &s.kind {
            RawStmtKind::Expr(es) => expr_contains_goto(&es.expr),
            _ => false,
        }
    })
}

fn expr_contains_goto(e: &ExprType) -> bool {
    match e {
        ExprType::If(f) => {
            stmts_contain_goto(&f.then_block)
                || f.else_branch.as_deref().is_some_and(expr_contains_goto)
        }
        ExprType::Block(b) => stmts_contain_goto(&b.stmts),
        ExprType::Match(m) => m.arms.iter().any(|a| expr_contains_goto(&a.body)),
        _ => false,
    }
}

// ── Cross-state locals ────────────────────────────────────────────────────────

/// Move a local that is DEFINED in one state and READ in another out to a
/// pre-loop `let mut`, leaving a plain assignment where the `let` was.
///
/// Flattening turns straight-line code into `match pc` arms, and an arm scopes
/// its own locals — so `let captured = d.read();` in one arm and
/// `o.write(captured)` in another became "undefined variable 'captured'". The
/// unflattened form of the same source transpiles and makes `captured` a
/// register, which is the language's central rule ("every value live across an
/// await becomes a register"), so the two paths disagreed about the one thing
/// they must not.
///
/// The rewrite produces exactly what a user would write by hand:
///
/// ```text
/// let mut captured = d.read();        // hoisted: gives the type; no init literal
/// loop { match pc { 0 => { captured = d.read(); … } 1 => { … } } }
/// ```
///
/// which lands on the pre-loop register path rather than inventing a new one.
/// A local read only within its own state stays put — hoisting every local would
/// turn state-local temporaries into flip-flops nobody asked for.
fn hoist_cross_state_locals(sm: &mut StateMachine) -> Vec<RawStmt> {
    // (state index, name, init) for every local declared anywhere in a state.
    let mut declared: Vec<(usize, String, ExprType, Option<RawTypeRef>, SourceSpan)> = Vec::new();
    for (i, body) in sm.states.iter().enumerate() {
        collect_locals(body, i, &mut declared);
    }

    let mut hoisted = Vec::new();
    let mut to_convert: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (state, name, init, ty, span) in declared {
        let read_elsewhere = sm
            .states
            .iter()
            .enumerate()
            .any(|(j, body)| j != state && stmts_mention(body, &name));
        if !read_elsewhere || !to_convert.insert(name.clone()) {
            continue;
        }
        hoisted.push(RawStmt {
            order: 0,
            kind: RawStmtKind::Local(LocalStmt {
                is_mut: true,
                // Carry the annotation through: dropping it leaves an init like a
                // bare `0` with no width the hoisted declaration can infer.
                ty,
                name,
                init: Some(init),
                attrs: Vec::new(),
                span,
            }),
            text: String::new(),
            span,
        });
    }

    if !to_convert.is_empty() {
        for body in sm.states.iter_mut() {
            convert_locals_to_assignments(body, &to_convert);
        }
    }
    hoisted
}

fn collect_locals(
    stmts: &[RawStmt],
    state: usize,
    out: &mut Vec<(usize, String, ExprType, Option<RawTypeRef>, SourceSpan)>,
) {
    for s in stmts {
        match &s.kind {
            RawStmtKind::Local(l) => {
                if let Some(init) = &l.init {
                    out.push((state, l.name.clone(), init.clone(), l.ty.clone(), l.span));
                }
            }
            RawStmtKind::Expr(es) => collect_locals_in_expr(&es.expr, state, out),
            RawStmtKind::Item(_) => {}
        }
    }
}

fn collect_locals_in_expr(
    e: &ExprType,
    state: usize,
    out: &mut Vec<(usize, String, ExprType, Option<RawTypeRef>, SourceSpan)>,
) {
    match e {
        ExprType::If(f) => {
            collect_locals(&f.then_block, state, out);
            if let Some(eb) = &f.else_branch {
                collect_locals_in_expr(eb, state, out);
            }
        }
        ExprType::Block(b) => collect_locals(&b.stmts, state, out),
        ExprType::Match(m) => {
            for a in &m.arms {
                collect_locals_in_expr(&a.body, state, out);
            }
        }
        ExprType::Loop(l) => collect_locals(&l.body, state, out),
        ExprType::While(w) => collect_locals(&w.body, state, out),
        ExprType::ForLoop(f) => collect_locals(&f.body, state, out),
        _ => {}
    }
}

/// Replace `let <name> = <init>;` with `<name> = <init>;` for each hoisted name.
fn convert_locals_to_assignments(
    stmts: &mut Vec<RawStmt>,
    names: &std::collections::HashSet<String>,
) {
    for s in stmts.iter_mut() {
        let replacement = match &s.kind {
            RawStmtKind::Local(l) if names.contains(&l.name) => l.init.as_ref().map(|init| {
                expr_stmt(
                    ExprType::Assign(ExprAssign {
                        left: Box::new(path_expr(&l.name, l.span)),
                        right: Box::new(init.clone()),
                        span: l.span,
                    }),
                    l.span,
                )
            }),
            _ => None,
        };
        if let Some(r) = replacement {
            *s = r;
            continue;
        }
        if let RawStmtKind::Expr(es) = &mut s.kind {
            convert_locals_in_expr(&mut es.expr, names);
        }
    }
}

fn convert_locals_in_expr(e: &mut ExprType, names: &std::collections::HashSet<String>) {
    match e {
        ExprType::If(f) => {
            convert_locals_to_assignments(&mut f.then_block, names);
            if let Some(eb) = f.else_branch.as_mut() {
                convert_locals_in_expr(eb, names);
            }
        }
        ExprType::Block(b) => convert_locals_to_assignments(&mut b.stmts, names),
        ExprType::Match(m) => {
            for a in m.arms.iter_mut() {
                convert_locals_in_expr(&mut a.body, names);
            }
        }
        ExprType::Loop(l) => convert_locals_to_assignments(&mut l.body, names),
        ExprType::While(w) => convert_locals_to_assignments(&mut w.body, names),
        ExprType::ForLoop(f) => convert_locals_to_assignments(&mut f.body, names),
        _ => {}
    }
}

/// Whether `name` appears as an identifier anywhere in `stmts`.
fn stmts_mention(stmts: &[RawStmt], name: &str) -> bool {
    stmts.iter().any(|s| match &s.kind {
        RawStmtKind::Local(l) => l.init.as_ref().is_some_and(|i| expr_mentions(i, name)),
        RawStmtKind::Expr(es) => expr_mentions(&es.expr, name),
        RawStmtKind::Item(_) => false,
    })
}

fn expr_mentions(e: &ExprType, name: &str) -> bool {
    match e {
        ExprType::Path(p) => p.path_text.trim() == name,
        ExprType::Lit(_) => false,
        ExprType::Unary(u) => expr_mentions(&u.expr, name),
        ExprType::Binary(b) => expr_mentions(&b.left, name) || expr_mentions(&b.right, name),
        ExprType::Assign(a) => expr_mentions(&a.left, name) || expr_mentions(&a.right, name),
        ExprType::MethodCall(mc) => {
            expr_mentions(&mc.receiver, name) || mc.args.iter().any(|a| expr_mentions(a, name))
        }
        ExprType::Call(c) => c.args.iter().any(|a| expr_mentions(a, name)),
        ExprType::Index(i) => expr_mentions(&i.base, name) || expr_mentions(&i.index, name),
        ExprType::Cast(c) => expr_mentions(&c.expr, name),
        ExprType::Reference(r) => expr_mentions(&r.expr, name),
        ExprType::If(f) => {
            expr_mentions(&f.condition, name)
                || stmts_mention(&f.then_block, name)
                || f.else_branch.as_deref().is_some_and(|e| expr_mentions(e, name))
        }
        ExprType::Block(b) => stmts_mention(&b.stmts, name),
        ExprType::Match(m) => {
            expr_mentions(&m.scrutinee, name)
                || m.arms.iter().any(|a| expr_mentions(&a.body, name))
        }
        ExprType::Loop(l) => stmts_mention(&l.body, name),
        ExprType::While(w) => expr_mentions(&w.condition, name) || stmts_mention(&w.body, name),
        ExprType::ForLoop(f) => expr_mentions(&f.iter, name) || stmts_mention(&f.body, name),
        ExprType::Tuple(t) => t.elements.iter().any(|e| expr_mentions(e, name)),
        ExprType::Await(a) => expr_mentions(&a.base, name),
        _ => false,
    }
}

// ── State machine accumulator ─────────────────────────────────────────────────

/// States are indexed by their `pc` value; `states[0]` is the loop head.
struct StateMachine {
    states: Vec<Vec<RawStmt>>,
}

impl StateMachine {
    fn new() -> Self {
        // Reserve state 0 (the head) up front; its body is set last.
        StateMachine {
            states: vec![Vec::new()],
        }
    }

    fn new_state(&mut self) -> usize {
        self.states.push(Vec::new());
        self.states.len() - 1
    }

    fn set_body(&mut self, id: usize, body: Vec<RawStmt>) {
        self.states[id] = body;
    }
}

/// Where control goes when the current loop body ends or is broken out of.
///
/// `head` is the state to enter on the back edge — state 0 for the module's own
/// loop, or the nested loop's head state. `break_stmts` are the statements
/// following the nested loop, which a `break` continues with *in the same cycle*;
/// `outer` is the context they belong to.
struct LoopCtx<'a> {
    head: usize,
    break_stmts: Vec<RawStmt>,
    outer: Option<&'a LoopCtx<'a>>,
    /// `break_stmts` after lowering, computed once and cloned at every `break`.
    ///
    /// Leaving a loop is not a clock boundary, so a `break` INLINES the enclosing
    /// loop's continuation rather than transitioning to it. Lowering that
    /// continuation afresh per `break` allocated a fresh set of states for every
    /// tick in it — and the desugared counted `for` has two `break`s, each lowered
    /// twice more by the rotation's entry copy. `examples/uart/rx.rs` came out at
    /// 788 states, a count that did not depend on `CLKS_PER_BIT` (434 and 8 both
    /// gave 788), which is what identified it as duplication rather than unrolling.
    ///
    /// Caching the LOWERED form is the same trick the tick-in-branches path
    /// already uses on the body: the copies then share the sub-states their ticks
    /// allocated, so the continuation costs one set of states however many
    /// `break`s reach it. Every `break` from one loop continues with the same
    /// statements in the same context, so sharing is not an approximation — where
    /// control came from is already recorded by which state it came from.
    ///
    /// Lazily filled: a loop whose body never breaks must not allocate states for
    /// a continuation nothing reaches.
    lowered_break: std::cell::RefCell<Option<Vec<RawStmt>>>,
    /// What it MEANS to run off the end of this loop's body without ticking.
    fallthrough: FallThrough,
}

/// Why control reached the end of a body without a tick — and therefore whether
/// returning to the head costs a cycle.
///
/// The two cases look identical in `lower_into` and are a clock cycle apart:
///
/// * `AfterTick` — the body's own trailing tick was REMOVED by the rotation that
///   builds a nested loop's head state (`W ; tick ; C` is lowered as `C ; W`). The
///   fall-through stands for that tick, so `pc = head` is exactly right: the FSM's
///   own trailing tick supplies it.
/// * `ZeroTime` — nothing was removed; the body genuinely ended with statements
///   after its last tick, and the source returns to the head in the SAME cycle.
///   `pc = head` would spend a cycle the program does not have.
///
/// Measured on `loop { for _ in 0..3 { tick } dv.write(One); tick; dv.write(Zero); }`:
/// the simulator repeats every 4 cycles, the FSM every 5, because
/// `dv.write(Zero)` — which belongs to the next iteration's first cycle, since an
/// `Out` holds until rewritten — was given a state of its own.
///
/// It is a property of the CONTEXT rather than the call site, which matters for a
/// `break`: its continuation is lowered in the ENCLOSING context, and whether that
/// continuation's end stands for a removed tick depends on which loop it belongs
/// to, not on where the `break` was written.
#[derive(Clone, Copy, PartialEq)]
enum FallThrough {
    AfterTick,
    ZeroTime,
}

impl LoopCtx<'_> {
    /// The module's own loop: the back edge goes to state 0, and there is nothing
    /// to break out of (a `break` here is refused before extraction runs). Nothing
    /// was rotated away, so running off the end costs no cycle.
    fn module() -> Self {
        LoopCtx {
            head: 0,
            break_stmts: Vec::new(),
            outer: None,
            fallthrough: FallThrough::ZeroTime,
            lowered_break: std::cell::RefCell::new(None),
        }
    }
}

/// Append the FSM lowering of `stmts` into `target`, allocating new states as
/// ticks are crossed. Mirrors the `lower_into` algorithm in CONTROL_EXTRACTION.md.
///
/// Core rule: only a tick advances `pc`; a non-ticking path inlines its
/// continuation in the same cycle (so the continuation after an `if` is
/// *duplicated* into the non-ticking arm — correct, at some state-count cost).
/// `break` is the same rule seen from the other side: it inlines the enclosing
/// loop's continuation, because leaving a loop is not a clock boundary either.
fn lower_into(stmts: &[RawStmt], target: &mut Vec<RawStmt>, sm: &mut StateMachine, ctx: &LoopCtx) {
    for (i, stmt) in stmts.iter().enumerate() {
        if is_tick_stmt(stmt) {
            let rest = &stmts[i + 1..];
            if rest.is_empty() {
                // Trailing tick → back edge of the enclosing loop. No extra empty
                // state (that would burn a cycle — the correctness crux).
                target.push(pc_assign(ctx.head, stmt.span));
            } else {
                let next = sm.new_state();
                target.push(pc_assign(next, stmt.span));
                let mut body = Vec::new();
                lower_into(rest, &mut body, sm, ctx);
                sm.set_body(next, body);
            }
            return; // rest handled after the tick
        }

        if is_continue_stmt(stmt) {
            // Back to the enclosing loop's head, in the SAME cycle. Anything after
            // the `continue` is unreachable. Only a `continue` targeting the
            // MODULE's own loop reaches here — the gate refuses one inside a nested
            // loop, whose head state holds the ROTATED body (`C ; W`) and would
            // re-run the post-tick tail. See `unflattenable_in_expr`.
            target.push(goto_marker(ctx.head, stmt.span));
            return;
        }

        if is_break_stmt(stmt) {
            // Leave the nested loop and carry straight on with what followed it,
            // in the same cycle. Anything after the `break` is unreachable.
            let outer = ctx.outer.expect("break outside a nested loop is refused by the gate");
            // Lower it once per loop, not once per `break` — see `lowered_break`.
            // The borrow is released before recursing, so a continuation that
            // itself breaks out of something cannot deadlock on this cell.
            let cached = ctx.lowered_break.borrow().clone();
            let lowered = match cached {
                Some(l) => l,
                None => {
                    let mut v = Vec::new();
                    lower_into(&ctx.break_stmts, &mut v, sm, outer);
                    *ctx.lowered_break.borrow_mut() = Some(v.clone());
                    v
                }
            };
            target.extend(lowered);
            return;
        }

        if let Some(loop_expr) = as_nested_loop(stmt) {
            let rest = &stmts[i + 1..];
            let head = sm.new_state();
            let inner = LoopCtx {
                head,
                break_stmts: rest.to_vec(),
                outer: Some(ctx),
                // A nested loop's body is lowered with its trailing tick removed
                // (rotated, below), so running off its end stands for that tick.
                fallthrough: FallThrough::AfterTick,
                lowered_break: std::cell::RefCell::new(None),
            };
            let body = &loop_expr.body;
            // A state is "the code between two ticks", so for a body `W ; tick ; C`
            // the repeating unit is `C ; W` — the post-tick tail wrapped round onto
            // the pre-tick prefix. Making the head `W ; tick` instead splits C and W
            // across two states and the loop takes two clock cycles per source
            // iteration (measured: `loop { tick; if go { break } }` tested `go` on
            // every OTHER cycle).
            match body.iter().position(is_tick_stmt) {
                Some(t) => {
                    let mut rotated: Vec<RawStmt> = body[t + 1..].to_vec();
                    rotated.extend_from_slice(&body[..t]);
                    let mut head_body = Vec::new();
                    lower_into(&rotated, &mut head_body, sm, &inner);
                    sm.set_body(head, head_body);
                    // Entering runs the pre-tick prefix in the CURRENT cycle and
                    // lets this state's own tick be the loop's first tick. With an
                    // empty prefix this is just `pc = head`, emitted by the
                    // fall-through inside `lower_into`.
                    lower_into(&body[..t], target, sm, &inner);
                }
                None => {
                    // Ticks only inside branches: no top-level split point, and
                    // every path out of the body diverges (a fall-through with no
                    // tick would be a zero-time loop, which the reachability
                    // guarantee forbids). Cloning the LOWERED body into the entry
                    // shares the sub-states its ticks allocated, so the loop costs
                    // one extra state rather than a doubling.
                    let mut head_body = Vec::new();
                    lower_into(body, &mut head_body, sm, &inner);
                    target.extend(head_body.iter().cloned());
                    sm.set_body(head, head_body);
                }
            }
            return; // everything after the loop is reachable only through `break`
        }

        if let Some(if_expr) = as_if_that_diverges(stmt) {
            let rest = &stmts[i + 1..];

            // Continuation is inlined into both arms.
            let mut then_stmts = if_expr.then_block.clone();
            then_stmts.extend_from_slice(rest);
            let mut then_body = Vec::new();
            lower_into(&then_stmts, &mut then_body, sm, ctx);

            let mut else_stmts = else_branch_stmts(if_expr);
            else_stmts.extend_from_slice(rest);
            let mut else_body = Vec::new();
            lower_into(&else_stmts, &mut else_body, sm, ctx);

            target.push(if_stmt(if_expr, then_body, else_body));
            return; // rest handled inside both arms
        }

        if let Some(match_expr) = as_match_that_diverges(stmt) {
            let rest = &stmts[i + 1..];

            // Same rule as `if`, generalized to N arms: the continuation after the
            // `match` is inlined into *every* arm, and each arm is lowered on its own
            // (a tick in one arm advances `pc`; a tick-free arm inlines the rest in
            // the same cycle). Duplication cost is per-arm, as the `if` case notes.
            let mut new_arms = Vec::with_capacity(match_expr.arms.len());
            for arm in &match_expr.arms {
                let mut arm_stmts = arm_body_stmts(arm);
                arm_stmts.extend_from_slice(rest);
                let mut arm_body = Vec::new();
                lower_into(&arm_stmts, &mut arm_body, sm, ctx);
                new_arms.push(ExprMatchArm {
                    pattern_text: arm.pattern_text.clone(),
                    guard: arm.guard.clone(),
                    body: Box::new(block_expr(arm_body, arm.span)),
                    span: arm.span,
                });
            }
            target.push(match_stmt(match_expr, new_arms));
            return; // rest handled inside every arm
        }

        // Plain combinational statement (incl. `if`/`match` with no tick).
        target.push(stmt.clone());
    }

    // Fell through without ticking. Whether that costs a cycle depends on why —
    // see `FallThrough`.
    let span = stmts.last().map(|s| s.span).unwrap_or_default();
    target.push(match ctx.fallthrough {
        FallThrough::AfterTick => pc_assign(ctx.head, span),
        FallThrough::ZeroTime => goto_marker(ctx.head, span),
    });
}

// ── Node constructors ─────────────────────────────────────────────────────────

fn block_expr(stmts: Vec<RawStmt>, span: SourceSpan) -> ExprType {
    ExprType::Block(ExprBlock { stmts, span })
}

fn path_expr(name: &str, span: SourceSpan) -> ExprType {
    ExprType::Path(ExprPath {
        path_text: name.to_string(),
        span,
    })
}

fn expr_stmt(expr: ExprType, span: SourceSpan) -> RawStmt {
    RawStmt {
        order: 0,
        kind: RawStmtKind::Expr(ExprStmt {
            expr,
            has_semi: true,
            span,
        }),
        text: String::new(),
        span,
    }
}

/// `__copper_goto = <n>;` — a zero-time transition. See [`GOTO`].
fn goto_marker(n: usize, span: SourceSpan) -> RawStmt {
    expr_stmt(
        ExprType::Assign(ExprAssign {
            left: Box::new(path_expr(GOTO, span)),
            right: Box::new(ExprType::Lit(ExprLit { text: n.to_string(), span })),
            span,
        }),
        span,
    )
}

/// The state a zero-time goto targets, if `s` is one.
fn goto_target(s: &RawStmt) -> Option<usize> {
    let RawStmtKind::Expr(es) = &s.kind else { return None };
    let ExprType::Assign(a) = &es.expr else { return None };
    let ExprType::Path(p) = &*a.left else { return None };
    if p.path_text != GOTO {
        return None;
    }
    let ExprType::Lit(l) = &*a.right else { return None };
    l.text.parse::<usize>().ok()
}

/// `pc = <n>;`
fn pc_assign(n: usize, span: SourceSpan) -> RawStmt {
    expr_stmt(
        ExprType::Assign(ExprAssign {
            left: Box::new(path_expr(PC, span)),
            right: Box::new(ExprType::Lit(ExprLit {
                text: n.to_string(),
                span,
            })),
            span,
        }),
        span,
    )
}

/// `if <cond> { <then_body> } else { <else_body> }`, reusing the source condition.
fn if_stmt(src: &ExprIf, then_body: Vec<RawStmt>, else_body: Vec<RawStmt>) -> RawStmt {
    let span = src.span;
    expr_stmt(
        ExprType::If(ExprIf {
            condition: src.condition.clone(),
            then_block: then_body,
            else_branch: Some(Box::new(block_expr(else_body, span))),
            span,
        }),
        span,
    )
}

/// The statements of an `if`'s else branch. `else if` / other else forms are
/// wrapped back into a statement and left for `lower_into` to re-descend.
fn else_branch_stmts(if_expr: &ExprIf) -> Vec<RawStmt> {
    match &if_expr.else_branch {
        None => Vec::new(),
        Some(b) => match b.as_ref() {
            ExprType::Block(blk) => blk.stmts.clone(),
            other => vec![expr_stmt(other.clone(), if_expr.span)],
        },
    }
}

/// `match <scrutinee> { <arms> }`, reusing the source scrutinee.
fn match_stmt(src: &ExprMatch, arms: Vec<ExprMatchArm>) -> RawStmt {
    expr_stmt(
        ExprType::Match(ExprMatch {
            scrutinee: src.scrutinee.clone(),
            arms,
            span: src.span,
        }),
        src.span,
    )
}

/// The statements of a `match` arm body. A non-block arm body (`Pat => expr`) is
/// wrapped back into a single statement for `lower_into` to re-descend.
fn arm_body_stmts(arm: &ExprMatchArm) -> Vec<RawStmt> {
    match arm.body.as_ref() {
        ExprType::Block(blk) => blk.stmts.clone(),
        other => vec![expr_stmt(other.clone(), arm.span)],
    }
}

// ── Detection ─────────────────────────────────────────────────────────────────

fn is_loop_stmt(s: &RawStmt) -> bool {
    matches!(&s.kind, RawStmtKind::Expr(es) if matches!(es.expr, ExprType::Loop(_)))
}

fn is_tick_await(base: &ExprType) -> bool {
    matches!(base, ExprType::MethodCall(mc) if mc.method == "tick" && mc.args.is_empty())
}

fn is_tick_stmt(s: &RawStmt) -> bool {
    matches!(&s.kind, RawStmtKind::Expr(es)
        if matches!(&es.expr, ExprType::Await(a) if is_tick_await(&a.base)))
}

fn is_continue_stmt(s: &RawStmt) -> bool {
    matches!(&s.kind, RawStmtKind::Expr(es) if matches!(es.expr, ExprType::Continue(_)))
}

fn is_break_stmt(s: &RawStmt) -> bool {
    matches!(&s.kind, RawStmtKind::Expr(es) if matches!(es.expr, ExprType::Break(_)))
}

/// `Some(loop_expr)` when `s` is a nested `loop { … }` statement.
fn as_nested_loop(s: &RawStmt) -> Option<&ExprLoop> {
    match &s.kind {
        RawStmtKind::Expr(es) => match &es.expr {
            ExprType::Loop(l) => Some(l),
            _ => None,
        },
        _ => None,
    }
}

/// Can control leave this expression other than by falling off its end?
///
/// Three ways: a tick (the next cycle starts elsewhere), a `break` (the
/// continuation is somewhere else entirely), or a `continue` (the loop head is,
/// in the same cycle). Each means the branches must be lowered separately with the
/// continuation inlined into each — which is what makes `if ready { break; }`
/// split into two `pc` futures rather than staying a plain combinational statement.
///
/// `continue` belongs here for exactly the same reason `break` does, and leaving
/// it out was silent: `if abort { continue; }` contains no tick and no break, so it
/// was pushed through as an ordinary statement and the `continue` inside it was
/// never lowered at all.
fn expr_diverges(e: &ExprType) -> bool {
    expr_contains_tick(e) || expr_breaks_enclosing_loop(e) || expr_continues_enclosing_loop(e)
}

fn stmts_break_enclosing_loop(stmts: &[RawStmt]) -> bool {
    stmts.iter().any(|s| match &s.kind {
        RawStmtKind::Expr(es) => expr_breaks_enclosing_loop(&es.expr),
        RawStmtKind::Local(l) => l.init.as_ref().is_some_and(expr_breaks_enclosing_loop),
        RawStmtKind::Item(_) => false,
    })
}

fn expr_breaks_enclosing_loop(e: &ExprType) -> bool {
    match e {
        ExprType::Break(_) => true,
        ExprType::If(f) => {
            stmts_break_enclosing_loop(&f.then_block)
                || f.else_branch.as_deref().is_some_and(expr_breaks_enclosing_loop)
        }
        ExprType::Block(b) => stmts_break_enclosing_loop(&b.stmts),
        ExprType::Match(m) => m.arms.iter().any(|a| expr_breaks_enclosing_loop(&a.body)),
        // A `break` under a loop of its own belongs to THAT loop, not this one.
        ExprType::Loop(_) | ExprType::While(_) | ExprType::ForLoop(_) => false,
        _ => false,
    }
}

/// `Some(if_expr)` when `s` is an `if` control can leave — see `expr_diverges`.
fn as_if_that_diverges(s: &RawStmt) -> Option<&ExprIf> {
    if let RawStmtKind::Expr(es) = &s.kind {
        if let ExprType::If(if_expr) = &es.expr {
            if expr_diverges(&es.expr) {
                return Some(if_expr);
            }
        }
    }
    None
}

/// `Some(match_expr)` when `s` is a `match` control can leave through some arm.
fn as_match_that_diverges(s: &RawStmt) -> Option<&ExprMatch> {
    if let RawStmtKind::Expr(es) = &s.kind {
        if let ExprType::Match(match_expr) = &es.expr {
            if expr_diverges(&es.expr) {
                return Some(match_expr);
            }
        }
    }
    None
}

/// Fire extraction when a tick is nested inside a branch — i.e. some top-level
/// statement contains a tick but is not itself the tick — or when the body
/// `continue`s.
///
/// A `continue` needs a `pc` even in an otherwise linear body: it re-enters the
/// loop head from the middle, which the phase FSM has no way to express. Without
/// this clause `loop { tick; if c { continue; } … tick; }` fell through to the
/// linear path and was refused for a construct extraction can now handle.
fn loop_needs_extraction(body: &[RawStmt]) -> bool {
    body.iter()
        .any(|s| stmt_contains_tick(s) && !is_tick_stmt(s))
        || stmts_continue_enclosing_loop(body)
}

/// Is there a `continue` bound to THIS loop? A `continue` under a nested loop of
/// its own belongs to that loop — mirrors `stmts_break_enclosing_loop`.
fn stmts_continue_enclosing_loop(stmts: &[RawStmt]) -> bool {
    stmts.iter().any(|s| match &s.kind {
        RawStmtKind::Expr(es) => expr_continues_enclosing_loop(&es.expr),
        RawStmtKind::Local(_) | RawStmtKind::Item(_) => false,
    })
}

fn expr_continues_enclosing_loop(e: &ExprType) -> bool {
    match e {
        ExprType::Continue(_) => true,
        ExprType::If(f) => {
            stmts_continue_enclosing_loop(&f.then_block)
                || f.else_branch.as_deref().is_some_and(expr_continues_enclosing_loop)
        }
        ExprType::Block(b) => stmts_continue_enclosing_loop(&b.stmts),
        ExprType::Match(m) => m.arms.iter().any(|a| expr_continues_enclosing_loop(&a.body)),
        ExprType::Loop(_) | ExprType::While(_) | ExprType::ForLoop(_) => false,
        _ => false,
    }
}

/// Why this pass declined a module, when the construct at fault is one the linear
/// path downstream cannot name.
///
/// Declining is deliberate — the linear lowering usually produces a better message
/// than this pass could. But it only sees what it *reaches*, so a construct that
/// stopped the flattening somewhere else in the body leaves it blaming whatever it
/// meets first. Measured on `examples/uart/rx.rs`: a `continue` on line 62 declined
/// the module, and the linear path then reported the perfectly well-formed
/// repeating wait on line 55 for having its tick in the wrong place — advice that
/// cannot be followed, about a loop the author wrote correctly.
pub struct UnflattenableConstruct {
    pub construct: String,
    pub span: SourceSpan,
    pub hint: Option<String>,
}

impl std::fmt::Display for UnflattenableConstruct {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: unsupported construct: {}", self.span.start_line, self.span.start_col, self.construct)?;
        if let Some(h) = &self.hint {
            write!(f, "\n  help: {h}")?;
        }
        Ok(())
    }
}

/// The named construct that stopped this module from being flattened, if there is
/// one. `None` both when the module flattens and when it was declined for a
/// malformed nested loop — `chir_lower::nested_loop_error` diagnoses that case
/// better than anything here could, so it is left to speak.
pub fn unflattenable_reason(fir: &FrontendModuleIR) -> Option<UnflattenableConstruct> {
    let loop_idx = fir.raw_statements.iter().position(is_loop_stmt)?;
    let body = match &fir.raw_statements[loop_idx].kind {
        RawStmtKind::Expr(es) => match &es.expr {
            ExprType::Loop(l) => &l.body,
            _ => return None,
        },
        _ => return None,
    };
    match contains_unflattenable_control(body) {
        Some(Unflattenable::Named(c)) => Some(c),
        _ => None,
    }
}

/// What `contains_unflattenable_control` found.
enum Unflattenable {
    /// A construct the linear path downstream cannot name — report THIS instead.
    Named(UnflattenableConstruct),
    /// A nested loop whose body is not one segment between two ticks.
    /// `chir_lower::nested_loop_error` splits this three ways and points at the
    /// innermost loop at fault; duplicating that reasoning here is exactly the
    /// two-halves-drift bug this file keeps recording, so it is not duplicated.
    MalformedNestedLoop,
}

fn named(construct: &str, span: SourceSpan, hint: &str) -> Option<Unflattenable> {
    Some(Unflattenable::Named(UnflattenableConstruct {
        construct: construct.to_string(),
        span,
        hint: Some(hint.to_string()),
    }))
}

/// Is there control flow in here this pass cannot flatten?
///
/// It handles straight-line code, `if`/`else`, `match`, and nested `loop`s left by
/// `break`. What it refuses, and why each is refused rather than approximated:
///
/// * a tick inside `while` / `for` — a counted or conditional repetition whose
///   trip count is not a state; `chir_lower` rejects both constructs outright and
///   its message names them, so declining here yields the better diagnostic;
/// * a tick inside a `let` initializer — not a statement position, so it has no
///   state to become;
/// * `continue` — jumping to the loop head mid-cycle needs the head's *lowered*
///   body at a point where it does not exist yet (it is still being lowered);
/// * a labelled `break`, or `break <value>` — the first targets a loop this pass
///   does not track, the second carries a value the FSM has nowhere to put;
/// * a `break` outside any nested loop — meaningless against the module's own
///   infinite loop, and `chir_lower` names it.
fn contains_unflattenable_control(stmts: &[RawStmt]) -> Option<Unflattenable> {
    unflattenable_in_stmts(stmts, false)
}

fn unflattenable_in_stmts(stmts: &[RawStmt], in_loop: bool) -> Option<Unflattenable> {
    stmts.iter().find_map(|s| match &s.kind {
        RawStmtKind::Expr(es) => unflattenable_in_expr(&es.expr, in_loop),
        // A tick inside an initializer expression has no statement position to
        // become a state.
        RawStmtKind::Local(l) if l.init.as_ref().is_some_and(expr_contains_tick) => named(
            "a `clk.tick().await` inside a `let` initializer — an initializer is not \
             a statement position, so there is no FSM state for the code either side \
             of the tick to become",
            s.span,
            "await into a local first: `let v = <expr>; clk.tick().await;`",
        ),
        _ => None,
    })
}

/// A nested loop body must be exactly one "code between two ticks" segment: it has
/// to END at a clock boundary. Two spellings do that:
///
/// * `<test> ; clk.tick().await` — the boundary is the body's own last statement;
/// * `<test> ; loop { … }` — the body's last statement is **another tick-bearing
///   loop**, which is where this segment's boundary comes from. The enclosing
///   loop's back edge is then taken when the inner loop *breaks*, not at a tick of
///   its own. `lower_into` already models exactly that (a `break` inlines the
///   enclosing loop's continuation in the same cycle), so admitting the shape is a
///   gate change rather than a new lowering.
///
/// The second clause is what `TODO` records as cause K, and it is worth knowing
/// what it does *not* unblock on its own. Because the tick must come last, every
/// hand-written nested `loop` can be left before it ticks — so an enclosing loop
/// whose only boundary is one of them cycles in zero time, which
/// `copper_analysis::check_reachability` now rejects (it used to accept it, and
/// the simulator livelocked while this pass' FSM ran a cycle per iteration). The
/// shapes that survive both are the ones whose inner loop always ticks: a counted
/// `for`, which is where the UART's `for _ in 0..CLKS_PER_BIT { … }` will land.
///
/// The other ordering — `loop { tick; <test> }` — is **outside the language, by
/// decision** (2026-08-24), not merely unimplemented. It puts the test in the
/// window after the entering edge, where a simulator samples the value the
/// just-past edge produced and a flip-flop samples the value present before its
/// own edge. Measured on `loop { clk.tick().await; if go { break; } }`: the
/// transpiled module reacted a full cycle earlier than the simulator, and holding
/// the stimulus for two cycles did NOT reconcile them — the two models read in
/// different windows, not at different points of one.
///
/// The restriction is cheap because the supported ordering expresses the same
/// designs: `loop { <test>; clk.tick().await; }` is what one would write anyway.
/// It is the same disposition as the pre-tick alignment hazard (D1) — the
/// divergent program is made unwritable rather than adjudicated. See the `TODO`
/// and `design_docs/SYNCHRONOUS_SEMANTICS.md`.
///
/// Note the divergence needs an input that changes mid-cycle. An `In` driven by a
/// clocked module in the same domain is stable across the window, so both models
/// read the same value — it is a testbench-observable difference, which is
/// precisely why it cannot be left in: sim ≡ SV under a testbench is the bar.
fn body_ends_at_a_clock_boundary(body: &[RawStmt]) -> bool {
    match body.iter().position(is_tick_stmt) {
        Some(t) => t + 1 == body.len(),
        // No tick of its own — the segment may still end by entering another
        // tick-bearing loop as its last statement. Recursing (rather than merely
        // asking whether that loop contains a tick) keeps this predicate
        // self-sufficient: it can only ever be *stricter* than the statement walk
        // that also visits the inner loop, never looser, so the two halves of the
        // gate cannot drift into disagreeing about what is admissible.
        //
        // Still declined here: ticks that live only inside `if`/`match` arms of the
        // body (`loop { if c { tick; } else { tick; } }`). There is no single last
        // statement that is the boundary, and no hardware-anchored check that the
        // shape lowers correctly.
        None => body
            .last()
            .and_then(as_nested_loop)
            .is_some_and(|inner| body_ends_at_a_clock_boundary(&inner.body)),
    }
}

/// `in_loop` tracks whether a nested `loop` encloses this expression, which is
/// what makes a `break` meaningful.
fn unflattenable_in_expr(e: &ExprType, in_loop: bool) -> Option<Unflattenable> {
    match e {
        // A `continue` targeting the MODULE's own loop is supported: it becomes a
        // zero-time goto to state 0, spliced after every state body exists. Inside
        // a NESTED loop it is not, and the reason is the rotation: that loop's head
        // state holds `C ; W` (the post-tick tail wrapped onto the pre-tick prefix),
        // so re-entering it would run `C` — code the source places AFTER the tick
        // this `continue` never reaches. The right target is the start of `W`, which
        // is not a state; it is inlined into whatever precedes the loop.
        ExprType::Continue(_) if in_loop => named(
            "`continue` inside a NESTED loop — the loop's head state holds its body \
             ROTATED around the tick, so re-entering it would re-run the statements \
             that follow the tick, which this `continue` never reached",
            e.span(),
            "restructure so the path falls through to the loop's end instead, e.g. \
             `if ok { <work> }` rather than `if !ok { continue; } <work>`",
        ),
        ExprType::Continue(_) => None,
        ExprType::Break(b) if !in_loop => named(
            "a `break` outside any nested loop — there is nothing to leave but the \
             module's own infinite loop",
            b.span,
            "drop the `break`, or put it inside the nested loop it is meant to exit",
        ),
        ExprType::Break(b) if b.label.is_some() => named(
            "a labelled `break` — it targets a loop this pass does not track",
            b.span,
            "restructure so the break leaves its own innermost loop",
        ),
        ExprType::Break(b) if b.expr.is_some() => named(
            "`break <value>` — the FSM has nowhere to put the value",
            b.span,
            "assign to a local before breaking: `v = <value>; break;`",
        ),
        ExprType::Break(_) => None,
        ExprType::Loop(l) => {
            if !body_ends_at_a_clock_boundary(&l.body) {
                Some(Unflattenable::MalformedNestedLoop)
            } else {
                unflattenable_in_stmts(&l.body, true)
            }
        }
        // A tick under a construct whose repetition is not a state. Both are
        // desugared before this gate runs, so reaching here means the desugar
        // declined the header — an inclusive or non-range iterator, or a body whose
        // last statement is not the tick.
        ExprType::While(w) if stmts_contain_tick(&w.body) => Some(Unflattenable::MalformedNestedLoop),
        ExprType::ForLoop(f) if stmts_contain_tick(&f.body) => named(
            "a `clk.tick().await` inside a `for` whose repetition cannot be counted — \
             the header must be an exclusive `start..end` range and the tick must be \
             the LAST statement of the body",
            f.span,
            "write it as `for <var> in <start>..<end> { <work>; clk.tick().await; }`",
        ),
        // Structures the flattener descends into — keep looking, same loop depth.
        ExprType::If(f) => unflattenable_in_stmts(&f.then_block, in_loop).or_else(|| {
            f.else_branch
                .as_deref()
                .and_then(|e| unflattenable_in_expr(e, in_loop))
        }),
        ExprType::Block(b) => unflattenable_in_stmts(&b.stmts, in_loop),
        ExprType::Match(m) => m.arms.iter().find_map(|a| unflattenable_in_expr(&a.body, in_loop)),
        _ => None,
    }
}

/// The first `clk.tick().await` statement anywhere in `stmts`, cloned.
fn find_tick_stmt(stmts: &[RawStmt]) -> Option<RawStmt> {
    for s in stmts {
        if is_tick_stmt(s) {
            return Some(s.clone());
        }
        if let RawStmtKind::Expr(es) = &s.kind {
            if let Some(t) = find_tick_in_expr(&es.expr) {
                return Some(t);
            }
        }
    }
    None
}

fn find_tick_in_expr(e: &ExprType) -> Option<RawStmt> {
    match e {
        ExprType::If(f) => find_tick_stmt(&f.then_block)
            .or_else(|| f.else_branch.as_deref().and_then(find_tick_in_expr)),
        ExprType::Block(b) => find_tick_stmt(&b.stmts),
        ExprType::Match(m) => m.arms.iter().find_map(|a| find_tick_in_expr(&a.body)),
        // Nested loops must be searched too. This walk and the gate have to agree
        // about where a tick can live — when they drift, `extract_control` silently
        // declines a module the gate admitted and the linear path downstream reports
        // some unrelated construct. Omitting `Loop` here went unnoticed for as long
        // as every flattenable module also had a tick at the top level of its own
        // loop; a body whose ONLY tick is inside a nested loop (cause K) is the first
        // shape where that stopped being true.
        ExprType::Loop(l) => find_tick_stmt(&l.body),
        ExprType::While(w) => find_tick_stmt(&w.body),
        ExprType::ForLoop(f) => find_tick_stmt(&f.body),
        _ => None,
    }
}

fn stmts_contain_tick(stmts: &[RawStmt]) -> bool {
    stmts.iter().any(stmt_contains_tick)
}

fn stmt_contains_tick(s: &RawStmt) -> bool {
    match &s.kind {
        RawStmtKind::Expr(es) => expr_contains_tick(&es.expr),
        RawStmtKind::Local(l) => l.init.as_ref().is_some_and(expr_contains_tick),
        RawStmtKind::Item(_) => false,
    }
}

fn expr_contains_tick(e: &ExprType) -> bool {
    match e {
        ExprType::Await(a) => is_tick_await(&a.base),
        ExprType::If(f) => {
            stmts_contain_tick(&f.then_block)
                || f.else_branch.as_deref().is_some_and(expr_contains_tick)
        }
        ExprType::Block(b) => stmts_contain_tick(&b.stmts),
        ExprType::Match(m) => m.arms.iter().any(|a| expr_contains_tick(&a.body)),
        ExprType::Loop(l) => stmts_contain_tick(&l.body),
        ExprType::While(w) => stmts_contain_tick(&w.body),
        ExprType::ForLoop(f) => stmts_contain_tick(&f.body),
        _ => false,
    }
}
