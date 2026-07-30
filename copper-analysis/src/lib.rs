//! Shared compile-time control/liveness analysis for Copper (c2 architecture).
//!
//! **Why this crate exists (gate G6 in `design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md`).**
//! The c2 decision is that the sim macro (`copper-macros`) and the transpiler
//! (`copper-codegen`) must consume *one* authoritative control-flow analysis —
//! register inference (T1), read-timing facts (item 3), and the FSM report all
//! need the same CFG, and two analyses that must agree is itself a correctness
//! hazard. The open feasibility question was whether the `copper-macros`
//! **proc-macro** can depend on such a shared crate without a dependency cycle or
//! a heavy compile-time cost.
//!
//! **Answer (this crate demonstrates it): yes.** The analysis keys off
//! [`syn::ItemFn`] — the representation the proc-macro already receives *and* the
//! one the transpiler already builds (`copper_codegen::…::capture_frontend_ir`
//! takes a `&syn::ItemFn`; `transpile_source` is `parse_file` → `ItemFn`). So both
//! front-ends already hold the analysis input; no front-end unification is needed
//! for the analysis itself. And because `copper-core` is a leaf crate, a crate
//! depending only on `copper-core` + `syn` (both of which `copper-macros` already
//! pulls in) introduces **no cycle** and **no new heavy transitive dependency**.
//!
//! **Scope of THIS module (a feasibility slice, not item 2).**
//! [`infer_registers`] implements the minimal, principled control-flow criterion
//! that suffices to drive one multi-tick FSM (`mac_fsm`) end-to-end and match its
//! independent hand-written reference SystemVerilog reg-for-reg. The production
//! item-2 pass generalizes this to full backward liveness over a real CFG (see
//! [`registers_from_fir`]).

use std::collections::BTreeSet;

use syn::visit::Visit;
use syn::{BinOp, Expr, ItemFn, Pat, Stmt};

/// Infer the synthesizable **register set** of a sequential hardware module from
/// its control flow.
///
/// Criterion (minimal, item-2 generalizes it): a local binding declared *before*
/// the module's top-level `loop` and **reassigned inside** that loop is a
/// register — its value must survive the clock edge (`clk.tick().await`) to the
/// next iteration. A pre-loop binding that is only *read* in the loop is a
/// constant/wire, not a register (e.g. `lfsr`'s `xor_mask`).
///
/// Returns register names sorted (stable output for structural comparison).
///
/// Not yet handled here (item 2's job): registers *born inside* the loop and live
/// across an interior `.await` (e.g. `mac_pipeline`'s `product`/`c_s`/`sum`), and
/// bit-indexed partial assignments. This slice targets the pre-loop-state FSM
/// shape, whose independent SV golden declares its registers explicitly.
pub fn infer_registers(f: &ItemFn) -> Vec<String> {
    let Some(loop_body) = top_level_loop_body(f) else {
        return Vec::new();
    };

    // Candidate state: bindings declared before the top-level loop.
    let pre_loop: BTreeSet<String> = pre_loop_bindings(f);

    // Of those, the ones assigned inside the loop cross the tick → registers.
    let mut assigned = AssignCollector::default();
    for stmt in &loop_body {
        assigned.visit_stmt(stmt);
    }

    pre_loop
        .intersection(&assigned.names)
        .cloned()
        .collect::<Vec<_>>()
}

/// Production entry point (item 2): infer registers from the shared frontend IR
/// via full backward liveness over the CFG. Stubbed for the G6 slice — present to
/// pin the intended type dependency on `copper-core`'s IR, confirming the shared
/// crate compiles against the same FIR both front-ends build.
pub fn registers_from_fir(_ir: &copper_core::frontend_ir::FrontendModuleIR) -> Vec<String> {
    unimplemented!("item 2: backward liveness over the CFG built from the shared FIR")
}

/// The statement list of the module's top-level `loop { .. }`, if any.
fn top_level_loop_body(f: &ItemFn) -> Option<Vec<Stmt>> {
    f.block.stmts.iter().find_map(|s| match s {
        Stmt::Expr(Expr::Loop(l), _) => Some(l.body.stmts.clone()),
        _ => None,
    })
}

/// Names bound by `let [mut] name [: T] = ..` statements appearing before the
/// top-level loop.
fn pre_loop_bindings(f: &ItemFn) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for stmt in &f.block.stmts {
        match stmt {
            Stmt::Expr(Expr::Loop(_), _) => break, // reached the loop
            Stmt::Local(local) => {
                if let Some(name) = binding_ident(&local.pat) {
                    names.insert(name);
                }
            }
            _ => {}
        }
    }
    names
}

/// The identifier bound by a simple `let` pattern (`x` or `x: T`), if it is one.
fn binding_ident(pat: &Pat) -> Option<String> {
    match pat {
        Pat::Ident(pi) => Some(pi.ident.to_string()),
        Pat::Type(pt) => binding_ident(&pt.pat),
        _ => None,
    }
}

