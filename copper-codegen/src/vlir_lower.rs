//! Phase D — Verilog legalization: `SHIRModule` -> `VLIRModule`.
//!
//! A mechanical, semantics-preserving pass. See `design_docs/VLIR_DESIGN.md`.
//!
//! Implemented for Milestone 1: name legalization (D2), literal width
//! annotation (D3/D5), mux->ternary + case-expr lifting (D4), and multi-phase
//! guard injection (D6). Tuple-pattern match lowering and conditional output
//! drives are recognized-but-not-yet-supported and produce a clear error.

use std::collections::{HashMap, HashSet};

use copper_core::chir::{CHIRBinOp, CHIRType, CHIRUnOp, Width};
use copper_core::shir::{
    SHIRBody, SHIRCombBody, SHIRExpr, SHIRLit, SHIRModule, SHIRPhase, SHIRPortDir, SHIRPortKind,
    SHIRRegUpdate, SHIRSeqBody, SHIRStmt, SHIRSubmoduleInst,
};
use copper_core::vlir::{
    VLIRAlwaysFF, VLIRBinOp, VLIRBody, VLIRCombBody, VLIRCombPhase, VLIRContinuousAssign, VLIRExpr,
    VLIRFFStmt, VLIRModule, VLIRPort, VLIRPortDir, VLIRPortKind, VLIRRegDecl, VLIRSeqBody, VLIRStmt,
    VLIRSubmoduleInst, VLIRUnOp,
};

// ── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum VLIRLowerError {
    /// A `match` on a tuple scrutinee — deferred to M2 (see VLIR_DESIGN §Pass 2).
    TuplePatternUnsupported,
    /// An output port driven inside a conditional branch — deferred to M2.
    ConditionalOutputUnsupported { port: String },
    /// A `Case` expression nested inside another expression (only top-level
    /// case-expressions in reg updates / wire values are lifted for now).
    NestedCaseExpr,
    /// A non-concrete width reached Phase D — parametric widths are M2.
    SymbolicWidthUnsupported,
}

impl std::fmt::Display for VLIRLowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VLIRLowerError::TuplePatternUnsupported =>
                write!(f, "tuple-pattern match lowering is not yet implemented (M2)"),
            VLIRLowerError::ConditionalOutputUnsupported { port } =>
                write!(f, "conditional drive of output port '{port}' is not yet implemented (M2)"),
            VLIRLowerError::NestedCaseExpr =>
                write!(f, "match-as-expression nested inside another expression is not yet implemented"),
            VLIRLowerError::SymbolicWidthUnsupported =>
                write!(f, "parametric/symbolic bit widths are not yet implemented (M2)"),
        }
    }
}

type LowerResult<T> = Result<T, VLIRLowerError>;

// ── Public entry point ──────────────────────────────────────────────────────

pub fn lower_to_vlir(shir: &SHIRModule) -> LowerResult<VLIRModule> {
    let mut leg = Legalizer::new();

    // Legalize all declared names up front so every reference resolves
    // consistently, regardless of body-traversal order.
    let name = leg.legalize(&shir.name);
    for p in &shir.ports {
        leg.legalize(&p.name);
    }
    collect_and_legalize_body_names(&shir.body, &mut leg);

    let ports = shir
        .ports
        .iter()
        .map(|p| VLIRPort {
            name: leg.get(&p.name),
            direction: match p.direction {
                SHIRPortDir::Input => VLIRPortDir::Input,
                SHIRPortDir::Output => VLIRPortDir::Output,
            },
            kind: match &p.kind {
                SHIRPortKind::Clock => VLIRPortKind::Clock,
                SHIRPortKind::Data { .. } => VLIRPortKind::Logic,
            },
            width: match &p.kind {
                SHIRPortKind::Clock => Width::Concrete(1),
                SHIRPortKind::Data { ty } => width_of(ty),
            },
        })
        .collect();

    let body = match &shir.body {
        SHIRBody::Combinational(c) => VLIRBody::Combinational(lower_comb(c, &leg)?),
        SHIRBody::Sequential(s) => VLIRBody::Sequential(lower_seq(s, &leg)?),
    };

    Ok(VLIRModule { name, params: Vec::new(), ports, body })
}

