use crate::frontend_ir::SourceSpan;

// ── Top-level module ─────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct CHIRModule {
    pub name: String,
    /// Module-level parameters (SystemVerilog `parameter`s). Empty until the
    /// parametric-module work in M2; present now so parameters are additive
    /// rather than a wide `usize -> Width` retrofit later. See
    /// `TRANSPILATION_ROADMAP.md` decision #4 / task D1a.
    pub params: Vec<ModuleParam>,
    pub ports: Vec<CHIRPort>,
    pub body: CHIRBody,
    pub span: SourceSpan,
}

/// A module-level parameter, e.g. `parameter N = 8`. `default` is the concrete
/// value bound at the instantiation the transpiler was invoked on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleParam {
    pub name: String,
    pub default: Option<usize>,
}

// ── Ports ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CHIRPort {
    pub name: String,
    pub direction: CHIRPortDir,
    pub kind: CHIRPortKind,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CHIRPortDir {
    Input,
    Output,
}

#[derive(Debug, Clone)]
pub enum CHIRPortKind {
    /// Clock — not a data signal; carries domain name.
    Clock { domain: String },
    /// Data — a hardware-typed signal of known width.
    Data { ty: CHIRType },
}

// ── Type system ───────────────────────────────────────────────────────────────

/// Hardware-native types only. All Rust-runtime types are stripped in Phase B.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CHIRType {
    UInt { width: Width },
    SInt { width: Width },
    Bool,
}

/// The bit width of a hardware value.
///
/// Today only `Concrete` is constructed — every current example is transpiled
/// at concrete widths. The enum exists now so symbolic widths (`Bits<N>`) can be
/// added in M2 as a new variant without re-typing the field across CHIR/SHIR/VLIR
/// and their tests. See `TRANSPILATION_ROADMAP.md` decision #4 / task D1a.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Width {
    /// A fully-resolved bit width.
    Concrete(usize),
    // M2: Param(String) — a `parameter N`
    // M2: Sub(Box<Width>, usize) — e.g. `N - 1`
}

impl Width {
    /// The resolved width. Panics on a non-concrete width — callers that can
    /// legitimately encounter symbolic widths (M2+) must match explicitly.
    pub fn concrete(&self) -> usize {
        match self {
            Width::Concrete(n) => *n,
        }
    }
}

impl From<usize> for Width {
    fn from(n: usize) -> Self {
        Width::Concrete(n)
    }
}

// ── Module body ───────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum CHIRBody {
    Combinational(CHIRCombBody),
    Sequential(CHIRSeqBody),
}

/// Combinational module body.
/// `stmts` holds Wire declarations for intermediates and PortWrite for outputs,
/// in source order.
#[derive(Debug, Clone)]
pub struct CHIRCombBody {
    pub submodules: Vec<CHIRSubmoduleInst>,
    pub stmts: Vec<CHIRStmt>,
}

/// Sequential module body: registers, submodule instances, and loop body.
#[derive(Debug, Clone)]
pub struct CHIRSeqBody {
    /// Name of the clock parameter this module is clocked on.
    pub clock: String,
    /// State registers — variables that live across a tick boundary.
    pub registers: Vec<CHIRRegDecl>,
    /// `#[hardware]` submodule instantiations used in this module.
    pub submodules: Vec<CHIRSubmoduleInst>,
    /// The infinite loop body containing one or more AwaitTick boundaries.
    pub loop_body: Vec<CHIRStmt>,
}

#[derive(Debug, Clone)]
pub struct CHIRRegDecl {
    pub name: String,
    pub ty: CHIRType,
    /// Initial value for simulation. Not emitted in synthesis output.
    pub init: Option<CHIRLit>,
    pub span: SourceSpan,
}

// ── Submodule instantiation ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CHIRSubmoduleInst {
    pub inst_name: String,
    pub module_name: String,
    pub inputs: Vec<(String, CHIRExpr)>,
    pub output_wire: String,
    pub output_ty: CHIRType,
    pub span: SourceSpan,
}

// ── Statement model ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum CHIRStmt {
    /// Declare a combinational wire value. Does not cross a tick boundary.
    Wire {
        name: String,
        ty: CHIRType,
        value: CHIRExpr,
        span: SourceSpan,
    },

    /// Assign to a register. Phase B guarantees this targets a register name.
    Assign {
        target: String,
        value: CHIRExpr,
        span: SourceSpan,
    },

    /// Drive an output port — from `out_port.write(expr)`.
    /// Can appear anywhere: flat, inside If, inside Match.
    PortWrite {
        port_name: String,
        value: CHIRExpr,
        span: SourceSpan,
    },

    /// Clock-edge boundary — from clk.tick().await.
    AwaitTick {
        clock: String,
        span: SourceSpan,
    },

    /// Conditional branch.
    If {
        condition: CHIRExpr,
        then_body: Vec<CHIRStmt>,
        else_body: Option<Vec<CHIRStmt>>,
        span: SourceSpan,
    },

    /// Pattern match used as a statement (for side effects / register assignments).
    Match {
        scrutinee: CHIRExpr,
        arms: Vec<CHIRMatchArm>,
        span: SourceSpan,
    },
}

