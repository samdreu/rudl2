//! Phase E — SystemVerilog text emission: `VLIRModule` -> `String`.
//!
//! Purely mechanical serialization. No semantic decisions — those were fixed in
//! Phases C/D. See `design_docs/EMISSION_DESIGN.md`.

use std::collections::HashSet;

use copper_core::chir::Width;
use copper_core::vlir::{
    ToolchainProfile, VLIRAlwaysFF, VLIRBinOp, VLIRBody, VLIRCombBody, VLIRCombPhase,
    VLIRContinuousAssign, VLIRExpr, VLIRFFStmt, VLIRModule, VLIRPort, VLIRPortDir, VLIRPortKind,
    VLIRRegDecl, VLIRSeqBody, VLIRStmt, VLIRSubmoduleInst, VLIRUnOp,
};

pub struct EmitConfig {
    pub profile: ToolchainProfile,
    pub indent_width: usize,
}

impl Default for EmitConfig {
    fn default() -> Self {
        EmitConfig { profile: ToolchainProfile::default(), indent_width: 4 }
    }
}

pub fn emit_verilog(module: &VLIRModule, config: &EmitConfig) -> String {
    let mut e = Emitter { out: String::new(), cfg: config };
    e.module(module);
    e.out
}

struct Emitter<'a> {
    out: String,
    cfg: &'a EmitConfig,
}