/// Collects the names of variables that are assignment *targets* — both plain
/// `x = ..` and compound `x += ..` etc. — anywhere it visits.
#[derive(Default)]
struct AssignCollector {
    names: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for AssignCollector {
    fn visit_expr_assign(&mut self, node: &'ast syn::ExprAssign) {
        if let Some(name) = simple_path_ident(&node.left) {
            self.names.insert(name);
        }
        syn::visit::visit_expr_assign(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        // syn 2.0 models compound assignment (`+=`, `-=`, …) as a binary op.
        if is_assign_op(&node.op) {
            if let Some(name) = simple_path_ident(&node.left) {
                self.names.insert(name);
            }
        }
        syn::visit::visit_expr_binary(self, node);
    }
}

/// The single identifier of a bare path expression (`x`), else `None` (skips
/// `x[i]`, `x.field`, etc. — bit/field assigns to an already-counted register).
fn simple_path_ident(e: &Expr) -> Option<String> {
    if let Expr::Path(p) = e {
        if p.path.segments.len() == 1 && p.qself.is_none() {
            return Some(p.path.segments[0].ident.to_string());
        }
    }
    None
}

fn is_assign_op(op: &BinOp) -> bool {
    matches!(
        op,
        BinOp::AddAssign(_)
            | BinOp::SubAssign(_)
            | BinOp::MulAssign(_)
            | BinOp::DivAssign(_)
            | BinOp::RemAssign(_)
            | BinOp::BitXorAssign(_)
            | BinOp::BitAndAssign(_)
            | BinOp::BitOrAssign(_)
            | BinOp::ShlAssign(_)
            | BinOp::ShrAssign(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `mac_fsm` coding (mirrors `tests/fixtures/mac_fsm_dut.rs`): a single-tick
    /// FSM whose persistent state is four pre-loop locals.
    const MAC_FSM_SRC: &str = r#"
        #[hardware(sequential)]
        async fn mac_fsm(
            clk: Clock<MainClk>,
            a: In<Bits<8>, MainClk>,
            b: In<Bits<8>, MainClk>,
            c: In<Bits<8>, MainClk>,
            out: RegOut<Bits<8>, MainClk>,
        ) {
            let mut stage = Stage::Load;
            let mut product: Bits<8> = Bits::from_lit::<0>();
            let mut c_latch: Bits<8> = Bits::from_lit::<0>();
            let mut result: Bits<8> = Bits::from_lit::<0>();
            loop {
                match stage {
                    Stage::Load => {
                        product = a.read() * b.read();
                        c_latch = c.read();
                        stage = Stage::Mul;
                    }
                    Stage::Mul => {
                        result = product.clone() + c_latch.clone();
                        stage = Stage::Out;
                    }
                    Stage::Out => {
                        out.write(result.clone());
                        stage = Stage::Load;
                    }
                }
                clk.tick().await;
            }
        }
    "#;

    fn parse(src: &str) -> ItemFn {
        syn::parse_str(src).expect("parse ItemFn")
    }

    #[test]
    fn infers_mac_fsm_register_set() {
        let regs = infer_registers(&parse(MAC_FSM_SRC));
        assert_eq!(
            regs,
            vec![
                "c_latch".to_string(),
                "product".to_string(),
                "result".to_string(),
                "stage".to_string()
            ],
            "inferred register set does not match the mac_fsm state"
        );
    }

    /// A pre-loop constant that is only read in the loop is a wire, not a register.
    #[test]
    fn constant_not_counted_as_register() {
        let src = r#"
            #[hardware(sequential)]
            async fn m(clk: Clock<C>, o: Out<Bits<32>, C>) {
                let mask = Bits::from_u32(7);
                let mut state = Bits::from_u32(1);
                loop {
                    state = state ^ mask;
                    o.write(state);
                    clk.tick().await;
                }
            }
        "#;
        let regs = infer_registers(&parse(src));
        assert_eq!(regs, vec!["state".to_string()], "mask is a constant, not a register");
    }

    /// Structural reg-for-reg check against the INDEPENDENT hand-written reference
    /// SystemVerilog (`tests/fixtures/timing_probe_sv/mac_fsm.sv`) — this is gate
    /// G2's definition of register-inference correctness, on the G6 slice. Parses
    /// the reference's internal `reg` declarations (excluding the `output reg`,
    /// which is Copper's `RegOut` output-port axis, not internal state) and asserts
    /// they equal the inferred set.
    #[test]
    fn inferred_set_matches_independent_reference_sv() {
        let sv_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/timing_probe_sv/mac_fsm.sv");
        let sv = std::fs::read_to_string(&sv_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", sv_path.display()));

        let mut sv_regs: BTreeSet<String> = BTreeSet::new();
        for line in sv.lines() {
            let t = line.trim();
            // Internal registers only: `reg [..] a, b, c;`. Skip `output reg ..`.
            if let Some(rest) = t.strip_prefix("reg ") {
                let after_width = rest.rsplit(']').next().unwrap_or(rest);
                for name in after_width.trim_end_matches(';').split(',') {
                    let name = name.trim();
                    if !name.is_empty() {
                        sv_regs.insert(name.to_string());
                    }
                }
            }
        }

        let inferred: BTreeSet<String> =
            infer_registers(&parse(MAC_FSM_SRC)).into_iter().collect();

        assert_eq!(
            inferred, sv_regs,
            "inferred register set does not structurally match the independent \
             reference SV mac_fsm.sv"
        );
    }
}