// ── Combinational body ──────────────────────────────────────────────────────

fn lower_comb(c: &SHIRCombBody, leg: &Legalizer) -> LowerResult<VLIRCombBody> {
    let submodules = c.submodules.iter().map(|s| lower_submodule(s, leg)).collect::<LowerResult<_>>()?;
    let (comb_stmts, output_assigns) = lower_flat_stmts(&c.stmts, leg)?;
    Ok(VLIRCombBody { submodules, comb_stmts, output_assigns })
}

// ── Sequential body ─────────────────────────────────────────────────────────

fn lower_seq(s: &SHIRSeqBody, leg: &Legalizer) -> LowerResult<VLIRSeqBody> {
    let clock = leg.get(&s.clock);

    let reg_decls = s
        .registers
        .iter()
        .map(|r| VLIRRegDecl { name: leg.get(&r.name), width: width_of(&r.ty) })
        .collect();

    let submodules = s.submodules.iter().map(|m| lower_submodule(m, leg)).collect::<LowerResult<_>>()?;

    // Width of the phase register (for phase guards / PhaseEq literals).
    let phase_r_width = s
        .registers
        .iter()
        .find(|r| r.name == "phase_r")
        .map(|r| width_of(&r.ty))
        .unwrap_or(Width::Concrete(1));

    let multi_phase = s.phases.len() > 1;

    let mut comb_phases = Vec::new();
    let mut output_assigns = Vec::new();
    let mut ff_stmts = Vec::new();

    for phase in &s.phases {
        let (stmts, mut outs) = lower_flat_stmts(&phase.pre_edge, leg)?;
        // Output continuous assigns are module-level; collect from every phase.
        output_assigns.append(&mut outs);

        let phase_guard = if multi_phase {
            Some(phase_eq(phase.phase_idx, &phase_r_width))
        } else {
            None
        };
        comb_phases.push(VLIRCombPhase { phase_guard, stmts });

        // Register updates -> always_ff non-blocking assigns, guarded by phase
        // for multi-phase modules.
        let updates = lower_reg_updates(&phase.post_edge, leg)?;
        if multi_phase {
            ff_stmts.push(VLIRFFStmt::If {
                condition: phase_eq(phase.phase_idx, &phase_r_width),
                then_stmts: updates,
                else_stmts: None,
            });
        } else {
            ff_stmts.extend(updates);
        }
    }

    Ok(VLIRSeqBody {
        clock: clock.clone(),
        reg_decls,
        submodules,
        comb_phases,
        always_ff: VLIRAlwaysFF { clock, stmts: ff_stmts },
        output_assigns,
    })
}

fn phase_eq(idx: usize, width: &Width) -> VLIRExpr {
    VLIRExpr::BinOp {
        left: Box::new(VLIRExpr::Var("phase_r".to_string())),
        op: VLIRBinOp::Eq,
        right: Box::new(VLIRExpr::Lit { width: width.clone(), value: idx as u128 }),
    }
}

fn lower_reg_updates(updates: &[SHIRRegUpdate], leg: &Legalizer) -> LowerResult<Vec<VLIRFFStmt>> {
    let mut out = Vec::new();
    for u in updates {
        let target = leg.get(&u.target);
        // Top-level case-as-expression is lifted into a case statement.
        if let SHIRExpr::Case { scrutinee, arms, default } = &u.next_value {
            let selector = lower_expr(scrutinee, leg)?;
            let mut ff_arms = Vec::new();
            for arm in arms {
                let selector_value = pattern_to_selector(&arm.pattern)?;
                ff_arms.push(copper_core::vlir::VLIRFFCaseArm {
                    selector_value,
                    stmts: vec![VLIRFFStmt::NonBlockingAssign {
                        target: target.clone(),
                        value: lower_expr(&arm.value, leg)?,
                    }],
                });
            }
            out.push(VLIRFFStmt::Case {
                selector,
                arms: ff_arms,
                default: Some(vec![VLIRFFStmt::NonBlockingAssign {
                    target: target.clone(),
                    value: lower_expr(default, leg)?,
                }]),
            });
        } else {
            out.push(VLIRFFStmt::NonBlockingAssign {
                target,
                value: lower_expr(&u.next_value, leg)?,
            });
        }
    }
    Ok(out)
}