impl Emitter<'_> {
    fn indent(&self, level: usize) -> String {
        " ".repeat(level * self.cfg.indent_width)
    }

    fn module(&mut self, m: &VLIRModule) {
        self.emit_port_header(m);
        self.out.push('\n');
        match &m.body {
            VLIRBody::Combinational(c) => self.comb_body(c),
            VLIRBody::Sequential(s) => self.seq_body(s),
        }
        self.out.push_str("endmodule\n");
    }

    // ── Header ──────────────────────────────────────────────────────────────

    fn emit_port_header(&mut self, m: &VLIRModule) {
        if m.params.is_empty() {
            self.out.push_str(&format!("module {} (\n", m.name));
        } else {
            // `module name #(parameter int N = 8, parameter int M = 4) (`
            self.out.push_str(&format!("module {} #(\n", m.name));
            let last = m.params.len().saturating_sub(1);
            for (i, p) in m.params.iter().enumerate() {
                // Always emit a default so the module is valid standalone (a
                // parameter with no value is a Verilator lint error). Use the
                // source default when known, else a `1` placeholder that an
                // instantiation overrides.
                let default = p.default.unwrap_or(1);
                let comma = if i == last { "" } else { "," };
                self.out.push_str(&format!(
                    "{}parameter int {} = {}{}\n",
                    self.indent(1),
                    p.name,
                    default,
                    comma
                ));
            }
            self.out.push_str(") (\n");
        }
        // Clock inputs first, then other inputs, then outputs.
        let mut ordered: Vec<&VLIRPort> = Vec::new();
        ordered.extend(m.ports.iter().filter(|p| p.kind == VLIRPortKind::Clock));
        ordered.extend(m.ports.iter().filter(|p| {
            p.direction == VLIRPortDir::Input && p.kind != VLIRPortKind::Clock
        }));
        ordered.extend(m.ports.iter().filter(|p| p.direction == VLIRPortDir::Output));

        let last = ordered.len().saturating_sub(1);
        for (i, p) in ordered.iter().enumerate() {
            let dir = match p.direction {
                VLIRPortDir::Input => "input ",
                VLIRPortDir::Output => "output",
            };
            let range = range_str(&p.width);
            let comma = if i == last { "" } else { "," };
            self.out.push_str(&format!(
                "{}{} logic {}{}{}\n",
                self.indent(1),
                dir,
                range,
                p.name,
                comma
            ));
        }
        self.out.push_str(");\n");
    }

    // ── Combinational body ──────────────────────────────────────────────────

    fn comb_body(&mut self, c: &VLIRCombBody) {
        self.submodules(&c.submodules);
        self.emit_wire_decls(&c.comb_stmts);
        if !c.comb_stmts.is_empty() {
            self.out.push_str(&format!("{}always_comb begin\n", self.indent(1)));
            for s in &c.comb_stmts {
                self.comb_stmt(s, 2);
            }
            self.out.push_str(&format!("{}end\n\n", self.indent(1)));
        }
        self.output_assigns(&c.output_assigns);
    }

    // ── Sequential body ─────────────────────────────────────────────────────

    fn seq_body(&mut self, s: &VLIRSeqBody) {
        for r in &s.reg_decls {
            self.reg_decl(r);
        }
        if !s.reg_decls.is_empty() {
            self.out.push('\n');
        }

        self.submodules(&s.submodules);

        // Declare intermediate combinational wires used in the pre-edge phases.
        let phase_stmts: Vec<&VLIRStmt> =
            s.comb_phases.iter().flat_map(|p| p.stmts.iter()).collect();
        let mut decls = Vec::new();
        let mut seen = HashSet::new();
        for st in phase_stmts {
            collect_wire_decls(std::slice::from_ref(st), &mut decls, &mut seen);
        }
        for (name, width) in &decls {
            self.out.push_str(&format!("{}logic {}{};\n", self.indent(1), range_str(width), name));
        }
        if !decls.is_empty() {
            self.out.push('\n');
        }

        // always_comb (one guarded block covering all phases, or a bare block).
        let has_comb = s.comb_phases.iter().any(|p| !p.stmts.is_empty());
        if has_comb {
            self.out.push_str(&format!("{}always_comb begin\n", self.indent(1)));
            for phase in &s.comb_phases {
                self.comb_phase(phase);
            }
            self.out.push_str(&format!("{}end\n\n", self.indent(1)));
        }

        self.always_ff(&s.always_ff);
        self.out.push('\n');
        self.output_assigns(&s.output_assigns);
    }

    fn comb_phase(&mut self, phase: &VLIRCombPhase) {
        if phase.stmts.is_empty() {
            return;
        }
        match &phase.phase_guard {
            None => {
                for s in &phase.stmts {
                    self.comb_stmt(s, 2);
                }
            }
            Some(guard) => {
                self.out.push_str(&format!("{}if ({}) begin\n", self.indent(2), expr_str(guard)));
                for s in &phase.stmts {
                    self.comb_stmt(s, 3);
                }
                self.out.push_str(&format!("{}end\n", self.indent(2)));
            }
        }
    }

    /// Emit `logic` declarations for every intermediate wire assigned in the
    /// given combinational statements (a signal written in `always_comb` must be
    /// declared). Deduplicated by name.
    fn emit_wire_decls(&mut self, stmts: &[VLIRStmt]) {
        let mut decls = Vec::new();
        let mut seen = HashSet::new();
        collect_wire_decls(stmts, &mut decls, &mut seen);
        for (name, width) in &decls {
            self.out.push_str(&format!("{}logic {}{};\n", self.indent(1), range_str(width), name));
        }
        if !decls.is_empty() {
            self.out.push('\n');
        }
    }

    fn reg_decl(&mut self, r: &VLIRRegDecl) {
        self.out.push_str(&format!("{}logic {}{};\n", self.indent(1), range_str(&r.width), r.name));
    }

    // ── Submodules ──────────────────────────────────────────────────────────

    fn submodules(&mut self, subs: &[VLIRSubmoduleInst]) {
        for s in subs {
            // Declare the instance's output wire immediately before the instance.
            self.out.push_str(&format!(
                "{}logic {}{};\n",
                self.indent(1),
                range_str(&s.output_width),
                s.output_wire
            ));
            self.out.push_str(&format!("{}{} {} (\n", self.indent(1), s.module_name, s.inst_name));
            // Named input connections, then the output port. Note: SHIR carries
            // only the callee output *wire*, not its port name; M1 has no
            // submodules, so we use the conventional `.out`. Threading the real
            // callee output port name is tracked for the hierarchy milestone (M3).
            for (port, val) in &s.inputs {
                self.out.push_str(&format!("{}.{} ({}),\n", self.indent(2), port, expr_str(val)));
            }
            self.out.push_str(&format!("{}.out ({})\n", self.indent(2), s.output_wire));
            self.out.push_str(&format!("{});\n\n", self.indent(1)));
        }
    }

    // ── always_ff ───────────────────────────────────────────────────────────

    fn always_ff(&mut self, ff: &VLIRAlwaysFF) {
        self.out
            .push_str(&format!("{}always_ff @(posedge {}) begin\n", self.indent(1), ff.clock));
        for s in &ff.stmts {
            self.ff_stmt(s, 2);
        }
        self.out.push_str(&format!("{}end\n", self.indent(1)));
    }

    fn ff_stmt(&mut self, s: &VLIRFFStmt, level: usize) {
        match s {
            VLIRFFStmt::NonBlockingAssign { target, value } => {
                self.out
                    .push_str(&format!("{}{} <= {};\n", self.indent(level), target, expr_str(value)));
            }
            VLIRFFStmt::If { condition, then_stmts, else_stmts } => {
                self.out
                    .push_str(&format!("{}if ({}) begin\n", self.indent(level), expr_str(condition)));
                for st in then_stmts {
                    self.ff_stmt(st, level + 1);
                }
                self.out.push_str(&format!("{}end", self.indent(level)));
                if let Some(e) = else_stmts {
                    self.out.push_str(" else begin\n");
                    for st in e {
                        self.ff_stmt(st, level + 1);
                    }
                    self.out.push_str(&format!("{}end\n", self.indent(level)));
                } else {
                    self.out.push('\n');
                }
            }
            VLIRFFStmt::Case { selector, arms, default } => {
                self.out.push_str(&format!("{}case ({})\n", self.indent(level), expr_str(selector)));
                for arm in arms {
                    self.out.push_str(&format!(
                        "{}{}: begin\n",
                        self.indent(level + 1),
                        expr_str(&arm.selector_value)
                    ));
                    for st in &arm.stmts {
                        self.ff_stmt(st, level + 2);
                    }
                    self.out.push_str(&format!("{}end\n", self.indent(level + 1)));
                }
                if let Some(d) = default {
                    self.out.push_str(&format!("{}default: begin\n", self.indent(level + 1)));
                    for st in d {
                        self.ff_stmt(st, level + 2);
                    }
                    self.out.push_str(&format!("{}end\n", self.indent(level + 1)));
                }
                self.out.push_str(&format!("{}endcase\n", self.indent(level)));
            }
        }
    }

    // ── always_comb statements ───────────────────────────────────────────────

    fn comb_stmt(&mut self, s: &VLIRStmt, level: usize) {
        match s {
            VLIRStmt::WireAssign { name, value, .. } => {
                self.out
                    .push_str(&format!("{}{} = {};\n", self.indent(level), name, expr_str(value)));
            }
            VLIRStmt::PortAssign { port_name, value } => {
                self.out.push_str(&format!(
                    "{}{} = {};\n",
                    self.indent(level),
                    port_name,
                    expr_str(value)
                ));
            }
            VLIRStmt::If { condition, then_stmts, else_stmts } => {
                self.out
                    .push_str(&format!("{}if ({}) begin\n", self.indent(level), expr_str(condition)));
                for st in then_stmts {
                    self.comb_stmt(st, level + 1);
                }
                self.out.push_str(&format!("{}end", self.indent(level)));
                if let Some(e) = else_stmts {
                    self.out.push_str(" else begin\n");
                    for st in e {
                        self.comb_stmt(st, level + 1);
                    }
                    self.out.push_str(&format!("{}end\n", self.indent(level)));
                } else {
                    self.out.push('\n');
                }
            }
            VLIRStmt::Case { selector, arms, default } => {
                self.out.push_str(&format!("{}case ({})\n", self.indent(level), expr_str(selector)));
                for arm in arms {
                    self.out.push_str(&format!(
                        "{}{}: begin\n",
                        self.indent(level + 1),
                        expr_str(&arm.selector_value)
                    ));
                    for st in &arm.stmts {
                        self.comb_stmt(st, level + 2);
                    }
                    self.out.push_str(&format!("{}end\n", self.indent(level + 1)));
                }
                if let Some(d) = default {
                    self.out.push_str(&format!("{}default: begin\n", self.indent(level + 1)));
                    for st in d {
                        self.comb_stmt(st, level + 2);
                    }
                    self.out.push_str(&format!("{}end\n", self.indent(level + 1)));
                }
                self.out.push_str(&format!("{}endcase\n", self.indent(level)));
            }
            VLIRStmt::ForLoop { var, start, end, body } => {
                self.out.push_str(&format!(
                    "{}for (int {v} = {}; {v} < {}; {v}++) begin\n",
                    self.indent(level),
                    expr_str(start),
                    expr_str(end),
                    v = var,
                ));
                for st in body {
                    self.comb_stmt(st, level + 1);
                }
                self.out.push_str(&format!("{}end\n", self.indent(level)));
            }
        }
    }

    fn output_assigns(&mut self, assigns: &[VLIRContinuousAssign]) {
        for a in assigns {
            self.out.push_str(&format!("{}assign {} = {};\n", self.indent(1), a.target, expr_str(&a.value)));
        }
        if !assigns.is_empty() {
            self.out.push('\n');
        }
    }
}

