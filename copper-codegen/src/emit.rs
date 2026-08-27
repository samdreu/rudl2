//! Phase E — SystemVerilog text emission: `VLIRModule` -> `String`.
//!
//! Purely mechanical serialization. No semantic decisions — those were fixed in
//! Phases C/D. See `design_docs/EMISSION_DESIGN.md`.

use std::collections::HashSet;

use copper_core::chir::Width;
use copper_core::vlir::{
    ToolchainProfile, VLIRAlwaysFF, VLIRBinOp, VLIRBody, VLIRCombBody, VLIRCombPhase,
    VLIRContinuousAssign, VLIRExpr, VLIRFFStmt, VLIRLocalParam, VLIRModule, VLIRPort, VLIRPortDir,
    VLIRPortKind,
    VLIRMemDecl, VLIRMemInit, VLIRRegDecl, VLIRSeqBody, VLIRStmt, VLIRStructuralBody, VLIRSubmoduleInst,
    VLIRUnOp,
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
    let mut e = Emitter {
        out: String::new(),
        cfg: config,
        port_names: module.ports.iter().map(|p| p.name.clone()).collect(),
    };
    e.module(module);
    e.out
}

struct Emitter<'a> {
    out: String,
    cfg: &'a EmitConfig,
    /// Port names — a received-memory bus port is DRIVEN by the same
    /// `always_comb` nets an internal memory uses, so the wire-decl collector
    /// must not redeclare it (`logic … m_wr0_addr;` next to the port is a
    /// double declaration).
    port_names: HashSet<String>,
}

