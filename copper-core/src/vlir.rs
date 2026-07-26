//! Verilog-Legal IR (VLIR) — Phase D output.
//!
//! VLIR is a thin legalization layer over SHIR that maps 1:1 onto valid
//! SystemVerilog syntax. All names are legalized, all literals carry concrete
//! widths, muxes are ternaries, and case-expressions are lifted to statements.
//! After Phase D the only remaining work is mechanical text serialization
//! (Phase E, `copper-codegen/src/emit.rs`).
//!
//! See `design_docs/VLIR_DESIGN.md` for the full contract.

use crate::chir::Width;

// ── Toolchain profile ───────────────────────────────────────────────────────

/// Target-toolchain profile. Consulted only in Phase D/E. Defaults to
/// `Verilator` — the only verification path today (see roadmap decision #3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolchainProfile {
    #[default]
    Verilator,
    Generic,
    Yosys,
}

// ── Module ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct VLIRModule {
    pub name: String,
    /// SystemVerilog `parameter`s. Empty until the M2 parametric work.
    pub params: Vec<VLIRParam>,
    pub ports: Vec<VLIRPort>,
    pub body: VLIRBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VLIRParam {
    pub name: String,
    pub default: Option<usize>,
}

#[derive(Debug)]
pub struct VLIRPort {
    pub name: String,
    pub direction: VLIRPortDir,
    pub kind: VLIRPortKind,
    /// Resolved bit width. `Width` (not raw `usize`) so a future parametric
    /// port (`[N-1:0]`) is a new variant, not a field re-type.
    pub width: Width,
    /// Registered output port (`RegOut<T,D>`) — driven from `always_ff`. See
    /// `CHIRPort::registered`.
    pub registered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VLIRPortDir {
    Input,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VLIRPortKind {
    Clock,
    Logic,
}

#[derive(Debug)]
pub enum VLIRBody {
    Combinational(VLIRCombBody),
    Sequential(VLIRSeqBody),
}

// ── Combinational body ──────────────────────────────────────────────────────

#[derive(Debug)]
pub struct VLIRCombBody {
    pub submodules: Vec<VLIRSubmoduleInst>,
    /// `always_comb` block contents, in order.
    pub comb_stmts: Vec<VLIRStmt>,
    /// `assign out = <expr>;` — one per output port.
    pub output_assigns: Vec<VLIRContinuousAssign>,
}

// ── Sequential body ─────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct VLIRSeqBody {
    pub clock: String,
    pub reg_decls: Vec<VLIRRegDecl>,
    pub submodules: Vec<VLIRSubmoduleInst>,
    /// One `always_comb` block per phase (pre-edge wires + conditional port
    /// drives). Single-phase modules have exactly one, with `phase_guard: None`.
    pub comb_phases: Vec<VLIRCombPhase>,
    /// Single `always_ff` block — all register updates.
    pub always_ff: VLIRAlwaysFF,
    pub output_assigns: Vec<VLIRContinuousAssign>,
}

#[derive(Debug)]
pub struct VLIRRegDecl {
    pub name: String,
    pub width: Width,
}

#[derive(Debug)]
pub struct VLIRSubmoduleInst {
    pub inst_name: String,
    pub module_name: String,
    pub inputs: Vec<(String, VLIRExpr)>,
    pub output_wire: String,
    pub output_width: Width,
}

#[derive(Debug)]
pub struct VLIRCombPhase {
    /// `None` for single-phase; `Some(phase_r == K)` for multi-phase.
    pub phase_guard: Option<VLIRExpr>,
    pub stmts: Vec<VLIRStmt>,
}

#[derive(Debug)]
pub struct VLIRAlwaysFF {
    pub clock: String,
    pub stmts: Vec<VLIRFFStmt>,
}

#[derive(Debug)]
pub struct VLIRContinuousAssign {
    pub target: String,
    pub value: VLIRExpr,
}

// ── Statement models ────────────────────────────────────────────────────────
//
// Two separate statement types so Phase E cannot emit a blocking assign inside
// `always_ff` or a non-blocking assign inside `always_comb`. The type system
// enforces the assignment policy (VERILOG_OUTPUT_STANDARDS §4).

/// Statements inside `always_comb` blocks (blocking `=`).
#[derive(Debug)]
pub enum VLIRStmt {
    WireAssign {
        name: String,
        width: Width,
        value: VLIRExpr,
    },
    /// Drive an output port from within combinational logic (`out = <expr>;`).
    PortAssign {
        port_name: String,
        value: VLIRExpr,
    },
    If {
        condition: VLIRExpr,
        then_stmts: Vec<VLIRStmt>,
        else_stmts: Option<Vec<VLIRStmt>>,
    },
    Case {
        selector: VLIRExpr,
        arms: Vec<VLIRCaseArm>,
        default: Option<Vec<VLIRStmt>>,
    },
    /// `for (int <var> = <start>; <var> < <end>; <var>++) begin … end`.
    ForLoop {
        var: String,
        start: VLIRExpr,
        end: VLIRExpr,
        body: Vec<VLIRStmt>,
    },
    /// `base[index] = value;` — single-bit assignment.
    IndexAssign {
        base: String,
        index: VLIRExpr,
        value: VLIRExpr,
    },
}

/// Statements inside `always_ff` blocks (non-blocking `<=` only).
#[derive(Debug)]
pub enum VLIRFFStmt {
    NonBlockingAssign {
        target: String,
        value: VLIRExpr,
    },
    If {
        condition: VLIRExpr,
        then_stmts: Vec<VLIRFFStmt>,
        else_stmts: Option<Vec<VLIRFFStmt>>,
    },
    Case {
        selector: VLIRExpr,
        arms: Vec<VLIRFFCaseArm>,
        default: Option<Vec<VLIRFFStmt>>,
    },
}

#[derive(Debug)]
pub struct VLIRCaseArm {
    pub selector_value: VLIRExpr,
    pub stmts: Vec<VLIRStmt>,
}

#[derive(Debug)]
pub struct VLIRFFCaseArm {
    pub selector_value: VLIRExpr,
    pub stmts: Vec<VLIRFFStmt>,
}

// ── Expression model ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum VLIRExpr {
    Var(String),
    /// Width-explicit literal, e.g. `8'd5`, `1'b0`.
    Lit { width: Width, value: u128 },
    BinOp {
        left: Box<VLIRExpr>,
        op: VLIRBinOp,
        right: Box<VLIRExpr>,
    },
    UnOp {
        op: VLIRUnOp,
        expr: Box<VLIRExpr>,
    },
    /// `cond ? then_val : else_val`
    Ternary {
        cond: Box<VLIRExpr>,
        then_val: Box<VLIRExpr>,
        else_val: Box<VLIRExpr>,
    },
    /// `{a, b, c}`
    Concat(Vec<VLIRExpr>),
    /// `expr[high:low]`
    Slice {
        expr: Box<VLIRExpr>,
        high: usize,
        low: usize,
    },
    /// `base[index]` — single-bit select at a dynamic index.
    DynBit {
        base: Box<VLIRExpr>,
        index: Box<VLIRExpr>,
    },
    /// `width'(expr)` — width-cast (legal with a symbolic width).
    Resize {
        expr: Box<VLIRExpr>,
        width: Width,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VLIRBinOp {
    Add, Sub, Mul, Rem,
    BitAnd, BitOr, BitXor,
    Shl, Shr,
    Eq, Neq, Lt, Lte, Gt, Gte,
    LogicalAnd, LogicalOr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VLIRUnOp {
    BitNot, LogicalNot, Neg,
    ReductionAnd, ReductionOr, ReductionXor,
}