/// Recursively collect `(name, width)` of every `WireAssign` target in a
/// combinational statement tree, deduplicated by `seen`.
fn collect_wire_decls(
    stmts: &[VLIRStmt],
    out: &mut Vec<(String, Width)>,
    seen: &mut HashSet<String>,
) {
    for s in stmts {
        match s {
            VLIRStmt::WireAssign { name, width, .. } => {
                if seen.insert(name.clone()) {
                    out.push((name.clone(), width.clone()));
                }
            }
            VLIRStmt::PortAssign { .. } => {}
            VLIRStmt::If { then_stmts, else_stmts, .. } => {
                collect_wire_decls(then_stmts, out, seen);
                if let Some(e) = else_stmts {
                    collect_wire_decls(e, out, seen);
                }
            }
            VLIRStmt::Case { arms, default, .. } => {
                for a in arms {
                    collect_wire_decls(&a.stmts, out, seen);
                }
                if let Some(d) = default {
                    collect_wire_decls(d, out, seen);
                }
            }
            VLIRStmt::ForLoop { body, .. } => collect_wire_decls(body, out, seen),
        }
    }
}

// ── Width / expression formatting (the single width->text route, D1a) ────────

/// The one place a width becomes Verilog text — so a future symbolic width
/// (`[N-1:0]`) is a change here only.
fn range_str(w: &Width) -> String {
    match w {
        Width::Concrete(1) => String::new(),
        Width::Concrete(n) => format!("[{}:0] ", n - 1),
        // A parametric width `N` renders as `[N-1:0]`.
        Width::Param(name) => format!("[{name}-1:0] "),
    }
}

