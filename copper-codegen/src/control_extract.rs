//! Control extraction — increment A: straight-line + `if`/`else` branching.
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
//! when a tick is nested inside a branch — linear (top-level-tick) modules keep
//! the proven lowering path untouched. See `design_docs/CONTROL_EXTRACTION.md`.

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
    // A tick inside a nested `loop` / `while` / `for` is a repeating wait, which
    // needs a self-looping state and `break`/`continue` handling — increment B,
    // not built. DECLINE rather than transform: the linear path downstream then
    // reports the offending construct with its own span, which is a far better
    // diagnostic than anything this pass could produce, and it is what the
    // simulator-only modules using that idiom (`examples/cpu/rv32i_cpu.rs`) get.
    //
    // This gate exists because the two halves of this pass disagreed:
    // `expr_contains_tick` descends into loops but `find_tick_in_expr` does not,
    // so such a module passed the gate and then hit an `.expect` — a raw panic on
    // user input, with no span and no name for the construct at fault.
    if contains_unflattenable_tick(&loop_body) {
        return;
    }

    let span = fir.raw_statements[loop_idx].span;

    // Flatten the CFG into per-state segment bodies (state 0 = loop head).
    let mut sm = StateMachine::new();
    let mut state0 = Vec::new();
    lower_into(&loop_body, &mut state0, &mut sm);
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

/// Append the FSM lowering of `stmts` into `target`, allocating new states as
/// ticks are crossed. Mirrors the `lower_into` algorithm in CONTROL_EXTRACTION.md.
///
/// Core rule: only a tick advances `pc`; a non-ticking path inlines its
/// continuation in the same cycle (so the continuation after an `if` is
/// *duplicated* into the non-ticking arm — correct, at some state-count cost).
fn lower_into(stmts: &[RawStmt], target: &mut Vec<RawStmt>, sm: &mut StateMachine) {
    for (i, stmt) in stmts.iter().enumerate() {
        if is_tick_stmt(stmt) {
            let rest = &stmts[i + 1..];
            if rest.is_empty() {
                // Trailing tick → loop back to the head. No extra empty state
                // (that would burn a cycle — the correctness crux).
                target.push(pc_assign(0, stmt.span));
            } else {
                let next = sm.new_state();
                target.push(pc_assign(next, stmt.span));
                let mut body = Vec::new();
                lower_into(rest, &mut body, sm);
                sm.set_body(next, body);
            }
            return; // rest handled after the tick
        }

        if let Some(if_expr) = as_if_with_tick(stmt) {
            let rest = &stmts[i + 1..];

            // Continuation is inlined into both arms.
            let mut then_stmts = if_expr.then_block.clone();
            then_stmts.extend_from_slice(rest);
            let mut then_body = Vec::new();
            lower_into(&then_stmts, &mut then_body, sm);

            let mut else_stmts = else_branch_stmts(if_expr);
            else_stmts.extend_from_slice(rest);
            let mut else_body = Vec::new();
            lower_into(&else_stmts, &mut else_body, sm);

            target.push(if_stmt(if_expr, then_body, else_body));
            return; // rest handled inside both arms
        }

        if let Some(match_expr) = as_match_with_tick(stmt) {
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
                lower_into(&arm_stmts, &mut arm_body, sm);
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

    // Fell through without ticking → continue to the head next cycle.
    let span = stmts.last().map(|s| s.span).unwrap_or_default();
    target.push(pc_assign(0, span));
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

/// `Some(if_expr)` when `s` is an `if` whose body (any depth) issues a tick.
fn as_if_with_tick(s: &RawStmt) -> Option<&ExprIf> {
    if let RawStmtKind::Expr(es) = &s.kind {
        if let ExprType::If(if_expr) = &es.expr {
            if expr_contains_tick(&es.expr) {
                return Some(if_expr);
            }
        }
    }
    None
}

/// `Some(match_expr)` when `s` is a `match` some arm of which (any depth) ticks.
fn as_match_with_tick(s: &RawStmt) -> Option<&ExprMatch> {
    if let RawStmtKind::Expr(es) = &s.kind {
        if let ExprType::Match(match_expr) = &es.expr {
            if expr_contains_tick(&es.expr) {
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

/// Is there a `clk.tick().await` somewhere this pass cannot flatten?
///
/// The flattener handles straight-line code, `if`/`else` and `match` — it inlines
/// each continuation into the branches and gives every tick its own state. A tick
/// inside a nested `loop` / `while` / `for` is a *repeating* wait: flattening it
/// needs a state that loops back to itself and an understanding of `break` /
/// `continue`, neither of which this pass models. A tick inside a `let`
/// initializer is not a statement position at all.
fn contains_unflattenable_tick(stmts: &[RawStmt]) -> bool {
    stmts.iter().any(|s| match &s.kind {
        RawStmtKind::Expr(es) => unflattenable_tick_in_expr(&es.expr),
        // A tick inside an initializer expression has no statement position to
        // become a state.
        RawStmtKind::Local(l) => l.init.as_ref().is_some_and(expr_contains_tick),
        RawStmtKind::Item(_) => false,
    })
}

fn unflattenable_tick_in_expr(e: &ExprType) -> bool {
    match e {
        // Any tick under a repeating construct, however deep.
        ExprType::Loop(l) => stmts_contain_tick(&l.body),
        ExprType::While(w) => stmts_contain_tick(&w.body),
        ExprType::ForLoop(f) => stmts_contain_tick(&f.body),
        // Structures the flattener DOES descend into — keep looking inside them.
        ExprType::If(f) => {
            contains_unflattenable_tick(&f.then_block)
                || f.else_branch.as_deref().is_some_and(unflattenable_tick_in_expr)
        }
        ExprType::Block(b) => contains_unflattenable_tick(&b.stmts),
        ExprType::Match(m) => m.arms.iter().any(|a| unflattenable_tick_in_expr(&a.body)),
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
