use crate::chir::{CHIRBinOp, CHIRType, CHIRUnOp, ModuleParam, Width};
use crate::frontend_ir::SourceSpan;

// ── Module ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct SHIRModule {
    pub name: String,
    /// Module-level parameters (const generics), carried through from CHIR to
    /// VLIR emission (M2).
    pub params: Vec<ModuleParam>,
    pub ports: Vec<SHIRPort>,
    pub body: SHIRBody,
    pub span: SourceSpan,
}

#[derive(Debug)]
pub struct SHIRPort {
    pub name: String,
    pub direction: SHIRPortDir,
    pub kind: SHIRPortKind,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SHIRPortDir {
    Input,
    Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SHIRPortKind {
    Clock,
    Data { ty: CHIRType },
}

// ── Body ──────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SHIRBody {
    Combinational(SHIRCombBody),
    Sequential(SHIRSeqBody),
}

// ── Combinational body ────────────────────────────────────────────────────────

/// Combinational body as a flat list of statements.
/// Statements are either `Wire` (intermediate value) or `PortDrive` (output),
/// preserving their source order and conditional structure.
#[derive(Debug)]
pub struct SHIRCombBody {
    pub submodules: Vec<SHIRSubmoduleInst>,
    pub stmts: Vec<SHIRStmt>,
}

// ── Sequential body ───────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct SHIRSeqBody {
    pub clock: String,
    pub registers: Vec<SHIRReg>,
    pub submodules: Vec<SHIRSubmoduleInst>,
    /// One entry per phase (tick boundary).
    /// Single-tick modules have exactly one phase (phase_idx = 0).
    pub phases: Vec<SHIRPhase>,
    // Output port drives are embedded in each phase's pre_edge as
    // SHIRStmt::PortDrive, possibly inside If/Match for conditional drives.
}

#[derive(Debug)]
pub struct SHIRReg {
    pub name: String,
    pub ty: CHIRType,
    pub init: Option<SHIRLit>,
}

/// A `#[hardware]` submodule instance. The output_wire is a combinational
/// wire available in all timing regions.
#[derive(Debug)]
pub struct SHIRSubmoduleInst {
    pub inst_name: String,
    pub module_name: String,
    pub inputs: Vec<(String, SHIRExpr)>,
    pub output_wire: String,
    pub output_ty: CHIRType,
}

// ── Phase ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct SHIRPhase {
    pub phase_idx: usize,
    /// Combinational statements before the clock edge.
    /// Includes Wire (intermediate values) and PortDrive (output port drives).
    pub pre_edge: Vec<SHIRStmt>,
    /// Register next-value updates — take effect at the clock edge.
    pub post_edge: Vec<SHIRRegUpdate>,
}

#[derive(Debug)]
pub struct SHIRRegUpdate {
    pub target: String,
    pub next_value: SHIRExpr,
}

// ── Statements ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SHIRStmt {
    /// Combinational wire declaration (intermediate value).
    Wire {
        name: String,
        ty: CHIRType,
        value: SHIRExpr,
    },
    /// Drive an output port. May appear flat or inside If/Match for
    /// conditional drives (e.g. Moore FSM output from match on state register).
    PortDrive {
        port_name: String,
        value: SHIRExpr,
    },
    If {
        condition: SHIRExpr,
        then_stmts: Vec<SHIRStmt>,
        else_stmts: Option<Vec<SHIRStmt>>,
    },
    Match {
        scrutinee: SHIRExpr,
        arms: Vec<SHIRMatchArm>,
    },
    /// `for <var> in <start>..<end>` (exclusive), emitted as an SV `for`.
    ForLoop {
        var: String,
        start: SHIRExpr,
        end: SHIRExpr,
        body: Vec<SHIRStmt>,
    },
    /// `base[index] = value;` — single-bit assignment.
    IndexAssign {
        base: String,
        index: SHIRExpr,
        value: SHIRExpr,
    },
}

#[derive(Debug)]
pub struct SHIRMatchArm {
    pub patterns: Vec<SHIRPattern>,
    pub guard: Option<SHIRExpr>,
    pub stmts: Vec<SHIRStmt>,
}

#[derive(Debug, Clone)]
pub enum SHIRPattern {
    Lit(SHIRLit),
    Wildcard,
    Tuple(Vec<SHIRPattern>),
    EnumVariant { name: String, inner: Option<Box<SHIRPattern>> },
}

// ── Expression model ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum SHIRExpr {
    Var(String),
    Lit(SHIRLit),
    BinOp {
        left: Box<SHIRExpr>,
        op: CHIRBinOp,
        right: Box<SHIRExpr>,
    },
    UnOp {
        op: CHIRUnOp,
        expr: Box<SHIRExpr>,
    },
    Mux {
        cond: Box<SHIRExpr>,
        then_val: Box<SHIRExpr>,
        else_val: Box<SHIRExpr>,
    },
    Case {
        scrutinee: Box<SHIRExpr>,
        arms: Vec<SHIRCaseArm>,
        default: Box<SHIRExpr>,
    },
    Concat(Vec<SHIRExpr>),
    Slice {
        expr: Box<SHIRExpr>,
        high: usize,
        low: usize,
    },
    /// Single-bit select at a dynamic index — `base[index]`.
    DynBit {
        base: Box<SHIRExpr>,
        index: Box<SHIRExpr>,
    },
    /// Width-cast — `width'(expr)`.
    Resize {
        expr: Box<SHIRExpr>,
        width: Width,
    },
    /// Compares phase_r == idx. Used in multi-phase post_edge conditions.
    PhaseEq(usize),
}

#[derive(Debug, Clone)]
pub struct SHIRCaseArm {
    pub pattern: SHIRPattern,
    pub guard: Option<SHIRExpr>,
    pub value: SHIRExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SHIRLit {
    pub ty: CHIRType,
    pub value: u128,
}

// ── Error model ───────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SHIRLowerError {
    /// `AwaitTick` appeared inside an `If` or `Match` arm.
    TickInsideBranch { span: SourceSpan },
    /// Sequential module has no `AwaitTick`.
    NoTick { span: SourceSpan },
    /// `AwaitTick` references a different clock than declared.
    CrossClockTick { expected: String, found: String, span: SourceSpan },
    /// A construct not supported at Phase C level.
    UnsupportedConstruct { description: String, span: SourceSpan },
}

impl SHIRLowerError {
    pub fn span(&self) -> &SourceSpan {
        match self {
            SHIRLowerError::TickInsideBranch { span } => span,
            SHIRLowerError::NoTick { span } => span,
            SHIRLowerError::CrossClockTick { span, .. } => span,
            SHIRLowerError::UnsupportedConstruct { span, .. } => span,
        }
    }
}

impl std::fmt::Display for SHIRLowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let span = self.span();
        write!(f, "{}:{}: ", span.start_line, span.start_col)?;
        match self {
            SHIRLowerError::TickInsideBranch { .. } =>
                write!(f, "clk.tick().await inside a conditional branch is not supported"),
            SHIRLowerError::NoTick { .. } =>
                write!(f, "sequential module has no clk.tick().await"),
            SHIRLowerError::CrossClockTick { expected, found, .. } =>
                write!(f, "tick on clock '{}' but module uses clock '{}'", found, expected),
            SHIRLowerError::UnsupportedConstruct { description, .. } =>
                write!(f, "unsupported construct: {}", description),
        }
    }
}