fn lit_str(width: &Width, value: u128) -> String {
    match width {
        Width::Concrete(1) => format!("1'b{}", value & 1),
        Width::Concrete(n) => format!("{}'d{}", n, value),
        // A literal sized by a parameter, e.g. `N'd5`.
        Width::Param(name) => format!("{name}'d{value}"),
    }
}

/// Fully parenthesized expression text — never rely on operator precedence.
fn expr_str(e: &VLIRExpr) -> String {
    match e {
        VLIRExpr::Var(name) => name.clone(),
        VLIRExpr::Lit { width, value } => lit_str(width, *value),
        VLIRExpr::BinOp { left, op, right } => {
            format!("({} {} {})", expr_str(left), binop_str(*op), expr_str(right))
        }
        VLIRExpr::UnOp { op, expr } => format!("({}{})", unop_str(*op), expr_str(expr)),
        VLIRExpr::Ternary { cond, then_val, else_val } => format!(
            "({} ? {} : {})",
            expr_str(cond),
            expr_str(then_val),
            expr_str(else_val)
        ),
        VLIRExpr::Concat(parts) => {
            let inner: Vec<String> = parts.iter().map(expr_str).collect();
            format!("{{{}}}", inner.join(", "))
        }
        VLIRExpr::Slice { expr, high, low } => {
            if high == low {
                format!("{}[{}]", expr_str(expr), high)
            } else {
                format!("{}[{}:{}]", expr_str(expr), high, low)
            }
        }
    }
}