// ── Flat statement lowering (shared by comb + per-phase pre_edge) ────────────
//
// Returns (always_comb statements, module-level continuous output assigns).
// A top-level `PortDrive` becomes a continuous `assign`; a `PortDrive` nested
// inside `If`/`Match` is a conditional output (deferred to M2).

fn lower_flat_stmts(
    stmts: &[SHIRStmt],
    leg: &Legalizer,
) -> LowerResult<(Vec<VLIRStmt>, Vec<VLIRContinuousAssign>)> {
    let mut comb = Vec::new();
    let mut assigns = Vec::new();
    for s in stmts {
        match s {
            SHIRStmt::Wire { name, ty, value } => comb.push(VLIRStmt::WireAssign {
                name: leg.get(name),
                width: width_of(ty),
                value: lower_expr(value, leg)?,
            }),
            SHIRStmt::PortDrive { port_name, value } => assigns.push(VLIRContinuousAssign {
                target: leg.get(port_name),
                value: lower_expr(value, leg)?,
            }),
            SHIRStmt::If { .. } | SHIRStmt::Match { .. } => {
                // Only conditional *wire* logic is supported here; a conditional
                // that drives an output port is deferred (M2). Detect that.
                if let Some(port) = first_port_drive(s) {
                    return Err(VLIRLowerError::ConditionalOutputUnsupported { port });
                }
                comb.push(lower_comb_stmt(s, leg)?);
            }
        }
    }
    Ok((comb, assigns))
}

/// Lower an `If`/`Match` statement whose leaves are wire assignments only.
fn lower_comb_stmt(s: &SHIRStmt, leg: &Legalizer) -> LowerResult<VLIRStmt> {
    match s {
        SHIRStmt::Wire { name, ty, value } => Ok(VLIRStmt::WireAssign {
            name: leg.get(name),
            width: width_of(ty),
            value: lower_expr(value, leg)?,
        }),
        SHIRStmt::PortDrive { port_name, .. } => {
            Err(VLIRLowerError::ConditionalOutputUnsupported { port: port_name.clone() })
        }
        SHIRStmt::If { condition, then_stmts, else_stmts } => Ok(VLIRStmt::If {
            condition: lower_expr(condition, leg)?,
            then_stmts: then_stmts.iter().map(|s| lower_comb_stmt(s, leg)).collect::<LowerResult<_>>()?,
            else_stmts: match else_stmts {
                Some(e) => Some(e.iter().map(|s| lower_comb_stmt(s, leg)).collect::<LowerResult<_>>()?),
                None => None,
            },
        }),
        SHIRStmt::Match { .. } => Err(VLIRLowerError::TuplePatternUnsupported),
    }
}

fn first_port_drive(s: &SHIRStmt) -> Option<String> {
    match s {
        SHIRStmt::PortDrive { port_name, .. } => Some(port_name.clone()),
        SHIRStmt::Wire { .. } => None,
        SHIRStmt::If { then_stmts, else_stmts, .. } => then_stmts
            .iter()
            .chain(else_stmts.iter().flatten())
            .find_map(first_port_drive),
        SHIRStmt::Match { arms, .. } => arms.iter().flat_map(|a| &a.stmts).find_map(first_port_drive),
    }
}

// ── Submodule ───────────────────────────────────────────────────────────────

fn lower_submodule(m: &SHIRSubmoduleInst, leg: &Legalizer) -> LowerResult<VLIRSubmoduleInst> {
    Ok(VLIRSubmoduleInst {
        inst_name: leg.get(&m.inst_name),
        module_name: leg.get(&m.module_name),
        inputs: m
            .inputs
            .iter()
            .map(|(port, e)| Ok((leg.get(port), lower_expr(e, leg)?)))
            .collect::<LowerResult<_>>()?,
        output_wire: leg.get(&m.output_wire),
        output_width: width_of(&m.output_ty),
    })
}