impl Emitter<'_> {
    fn indent(&self, level: usize) -> String {
        " ".repeat(level * self.cfg.indent_width)
    }

    fn module(&mut self, m: &VLIRModule) {
        // The port declarations and the body are rendered first because the
        // header depends on them: a `localparam` that nothing references is a
        // Verilator `UNUSEDPARAM` error under `-Wall`, and the emitted text is
        // the only exact answer to "does this module use `WIDTH`?". Deriving it
        // from the IR instead would mean a second traversal that has to stay in
        // step with every future statement and expression variant.
        let ports_text = self.ports_block(m);
        let body_text = {
            let mut e = Emitter {
                out: String::new(),
                cfg: self.cfg,
                port_names: self.port_names.clone(),
            };
            match &m.body {
                VLIRBody::Combinational(c) => e.comb_body(c),
                VLIRBody::Sequential(s) => e.seq_body(s),
                VLIRBody::Structural(st) => e.structural_body(st),
            }
            e.out
        };
        let used = used_localparams(m, &format!("{ports_text}{body_text}"));

        self.emit_param_header(m, &used);
        self.out.push_str(&ports_text);
        self.out.push('\n');
        self.out.push_str(&body_text);
        self.out.push_str("endmodule\n");
    }

    // ── Header ──────────────────────────────────────────────────────────────

    /// `module name #(parameter int N = 8, localparam int WIDTH = 8) (`, or a
    /// bare `module name (` when there is nothing to declare.
    ///
    /// `parameter`s come first and `localparam`s after, so a constant may be
    /// written in terms of a generic parameter but not the reverse — which
    /// matches Rust, where a `const` item cannot mention a function's generics.
    /// SystemVerilog resolves the list left to right, and `used` is already in
    /// dependency order.
    fn emit_param_header(&mut self, m: &VLIRModule, used: &[&VLIRLocalParam]) {
        if m.params.is_empty() && used.is_empty() {
            self.out.push_str(&format!("module {} (\n", m.name));
            return;
        }
        self.out.push_str(&format!("module {} #(\n", m.name));
        let total = m.params.len() + used.len();
        let mut i = 0usize;
        for p in m.params.iter() {
            // Always emit a default so the module is valid standalone (a
            // parameter with no value is a Verilator lint error). Use the
            // source default when known, else a `1` placeholder that an
            // instantiation overrides.
            let default = p.default.unwrap_or(1);
            i += 1;
            let comma = if i == total { "" } else { "," };
            self.out.push_str(&format!(
                "{}parameter int {} = {}{}\n",
                self.indent(1),
                p.name,
                default,
                comma
            ));
        }
        for lp in used {
            i += 1;
            let comma = if i == total { "" } else { "," };
            self.out.push_str(&format!(
                "{}localparam int {} = {}{}\n",
                self.indent(1),
                lp.name,
                lp.value_expr,
                comma
            ));
        }
        self.out.push_str(") (\n");
    }

    /// The port declaration list, `(` … `);`.
    fn ports_block(&self, m: &VLIRModule) -> String {
        let mut out = String::new();
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
            // An array port declares BOTH packed dimensions, outer first:
            // `[ELS-1:0][W-1:0]`. `range_str` returns a trailing space, so the
            // outer range is emitted without one to keep them adjacent.
            let range = match &p.outer_dim {
                Some(outer) => format!("{}{}", range_str(outer).trim_end(), range_str(&p.width)),
                None => range_str(&p.width),
            };
            let comma = if i == last { "" } else { "," };
            out.push_str(&format!(
                "{}{} logic {}{}{}\n",
                self.indent(1),
                dir,
                range,
                p.name,
                comma
            ));
        }
        out.push_str(");\n");
        out
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

        self.mem_decls(&s.memories);

        self.submodules(&s.submodules);

        // Declare intermediate combinational wires used in the pre-edge phases.
        let phase_stmts: Vec<&VLIRStmt> =
            s.comb_phases.iter().flat_map(|p| p.stmts.iter()).collect();
        let mut decls = Vec::new();
        let mut seen = self.port_names.clone();
        for st in phase_stmts {
            collect_wire_decls(std::slice::from_ref(st), &mut decls, &mut seen);
        }
        for (name, width, outer_dim) in &decls {
            // An array wire declares both packed dimensions, outer first — the
            // same shape as an array port.
            let range = match outer_dim {
                Some(o) => format!("{}{}", range_str(o).trim_end(), range_str(width)),
                None => range_str(width),
            };
            self.out.push_str(&format!("{}logic {}{};\n", self.indent(1), range, name));
        }
        if !decls.is_empty() {
            self.out.push('\n');
        }

        // always_comb (one guarded block covering all phases, or a bare block).
        let has_comb = s.comb_phases.iter().any(|p| !p.stmts.is_empty());
        if has_comb {
            self.out.push_str(&format!("{}always_comb begin\n", self.indent(1)));
            // Multi-phase: each pre-edge wire is a *phase-local* combinational
            // temp assigned inside only its own `if (phase_r == K)` guard. Merged
            // into one always_comb that would infer a latch (the value must "hold"
            // in the other phases). These temps are read only in the phase that
            // computes them (cross-phase uses are promoted to registers), so a
            // default at the top drives every path — no latch, same behavior.
            let multi_phase = s.comb_phases.iter().any(|p| p.phase_guard.is_some());
            if multi_phase {
                for (name, _, _) in &decls {
                    self.out.push_str(&format!("{}{} = '0;\n", self.indent(2), name));
                }
            }
            for phase in &s.comb_phases {
                self.comb_phase(phase);
            }
            self.out.push_str(&format!("{}end\n\n", self.indent(1)));
        }

        // Continuous reads of the arrays. Emitted after the combinational block
        // so the address nets they reference are already declared.
        self.mem_read_assigns(&s.memories);

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
        let mut seen = self.port_names.clone();
        collect_wire_decls(stmts, &mut decls, &mut seen);
        for (name, width, outer_dim) in &decls {
            // An array wire declares both packed dimensions, outer first — the
            // same shape as an array port.
            let range = match outer_dim {
                Some(o) => format!("{}{}", range_str(o).trim_end(), range_str(width)),
                None => range_str(width),
            };
            self.out.push_str(&format!("{}logic {}{};\n", self.indent(1), range, name));
        }
        if !decls.is_empty() {
            self.out.push('\n');
        }
    }

    /// `logic [W-1:0] <name> [0:DEPTH-1];` plus the read-port output nets.
    fn mem_decls(&mut self, mems: &[VLIRMemDecl]) {
        for m in mems {
            self.out.push_str(&format!(
                "{}logic {}{} [0:{}];\n",
                self.indent(1),
                range_str(&m.width),
                m.name,
                m.depth - 1
            ));
            for n in &m.read_data_nets {
                self.out.push_str(&format!(
                    "{}logic {}{};\n",
                    self.indent(1),
                    range_str(&n.width),
                    n.data
                ));
            }
        }
        if !mems.is_empty() {
            self.out.push('\n');
        }
        self.mem_inits(mems);
    }

    /// Power-on contents, as an `initial` block per preloaded memory.
    ///
    /// `initial` is how SystemVerilog states what a memory holds before the first
    /// clock edge: Verilator executes it at time 0, and FPGA tools read it to
    /// infer an initialized block RAM. It is deliberately NOT guarded by the
    /// clock, and deliberately blocking (`=`) — this is elaboration-time state,
    /// not a clocked update.
    fn mem_inits(&mut self, mems: &[VLIRMemDecl]) {
        for m in mems {
            let Some(init) = &m.init else { continue };
            self.out.push_str(&format!("{}initial begin\n", self.indent(1)));
            match init {
                VLIRMemInit::Fill { var, value } => {
                    self.out.push_str(&format!(
                        "{}for (int {v} = 0; {v} < {}; {v}++) begin\n",
                        self.indent(2),
                        m.depth,
                        v = var,
                    ));
                    self.out.push_str(&format!(
                        "{}{}[{}] = {};\n",
                        self.indent(3),
                        m.name,
                        var,
                        expr_str(value)
                    ));
                    self.out.push_str(&format!("{}end\n", self.indent(2)));
                }
                VLIRMemInit::Words(words) => {
                    for (i, w) in words.iter().enumerate() {
                        self.out.push_str(&format!(
                            "{}{}[{}] = {};\n",
                            self.indent(2),
                            m.name,
                            i,
                            expr_str(w)
                        ));
                    }
                }
            }
            self.out.push_str(&format!("{}end\n\n", self.indent(1)));
        }
    }

    /// `assign <data> = <value>;` — one per observed read port. The value is a
    /// plain array read for ReadFirst, or a write-forwarding mux for WriteFirst.
    fn mem_read_assigns(&mut self, mems: &[VLIRMemDecl]) {
        let mut any = false;
        for m in mems {
            for n in &m.read_data_nets {
                any = true;
                self.out.push_str(&format!(
                    "{}assign {} = {};\n",
                    self.indent(1),
                    n.data,
                    expr_str(&n.value)
                ));
            }
        }
        if any {
            self.out.push('\n');
        }
    }

    fn reg_decl(&mut self, r: &VLIRRegDecl) {
        self.out.push_str(&format!("{}logic {}{};\n", self.indent(1), range_str(&r.width), r.name));
    }

    // ── Submodules ──────────────────────────────────────────────────────────

    fn submodules(&mut self, subs: &[VLIRSubmoduleInst]) {
        for s in subs {
            // Structural (statement/port) form: every connection is a named port
            // wired to an existing net/port — clocks first, then data ports. No
            // instance-local output wire to declare (the nets are declared by the
            // parent's `structural_body`).
            if !s.clocks.is_empty() || !s.port_nets.is_empty() {
                self.out.push_str(&format!("{}{} {} (\n", self.indent(1), s.module_name, s.inst_name));
                let conns: Vec<(String, String)> = s.clocks.iter()
                    .chain(s.port_nets.iter())
                    .map(|(p, n)| (p.clone(), n.clone()))
                    .collect();
                let last = conns.len().saturating_sub(1);
                for (i, (port, net)) in conns.iter().enumerate() {
                    let comma = if i == last { "" } else { "," };
                    self.out.push_str(&format!("{}.{} ({}){}\n", self.indent(2), port, net, comma));
                }
                self.out.push_str(&format!("{});\n\n", self.indent(1)));
                continue;
            }

            // Legacy expression form: a single combinational output wire the
            // caller reads. Declare the instance's output wire immediately before
            // the instance, then wire inputs + the output port.
            self.out.push_str(&format!(
                "{}logic {}{};\n",
                self.indent(1),
                range_str(&s.output_width),
                s.output_wire
            ));
            self.out.push_str(&format!("{}{} {} (\n", self.indent(1), s.module_name, s.inst_name));
            for (port, val) in &s.inputs {
                self.out.push_str(&format!("{}.{} ({}),\n", self.indent(2), port, expr_str(val)));
            }
            let out_port = s.output_port.as_deref().unwrap_or("out");
            self.out.push_str(&format!("{}.{} ({})\n", self.indent(2), out_port, s.output_wire));
            self.out.push_str(&format!("{});\n\n", self.indent(1)));
        }
    }

    // ── Structural body ───────────────────────────────────────────────────────

    fn structural_body(&mut self, st: &VLIRStructuralBody) {
        // Internal nets wiring children together.
        for (name, width) in &st.nets {
            self.out.push_str(&format!("{}logic {}{};\n", self.indent(1), range_str(width), name));
        }
        if !st.nets.is_empty() {
            self.out.push('\n');
        }
        self.submodules(&st.submodules);
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
            VLIRFFStmt::MemAssign { mem, addr, value } => {
                self.out.push_str(&format!(
                    "{}{}[{}] <= {};\n",
                    self.indent(level),
                    mem,
                    expr_str(addr),
                    expr_str(value)
                ));
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
            VLIRStmt::PortAssign { port_name, value, .. } => {
                self.out.push_str(&format!(
                    "{}{} = {};\n",
                    self.indent(level),
                    port_name,
                    expr_str(value)
                ));
            }
            // `always_comb`: the post-edge form. The `edge_condition` copy was
            // already taken by `split_output_reg` for the `always_ff` half.
            VLIRStmt::If { condition, then_stmts, else_stmts, .. } => {
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
            VLIRStmt::Case { selector, arms, default, .. } => {
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
                match default {
                    Some(d) => {
                        self.out
                            .push_str(&format!("{}default: begin\n", self.indent(level + 1)));
                        for st in d {
                            self.comb_stmt(st, level + 2);
                        }
                        self.out.push_str(&format!("{}end\n", self.indent(level + 1)));
                    }
                    // An EMPTY default rather than none: Verilator runs with `-Wall`,
                    // where an incompletely-covered case is CASEINCOMPLETE and fails
                    // the build. A `match` with no wildcard arm reaches here — an
                    // extracted `match pc` whose comb work lives in only some states
                    // is the routine one — and doing nothing on the unlisted values is
                    // exactly what the missing arm already meant: every net this block
                    // drives is assigned its default at the top of the `always_comb`.
                    None => {
                        self.out
                            .push_str(&format!("{}default: begin\n", self.indent(level + 1)));
                        self.out.push_str(&format!("{}end\n", self.indent(level + 1)));
                    }
                }
                self.out.push_str(&format!("{}endcase\n", self.indent(level)));
            }
            VLIRStmt::ForLoop { var, start, end, body } => {
                self.out.push_str(&format!(
                    "{}for (int {v} = {}; {v} < {}; {v}++) begin\n",
                    self.indent(level),
                    loop_bound_str(start),
                    loop_bound_str(end),
                    v = var,
                ));
                for st in body {
                    self.comb_stmt(st, level + 1);
                }
                self.out.push_str(&format!("{}end\n", self.indent(level)));
            }
            VLIRStmt::IndexAssign { base, index, value } => {
                self.out.push_str(&format!(
                    "{}{}[{}] = {};\n",
                    self.indent(level),
                    base,
                    loop_bound_str(index),
                    expr_str(value),
                ));
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
    out: &mut Vec<(String, Width, Option<Width>)>,
    seen: &mut HashSet<String>,
) {
    for s in stmts {
        match s {
            VLIRStmt::WireAssign { name, width, outer_dim, .. } => {
                if seen.insert(name.clone()) {
                    out.push((name.clone(), width.clone(), outer_dim.clone()));
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
            // A bit-assign targets an already-declared signal.
            VLIRStmt::IndexAssign { .. } => {}
        }
    }
}

// ── Width / expression formatting (the single width->text route, D1a) ────────

/// The one place a width becomes Verilog text — so a future symbolic width
/// (`[N-1:0]`) is a change here only.

/// The `localparam`s the emitted module actually references, in dependency
/// order.
///
/// `text` is the rendered port list plus body. A constant counts as used if its
/// name appears there as a whole identifier, or if a constant that is itself
/// used mentions it in its initializer (`MOD = 1 << PTR_W` keeps `PTR_W` alive).
/// The closure runs to a fixpoint, so a chain of any depth resolves.
///
/// Anything not reached is dropped: Verilator's `UNUSEDPARAM` makes an unused
/// `localparam` a hard error under `-Wall`, so emitting the module's whole
/// file-scope const environment is not an option.
fn used_localparams<'a>(m: &'a VLIRModule, text: &str) -> Vec<&'a VLIRLocalParam> {
    let mut used: HashSet<String> = m
        .localparams
        .iter()
        .filter(|lp| crate::file_consts::contains_ident(text, &lp.name))
        .map(|lp| lp.name.clone())
        .collect();

    loop {
        let mut grew = false;
        for lp in &m.localparams {
            if !used.contains(&lp.name) {
                continue;
            }
            for other in &m.localparams {
                if !used.contains(&other.name)
                    && crate::file_consts::contains_ident(&lp.value_expr, &other.name)
                {
                    used.insert(other.name.clone());
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }

    m.localparams.iter().filter(|lp| used.contains(&lp.name)).collect()
}

fn range_str(w: &Width) -> String {
    match w {
        Width::Concrete(1) => String::new(),
        Width::Concrete(n) => format!("[{}:0] ", n - 1),
        // A parametric width `N` renders as `[N-1:0]`.
        Width::Param(name) => format!("[{name}-1:0] "),
    }
}

// (DynBit expr handled in expr_str below.)

/// A `for`-loop bound renders as a plain integer, not a width-sized literal —
/// the loop variable is a 32-bit `int`, so `64'd0` would width-truncate. A
/// literal bound becomes its decimal value; anything else (e.g. a `parameter N`)
/// uses the normal expression form.
fn loop_bound_str(e: &VLIRExpr) -> String {
    match e {
        VLIRExpr::Lit { value, .. } => value.to_string(),
        other => expr_str(other),
    }
}

fn lit_str(width: &Width, value: u128) -> String {
    match width {
        Width::Concrete(1) => format!("1'b{}", value & 1),
        Width::Concrete(n) => format!("{}'d{}", n, value),
        // A parameter can't be a sized literal's width (`N'd0` is illegal SV), so
        // use context-sized forms: `'0` all-zeros, `'1` all-ones, else an unsized
        // decimal that the assignment context sizes.
        Width::Param(_) => match value {
            0 => "'0".to_string(),
            v if v == u128::MAX => "'1".to_string(),
            v => v.to_string(),
        },
    }
}

/// Whether `[..]` may legally select from this expression's rendered text —
/// SV allows selects only on (possibly memory-indexed) identifiers, never on
/// a parenthesized expression.
fn select_legal(e: &VLIRExpr) -> bool {
    matches!(e, VLIRExpr::Var(_) | VLIRExpr::MemIndex { .. })
}

/// Fully parenthesized expression text — never rely on operator precedence.
fn expr_str(e: &VLIRExpr) -> String {
    match e {
        VLIRExpr::Var(name) => name.clone(),
        VLIRExpr::Lit { width, value } => lit_str(width, *value),
        VLIRExpr::BinOp { left, op, right } => {
            // SystemVerilog's `>>` is LOGICAL even on a signed operand; the
            // arithmetic shift Rust's `as i32 >> k` means needs `>>>` (and a
            // signed left operand, which the SignCast wrapper supplies). This is
            // the signedness ledger's `sign_extend_via_cast` — sign extension
            // silently became zero extension before this arm existed.
            let op_str = match (op, left.as_ref()) {
                (VLIRBinOp::Shr, VLIRExpr::SignCast { signed: true, .. }) => ">>>",
                _ => binop_str(*op),
            };
            format!("({} {} {})", expr_str(left), op_str, expr_str(right))
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
            if select_legal(expr) {
                if high == low {
                    format!("{}[{}]", expr_str(expr), high)
                } else {
                    format!("{}[{}:{}]", expr_str(expr), high, low)
                }
            } else {
                // SV forbids a select on a non-identifier expression (the
                // forwarding substitution can put a compound here, e.g.
                // `(n + 8'd1)[0]`); `W'(expr >> low)` selects the same bits.
                let width = high - low + 1;
                if *low == 0 {
                    format!("{}'({})", width, expr_str(expr))
                } else {
                    format!("{}'({} >> {})", width, expr_str(expr), low)
                }
            }
        }
        VLIRExpr::DynBit { base, index } => {
            if select_legal(base) {
                format!("{}[{}]", expr_str(base), loop_bound_str(index))
            } else {
                format!("1'({} >> {})", expr_str(base), loop_bound_str(index))
            }
        }
        VLIRExpr::MemIndex { mem, addr } => format!("{}[{}]", mem, expr_str(addr)),
        // Signedness reinterpretation. `$signed`/`$unsigned` change the TYPE of
        // the expression, not its bits: a comparison is signed iff every operand
        // is signed, so the CHIR lowering keeps the wrapper on both compare
        // operands and strips it wherever signedness is unobservable.
        VLIRExpr::SignCast { signed, expr } => format!(
            "{}({})",
            if *signed { "$signed" } else { "$unsigned" },
            expr_str(expr)
        ),
        // `width'(expr)` — SV width-cast; the size may be a parameter.
        VLIRExpr::Resize { expr, width } => {
            let size = match width {
                Width::Concrete(n) => n.to_string(),
                Width::Param(name) => name.clone(),
            };
            format!("{}'({})", size, expr_str(expr))
        }
    }
}

fn binop_str(op: VLIRBinOp) -> &'static str {
    match op {
        VLIRBinOp::Add => "+",
        VLIRBinOp::Sub => "-",
        VLIRBinOp::Mul => "*",
        VLIRBinOp::Rem => "%",
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
            localparams: vec![],
            ports: vec![VLIRPort {
                name: "out".to_string(),
                outer_dim: None,
                direction: VLIRPortDir::Output,
                kind: VLIRPortKind::Logic,
                width: Width::Concrete(8),
                registered: false,
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
                        outer_dim: None,
                        value: VLIRExpr::Var("i".to_string()),
                    }],
                }],
                output_assigns: vec![],
            }),
        };
        let sv = emit_verilog(&module, &EmitConfig::default());
        assert!(
            sv.contains("for (int i = 0; i < N; i++) begin"),
            "expected SV for loop, got:\n{sv}"
        );
        assert!(sv.contains("acc = i;"), "expected loop body, got:\n{sv}");
    }
}
