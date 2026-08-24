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
    ExprAssign, ExprBlock, ExprIf, ExprLit, ExprLoop, ExprMatch, ExprMatchArm, ExprPath, ExprStmt,
    ExprType, FrontendModuleIR, LocalStmt, RawStmt, RawStmtKind, RawTypeRef, SourceSpan,
};

/// The synthesized program-counter register name.
const PC: &str = "pc";

/// The `pc` register's Rust type. `match`-arm literals carry the matching suffix
/// so they lower to the same width as `pc` (else Verilator flags `WIDTHEXPAND` on
/// the `case` items, e.g. `64'd0` vs an 8-bit scrutinee).
const PC_TY: &str = "u8";
const PC_SUFFIX: &str = "u8";

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
    if contains_unflattenable_control(&loop_body) {
        return;
    }

    let span = fir.raw_statements[loop_idx].span;

    // Flatten the CFG into per-state segment bodies (state 0 = loop head).
    let mut sm = StateMachine::new();
    let mut state0 = Vec::new();
    lower_into(&loop_body, &mut state0, &mut sm, &LoopCtx::module());
    sm.set_body(0, state0);

    // Build `match pc { 0 => {..}, .., _ => {} }`.
    let mut arms = Vec::new();
    for (i, body) in sm.states.iter().enumerate() {
        arms.push(ExprMatchArm {
            pattern_text: format!("{i}{PC_SUFFIX}"),
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
                ty_text: PC_TY.to_string(),
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
}

impl LoopCtx<'_> {
    /// The module's own loop: the back edge goes to state 0, and there is nothing
    /// to break out of (a `break` here is refused before extraction runs).
    fn module() -> Self {
        LoopCtx { head: 0, break_stmts: Vec::new(), outer: None }
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

        if is_break_stmt(stmt) {
            // Leave the nested loop and carry straight on with what followed it,
            // in the same cycle. Anything after the `break` is unreachable.
            let outer = ctx.outer.expect("break outside a nested loop is refused by the gate");
            let cont = ctx.break_stmts.clone();
            lower_into(&cont, target, sm, outer);
            return;
        }

        if let Some(loop_expr) = as_nested_loop(stmt) {
            let rest = &stmts[i + 1..];
            let head = sm.new_state();
            let inner = LoopCtx {
                head,
                break_stmts: rest.to_vec(),
                outer: Some(ctx),
            };
            // The gate guarantees the body's tick is its LAST statement, so the
            // body is exactly "the code between two ticks" — one state. Lowering
            // it with `head` as the back-edge target makes its trailing tick emit
            // `pc = head`, i.e. stay here another cycle.
            let mut head_body = Vec::new();
            lower_into(&loop_expr.body, &mut head_body, sm, &inner);
            // Entering runs the first iteration in the CURRENT cycle, so the entry
            // gets the lowered body itself rather than a jump to `head` — a jump
            // would burn a cycle the source never asked for. Cloning the LOWERED
            // form shares the sub-states its ticks allocated, so a loop costs one
            // extra state, not a doubling.
            target.extend(head_body.iter().cloned());
            sm.set_body(head, head_body);
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

    // Fell through without ticking → take the back edge next cycle.
    let span = stmts.last().map(|s| s.span).unwrap_or_default();
    target.push(pc_assign(ctx.head, span));
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
/// Either by a tick (the next cycle starts elsewhere) or by breaking the enclosing
/// loop (the continuation is somewhere else entirely). Both mean the branches must
/// be lowered separately with the continuation inlined into each — which is what
/// makes `if ready { break; }` split into two `pc` futures rather than staying a
/// plain combinational statement.
fn expr_diverges(e: &ExprType) -> bool {
    expr_contains_tick(e) || expr_breaks_enclosing_loop(e)
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

/// Fire extraction only when a tick is nested inside a branch — i.e. some
/// top-level statement contains a tick but is not itself the tick.
fn loop_needs_extraction(body: &[RawStmt]) -> bool {
    body.iter()
        .any(|s| stmt_contains_tick(s) && !is_tick_stmt(s))
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
fn contains_unflattenable_control(stmts: &[RawStmt]) -> bool {
    stmts.iter().any(|s| match &s.kind {
        RawStmtKind::Expr(es) => unflattenable_in_expr(&es.expr, false),
        // A tick inside an initializer expression has no statement position to
        // become a state.
        RawStmtKind::Local(l) => l.init.as_ref().is_some_and(expr_contains_tick),
        RawStmtKind::Item(_) => false,
    })
}

/// A nested loop body must be exactly one "code between two ticks" segment: its
/// tick has to be the LAST statement, so the body is `<test> ; tick`.
///
/// The other ordering — `loop { tick; <test> }` — puts the test in the window
/// AFTER the entering edge, and that window is where the simulator and a
/// testbench disagree about which input value is current: the simulator's
/// post-edge settle still holds the value driven for the cycle whose edge just
/// passed, while the emitted FSM's state reads the value driven for the cycle
/// about to end. Measured on `loop { clk.tick().await; if go { break; } }`: the
/// transpiled module reacted a full cycle earlier than the simulator. That is the
/// mid-phase-read seam, not a flattening question, so this pass declines rather
/// than picking a side. See the `TODO`.
fn tick_is_last_statement(body: &[RawStmt]) -> bool {
    match body.iter().position(is_tick_stmt) {
        // Ticks only inside branches: no single "between two ticks" segment, and
        // no verified lowering. Declined for the same reason.
        None => false,
        Some(t) => t + 1 == body.len(),
    }
}

/// `in_loop` tracks whether a nested `loop` encloses this expression, which is
/// what makes a `break` meaningful.
fn unflattenable_in_expr(e: &ExprType, in_loop: bool) -> bool {
    match e {
        ExprType::Continue(_) => true,
        ExprType::Break(b) => !in_loop || b.label.is_some() || b.expr.is_some(),
        ExprType::Loop(l) => {
            !tick_is_last_statement(&l.body)
                || l.body.iter().any(|s| match &s.kind {
                    RawStmtKind::Expr(es) => unflattenable_in_expr(&es.expr, true),
                    RawStmtKind::Local(loc) => loc.init.as_ref().is_some_and(expr_contains_tick),
                    RawStmtKind::Item(_) => false,
                })
        }
        // A tick under a construct whose repetition is not a state.
        ExprType::While(w) => stmts_contain_tick(&w.body),
        ExprType::ForLoop(f) => stmts_contain_tick(&f.body),
        // Structures the flattener descends into — keep looking, same loop depth.
        ExprType::If(f) => {
            f.then_block.iter().any(|s| match &s.kind {
                RawStmtKind::Expr(es) => unflattenable_in_expr(&es.expr, in_loop),
                RawStmtKind::Local(l) => l.init.as_ref().is_some_and(expr_contains_tick),
                RawStmtKind::Item(_) => false,
            }) || f
                .else_branch
                .as_deref()
                .is_some_and(|e| unflattenable_in_expr(e, in_loop))
        }
        ExprType::Block(b) => b.stmts.iter().any(|s| match &s.kind {
            RawStmtKind::Expr(es) => unflattenable_in_expr(&es.expr, in_loop),
            RawStmtKind::Local(l) => l.init.as_ref().is_some_and(expr_contains_tick),
            RawStmtKind::Item(_) => false,
        }),
        ExprType::Match(m) => m.arms.iter().any(|a| unflattenable_in_expr(&a.body, in_loop)),
        _ => false,
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