// ── Expression lowering ─────────────────────────────────────────────────────

fn lower_expr(e: &SHIRExpr, leg: &Legalizer) -> LowerResult<VLIRExpr> {
    Ok(match e {
        SHIRExpr::Var(name) => VLIRExpr::Var(leg.get(name)),
        SHIRExpr::Lit(lit) => lower_lit(lit),
        SHIRExpr::BinOp { left, op, right } => VLIRExpr::BinOp {
            left: Box::new(lower_expr(left, leg)?),
            op: lower_binop(op),
            right: Box::new(lower_expr(right, leg)?),
        },
        SHIRExpr::UnOp { op, expr } => VLIRExpr::UnOp {
            op: lower_unop(op),
            expr: Box::new(lower_expr(expr, leg)?),
        },
        SHIRExpr::Mux { cond, then_val, else_val } => VLIRExpr::Ternary {
            cond: Box::new(lower_expr(cond, leg)?),
            then_val: Box::new(lower_expr(then_val, leg)?),
            else_val: Box::new(lower_expr(else_val, leg)?),
        },
        SHIRExpr::Concat(parts) => {
            VLIRExpr::Concat(parts.iter().map(|p| lower_expr(p, leg)).collect::<LowerResult<_>>()?)
        }
        SHIRExpr::Slice { expr, high, low } => VLIRExpr::Slice {
            expr: Box::new(lower_expr(expr, leg)?),
            high: *high,
            low: *low,
        },
        SHIRExpr::PhaseEq(idx) => VLIRExpr::BinOp {
            left: Box::new(VLIRExpr::Var("phase_r".to_string())),
            op: VLIRBinOp::Eq,
            // Width filled by the phase register; 32 is a safe upper bound for a
            // bare comparison literal (SV zero-extends). Kept minimal here.
            right: Box::new(VLIRExpr::Lit { width: Width::Concrete(32), value: *idx as u128 }),
        },
        // A case used as a sub-expression must be lifted by the caller; reaching
        // here means it was nested, which we do not handle yet.
        SHIRExpr::Case { .. } => return Err(VLIRLowerError::NestedCaseExpr),
    })
}

fn lower_lit(lit: &SHIRLit) -> VLIRExpr {
    VLIRExpr::Lit { width: width_of(&lit.ty), value: lit.value }
}

fn lower_binop(op: &CHIRBinOp) -> VLIRBinOp {
    match op {
        // Fixed-width Verilog arithmetic already wraps; `wrapping` is a no-op here.
        CHIRBinOp::Add { .. } => VLIRBinOp::Add,
        CHIRBinOp::Sub { .. } => VLIRBinOp::Sub,
        CHIRBinOp::Mul { .. } => VLIRBinOp::Mul,
        CHIRBinOp::BitAnd => VLIRBinOp::BitAnd,
        CHIRBinOp::BitOr => VLIRBinOp::BitOr,
        CHIRBinOp::BitXor => VLIRBinOp::BitXor,
        CHIRBinOp::Shl => VLIRBinOp::Shl,
        CHIRBinOp::Shr => VLIRBinOp::Shr,
        CHIRBinOp::Eq => VLIRBinOp::Eq,
        CHIRBinOp::Neq => VLIRBinOp::Neq,
        CHIRBinOp::Lt => VLIRBinOp::Lt,
        CHIRBinOp::Lte => VLIRBinOp::Lte,
        CHIRBinOp::Gt => VLIRBinOp::Gt,
        CHIRBinOp::Gte => VLIRBinOp::Gte,
        CHIRBinOp::LogicalAnd => VLIRBinOp::LogicalAnd,
        CHIRBinOp::LogicalOr => VLIRBinOp::LogicalOr,
    }
}