fn binop_str(op: VLIRBinOp) -> &'static str {
    match op {
        VLIRBinOp::Add => "+",
        VLIRBinOp::Sub => "-",
        VLIRBinOp::Mul => "*",
        VLIRBinOp::BitAnd => "&",
        VLIRBinOp::BitOr => "|",
        VLIRBinOp::BitXor => "^",
        VLIRBinOp::Shl => "<<",
        VLIRBinOp::Shr => ">>",
        VLIRBinOp::Eq => "==",
        VLIRBinOp::Neq => "!=",
        VLIRBinOp::Lt => "<",
        VLIRBinOp::Lte => "<=",
        VLIRBinOp::Gt => ">",
        VLIRBinOp::Gte => ">=",
        VLIRBinOp::LogicalAnd => "&&",
        VLIRBinOp::LogicalOr => "||",
    }
}

fn unop_str(op: VLIRUnOp) -> &'static str {
    match op {
        VLIRUnOp::BitNot => "~",
        VLIRUnOp::LogicalNot => "!",
        VLIRUnOp::Neg => "-",
        VLIRUnOp::ReductionAnd => "&",
        VLIRUnOp::ReductionOr => "|",
        VLIRUnOp::ReductionXor => "^",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_core::vlir::{
        VLIRBody, VLIRCombBody, VLIRExpr, VLIRModule, VLIRParam, VLIRPort, VLIRPortDir,
        VLIRPortKind, VLIRStmt,
    };

    /// A `ForLoop` statement over a parameter bound renders as an SV `for` inside
    /// `always_comb`, with the body indented (for-loop structure lowering).
    #[test]
    fn for_loop_renders_sv_for() {
        let module = VLIRModule {
            name: "loopy".to_string(),
            params: vec![VLIRParam { name: "N".to_string(), default: Some(8) }],
            ports: vec![VLIRPort {
                name: "out".to_string(),
                direction: VLIRPortDir::Output,
                kind: VLIRPortKind::Logic,
                width: Width::Concrete(8),
            }],
            body: VLIRBody::Combinational(VLIRCombBody {
                submodules: vec![],
                comb_stmts: vec![VLIRStmt::ForLoop {
                    var: "i".to_string(),
                    start: VLIRExpr::Lit { width: Width::Concrete(32), value: 0 },
                    end: VLIRExpr::Var("N".to_string()),
                    body: vec![VLIRStmt::WireAssign {
                        name: "acc".to_string(),
                        width: Width::Concrete(8),
                        value: VLIRExpr::Var("i".to_string()),
                    }],
                }],
                output_assigns: vec![],
            }),
        };
        let sv = emit_verilog(&module, &EmitConfig::default());
        assert!(
            sv.contains("for (int i = 32'd0; i < N; i++) begin"),
            "expected SV for loop, got:\n{sv}"
        );
        assert!(sv.contains("acc = i;"), "expected loop body, got:\n{sv}");
    }
}