impl CHIRStmt {
    pub fn span(&self) -> &SourceSpan {
        match self {
            CHIRStmt::Wire { span, .. } => span,
            CHIRStmt::Assign { span, .. } => span,
            CHIRStmt::PortWrite { span, .. } => span,
            CHIRStmt::AwaitTick { span, .. } => span,
            CHIRStmt::If { span, .. } => span,
            CHIRStmt::Match { span, .. } => span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CHIRMatchArm {
    pub patterns: Vec<CHIRPattern>,
    pub guard: Option<CHIRExpr>,
    pub body: Vec<CHIRStmt>,
    pub span: SourceSpan,
}

/// Patterns preserved from source.
#[derive(Debug, Clone)]
pub enum CHIRPattern {
    Lit(CHIRLit),
    Wildcard,
    Tuple(Vec<CHIRPattern>),
    EnumVariant { name: String, inner: Option<Box<CHIRPattern>> },
}

// ── Expression model ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum CHIRExpr {
    Var(String),
    Lit(CHIRLit),
    BinOp {
        left: Box<CHIRExpr>,
        op: CHIRBinOp,
        right: Box<CHIRExpr>,
    },
    UnOp {
        op: CHIRUnOp,
        expr: Box<CHIRExpr>,
    },
    Mux {
        cond: Box<CHIRExpr>,
        then_val: Box<CHIRExpr>,
        else_val: Box<CHIRExpr>,
    },
    Case {
        scrutinee: Box<CHIRExpr>,
        arms: Vec<CHIRCaseArm>,
        default: Option<Box<CHIRExpr>>,
    },
    Concat(Vec<CHIRExpr>),
    Slice {
        expr: Box<CHIRExpr>,
        high: usize,
        low: usize,
    },
}

#[derive(Debug, Clone)]
pub struct CHIRCaseArm {
    pub pattern: CHIRPattern,
    pub guard: Option<CHIRExpr>,
    pub value: CHIRExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CHIRLit {
    pub ty: CHIRType,
    pub value: u128,
}

#[derive(Debug, Clone)]
pub enum CHIRBinOp {
    Add { wrapping: bool },
    Sub { wrapping: bool },
    Mul { wrapping: bool },
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
    LogicalAnd,
    LogicalOr,
}

#[derive(Debug, Clone)]
pub enum CHIRUnOp {
    BitNot,
    LogicalNot,
    Neg,
    ReductionAnd,
    ReductionOr,
    ReductionXor,
}

// ── Error model ───────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum CHIRLowerError {
    UnsupportedConstruct {
        description: String,
        span: SourceSpan,
        suggested_rewrite: Option<String>,
    },
    UnresolvableType {
        ty_text: String,
        span: SourceSpan,
    },
    RegisterWireConflict {
        name: String,
        span: SourceSpan,
    },
    TickInsideBranch {
        span: SourceSpan,
    },
    AmbiguousWidth {
        span: SourceSpan,
    },
}

impl CHIRLowerError {
    pub fn span(&self) -> &SourceSpan {
        match self {
            CHIRLowerError::UnsupportedConstruct { span, .. } => span,
            CHIRLowerError::UnresolvableType { span, .. } => span,
            CHIRLowerError::RegisterWireConflict { span, .. } => span,
            CHIRLowerError::TickInsideBranch { span } => span,
            CHIRLowerError::AmbiguousWidth { span } => span,
        }
    }
}

impl std::fmt::Display for CHIRLowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let span = self.span();
        write!(f, "{}:{}: ", span.start_line, span.start_col)?;
        match self {
            CHIRLowerError::UnsupportedConstruct { description, .. } =>
                write!(f, "unsupported construct: {}", description),
            CHIRLowerError::UnresolvableType { ty_text, .. } =>
                write!(f, "cannot resolve type '{}' to a hardware type", ty_text),
            CHIRLowerError::RegisterWireConflict { name, .. } =>
                write!(f, "variable '{}' used as both register and wire", name),
            CHIRLowerError::TickInsideBranch { .. } =>
                write!(f, "clk.tick().await inside a conditional branch is not supported"),
            CHIRLowerError::AmbiguousWidth { .. } =>
                write!(f, "cannot infer bit width; add an explicit type annotation"),
        }
    }
}