fn lower_unop(op: &CHIRUnOp) -> VLIRUnOp {
    match op {
        CHIRUnOp::BitNot => VLIRUnOp::BitNot,
        CHIRUnOp::LogicalNot => VLIRUnOp::LogicalNot,
        CHIRUnOp::Neg => VLIRUnOp::Neg,
        CHIRUnOp::ReductionAnd => VLIRUnOp::ReductionAnd,
        CHIRUnOp::ReductionOr => VLIRUnOp::ReductionOr,
        CHIRUnOp::ReductionXor => VLIRUnOp::ReductionXor,
    }
}

/// Convert a scalar SHIR pattern into a concrete case-selector literal.
/// Tuple patterns are deferred to M2.
fn pattern_to_selector(p: &copper_core::shir::SHIRPattern) -> LowerResult<VLIRExpr> {
    use copper_core::shir::SHIRPattern;
    match p {
        SHIRPattern::Lit(lit) => Ok(lower_lit(lit)),
        SHIRPattern::Tuple(_) => Err(VLIRLowerError::TuplePatternUnsupported),
        // Wildcard / enum-variant selectors are handled via the case `default`
        // arm or enum encoding, not implemented for M1's scalar cases.
        SHIRPattern::Wildcard | SHIRPattern::EnumVariant { .. } => {
            Err(VLIRLowerError::TuplePatternUnsupported)
        }
    }
}

// ── Width helper ────────────────────────────────────────────────────────────

fn width_of(ty: &CHIRType) -> Width {
    match ty {
        CHIRType::UInt { width } | CHIRType::SInt { width } => width.clone(),
        CHIRType::Bool => Width::Concrete(1),
    }
}

// ── Name legalization (Pass 1) ──────────────────────────────────────────────

struct Legalizer {
    map: std::cell::RefCell<HashMap<String, String>>,
    used: std::cell::RefCell<HashSet<String>>,
}

impl Legalizer {
    fn new() -> Self {
        Legalizer {
            map: std::cell::RefCell::new(HashMap::new()),
            used: std::cell::RefCell::new(HashSet::new()),
        }
    }

    /// Legalize `name`, caching the result. Idempotent per original name.
    fn legalize(&mut self, name: &str) -> String {
        if let Some(existing) = self.map.borrow().get(name) {
            return existing.clone();
        }
        let mut base = sanitize(name);
        if is_reserved(&base) {
            base.push_str("_sig");
        }
        // Disambiguate collisions after sanitization.
        let mut candidate = base.clone();
        let mut n = 0;
        while self.used.borrow().contains(&candidate) {
            candidate = format!("{base}_{n}");
            n += 1;
        }
        self.used.borrow_mut().insert(candidate.clone());
        self.map.borrow_mut().insert(name.to_string(), candidate.clone());
        candidate
    }

    /// Look up an already-legalized name; falls back to sanitizing on the fly
    /// for names that were not pre-registered (should not happen for valid IR).
    fn get(&self, name: &str) -> String {
        self.map.borrow().get(name).cloned().unwrap_or_else(|| sanitize(name))
    }
}

fn sanitize(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if out.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true) {
        out = format!("sig_{out}");
    }
    out
}

fn collect_and_legalize_body_names(body: &SHIRBody, leg: &mut Legalizer) {
    match body {
        SHIRBody::Combinational(c) => {
            for s in &c.submodules {
                leg.legalize(&s.inst_name);
                leg.legalize(&s.module_name);
                leg.legalize(&s.output_wire);
            }
            collect_stmt_names(&c.stmts, leg);
        }
        SHIRBody::Sequential(s) => {
            leg.legalize(&s.clock);
            for r in &s.registers {
                leg.legalize(&r.name);
            }
            for m in &s.submodules {
                leg.legalize(&m.inst_name);
                leg.legalize(&m.module_name);
                leg.legalize(&m.output_wire);
            }
            for phase in &s.phases {
                collect_stmt_names(&phase.pre_edge, leg);
            }
        }
    }
}

fn collect_stmt_names(stmts: &[SHIRStmt], leg: &mut Legalizer) {
    for s in stmts {
        match s {
            SHIRStmt::Wire { name, .. } => {
                leg.legalize(name);
            }
            SHIRStmt::PortDrive { .. } => {}
            SHIRStmt::If { then_stmts, else_stmts, .. } => {
                collect_stmt_names(then_stmts, leg);
                if let Some(e) = else_stmts {
                    collect_stmt_names(e, leg);
                }
            }
            SHIRStmt::Match { arms, .. } => {
                for a in arms {
                    collect_stmt_names(&a.stmts, leg);
                }
            }
        }
    }
}

// Unused for M1 phases but referenced by the multi-phase path.
#[allow(dead_code)]
fn _phase_marker(_: &SHIRPhase) {}

/// Verilog / SystemVerilog reserved keywords (VLIR_DESIGN §Pass 1).
fn is_reserved(name: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "always", "always_comb", "always_ff", "always_latch", "and", "assign", "automatic",
        "begin", "bit", "buf", "byte", "case", "casex", "casez", "cell", "clocking", "config",
        "default", "defparam", "disable", "do", "edge", "else", "end", "endcase", "endconfig",
        "endfunction", "endgenerate", "endgroup", "endinterface", "endmodule", "endpackage",
        "endprimitive", "endprogram", "endproperty", "endspecify", "endsequence", "endtable",
        "endtask", "enum", "export", "extends", "extern", "final", "for", "force", "foreach",
        "forever", "fork", "function", "generate", "genvar", "if", "iff", "import", "initial",
        "inout", "input", "instance", "int", "integer", "interface", "join", "local", "localparam",
        "logic", "longint", "macromodule", "modport", "module", "nand", "negedge", "new", "nor",
        "not", "or", "output", "package", "packed", "parameter", "posedge", "primitive", "priority",
        "program", "property", "real", "realtime", "ref", "reg", "release", "repeat", "return",
        "sequence", "shortint", "shortreal", "signed", "specify", "specparam", "static", "string",
        "struct", "super", "table", "task", "this", "time", "timeprecision", "timeunit", "tran",
        "tri", "type", "typedef", "union", "unique", "unsigned", "var", "virtual", "void", "wait",
        "wand", "while", "wire", "with", "wor", "xnor", "xor",
    ];
    KEYWORDS.contains(&name)
}

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use crate::{transpile_item_fn, EmitConfig};
    use std::collections::{HashMap, HashSet};

    fn transpile(src: &str) -> String {
        let f: syn::ItemFn = syn::parse_str(src).expect("parse");
        transpile_item_fn(&f, &HashSet::new(), &HashMap::new(), &EmitConfig::default())
            .expect("transpile")
    }

    #[test]
    fn counter_emits_verilog() {
        let src = r#"
            async fn counter(clk: Clock<MainClk>, step: In<u8, MainClk>, out: Out<u8, MainClk>) {
                let mut count = 0u8;
                loop {
                    out.write(count);
                    clk.tick().await;
                    count = count.wrapping_add(step.read());
                }
            }
        "#;
        let sv = transpile(src);
        println!("\n===== GENERATED VERILOG (counter) =====\n{sv}=======================================");
        assert!(sv.contains("module counter"));
        assert!(sv.contains("always_ff @(posedge clk)"));
        assert!(sv.contains("assign out ="));
        assert!(sv.contains("<="));
    }

    /// Golden test: exact emitted text for the canonical counter. Catches
    /// unintended formatting/ordering churn in the emitter. Update the expected
    /// string deliberately when the emitter output is meant to change.
    #[test]
    fn counter_golden_output() {
        let src = r#"
            async fn counter(clk: Clock<MainClk>, step: In<u8, MainClk>, out: Out<u8, MainClk>) {
                let mut count = 0u8;
                loop {
                    out.write(count);
                    clk.tick().await;
                    count = count.wrapping_add(step.read());
                }
            }
        "#;
        let expected = "\
module counter (
    input  logic clk,
    input  logic [7:0] step,
    output logic [7:0] out
);

    logic [7:0] count;

    always_ff @(posedge clk) begin
        count <= (count + step);
    end

    assign out = count;

endmodule
";
        assert_eq!(transpile(src), expected);
    }

    /// Combinational `Logic` module: `.read()` + logic operators infer 1-bit
    /// widths, and intermediate wires are declared. Regression for the M2
    /// width-inference fix. (Previously failed with `AmbiguousWidth`.)
    #[test]
    fn logic_comb_module_golden_output() {
        let src = r#"
            fn one_bit_comparator(i0: In<Logic, ()>, i1: In<Logic, ()>, eq: Out<Logic, ()>) {
                let p0 = !i0.read() & !i1.read();
                let p1 = i0.read() & i1.read();
                eq.write(p0 | p1);
            }
        "#;
        let expected = "\
module one_bit_comparator (
    input  logic i0,
    input  logic i1,
    output logic eq
);

    logic p0;
    logic p1;

    always_comb begin
        p0 = ((!i0) & (!i1));
        p1 = (i0 & i1);
    end

    assign eq = (p0 | p1);

endmodule
";
        assert_eq!(transpile(src), expected);
    }

    /// Bit-indexing (`d[7]`) + `Logic::One` constant + conditional register
    /// update lower end-to-end to a bit-select, comparison, and ternary.
    #[test]
    fn bit_index_golden_output() {
        let src = r#"
            async fn m(clk: Clock<MainClk>, d: In<Bits<8>, MainClk>, o: Out<Bits<8>, MainClk>) {
                let mut r = Bits::from_u8(0);
                loop {
                    o.write(r);
                    clk.tick().await;
                    if d.read()[7] == Logic::One {
                        r = d.read();
                    }
                }
            }
        "#;
        let expected = "\
module m (
    input  logic clk,
    input  logic [7:0] d,
    output logic [7:0] o
);

    logic [7:0] r;

    always_ff @(posedge clk) begin
        r <= ((d[7] == 1'b1) ? d : r);
    end

    assign o = r;

endmodule
";
        assert_eq!(transpile(src), expected);
    }

    /// A `let`-bound `if`-expression with `{ block }` branches: the register
    /// update infers its width from a branch's block tail, and both branches
    /// lower via the block-tail extractor into a ternary.
    #[test]
    fn if_expr_with_block_branches_golden_output() {
        let src = r#"
            async fn m(clk: Clock<MainClk>, d: In<Bits<8>, MainClk>, sel: In<Logic, MainClk>, o: Out<Bits<8>, MainClk>) {
                let mut r = Bits::from_u8(0);
                loop {
                    o.write(r);
                    clk.tick().await;
                    r = if sel.read().as_bool() {
                        (d.read() >> 1) ^ r
                    } else {
                        d.read() >> 1
                    };
                }
            }
        "#;
        let expected = "\
module m (
    input  logic clk,
    input  logic [7:0] d,
    input  logic sel,
    output logic [7:0] o
);

    logic [7:0] r;

    always_ff @(posedge clk) begin
        r <= (sel ? ((d >> 64'd1) ^ r) : (d >> 64'd1));
    end

    assign o = r;

endmodule
";
        assert_eq!(transpile(src), expected);
    }

    /// The deferred-feature guards produce errors, not wrong Verilog.
    #[test]
    fn tuple_match_is_rejected_not_miscompiled() {
        // A match on a tuple scrutinee is an M2 feature; it must surface an
        // error rather than silently emitting incorrect SystemVerilog.
        let src = r#"
            async fn jk(clk: Clock<C>, j: In<bool, C>, k: In<bool, C>, q: Out<bool, C>) {
                let mut state = false;
                loop {
                    q.write(state);
                    clk.tick().await;
                    state = match (j.read(), k.read()) {
                        (false, false) => state,
                        (false, true) => false,
                        (true, false) => true,
                        _ => !state,
                    };
                }
            }
        "#;
        let f: syn::ItemFn = syn::parse_str(src).expect("parse");
        let result =
            transpile_item_fn(&f, &HashSet::new(), &HashMap::new(), &EmitConfig::default());
        assert!(result.is_err(), "tuple match should be rejected, got: {result:?}");
    }
}
