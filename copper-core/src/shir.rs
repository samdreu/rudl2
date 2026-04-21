use crate::chir::{CHIRBinOp, CHIRType, CHIRUnOp};
use crate::frontend_ir::SourceSpan;

// ── Module ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct SHIRModule {
    pub name: String,
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
    /// Reuses CHIRType — widths and signedness are already resolved by Phase B.
    Data { ty: CHIRType },
}

// ── Body ──────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SHIRBody {
    Combinational(SHIRCombBody),
    Sequential(SHIRSeqBody),
}

// ── Combinational body ────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct SHIRCombBody {
    /// `#[hardware]` submodule instances carried through from CHIR unchanged.
    pub submodules: Vec<SHIRSubmoduleInst>,
    /// Intermediate combinational values (blocking assign / wire in Verilog).
    pub wires: Vec<SHIRWire>,
    /// Drives the output port continuously.
    pub output_expr: SHIRExpr,
}

#[derive(Debug)]
pub struct SHIRWire {
    pub name: String,
    pub ty: CHIRType,
    pub value: SHIRExpr,
}

// ── Sequential body ───────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct SHIRSeqBody {
    pub clock: String,

    /// State registers — all live across tick boundaries.
    pub registers: Vec<SHIRReg>,

    /// `#[hardware]` submodule instances.
    /// Outputs are combinational wires visible in all phases.
    pub submodules: Vec<SHIRSubmoduleInst>,

    /// One entry per phase.
    /// Single-tick modules have exactly one entry (phase_idx = 0).
    /// Multi-tick modules have one entry per tick.
    pub phases: Vec<SHIRPhase>,

    /// How the output port is driven — see output drive model below.
    /// `None` if the module has no output port.
    pub output_drive: Option<SHIROutputDrive>,
}

#[derive(Debug)]
pub struct SHIRReg {
    pub name: String,
    pub ty: CHIRType,
    pub init: Option<SHIRLit>,
}

/// A `#[hardware]` submodule instance — passed through unchanged from CHIR.
/// The `output_wire` name is a combinational wire available in all timing regions.
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
    /// 0-indexed phase number within the loop.
    pub phase_idx: usize,

    /// Statements that produce combinational values before the clock edge.
    /// These become blocking assigns in `always_comb` or intermediate wires.
    pub pre_edge: Vec<SHIRStmt>,

    /// Register next-value updates that take effect at the clock edge.
    /// These become non-blocking assigns in `always_ff`.
    pub post_edge: Vec<SHIRRegUpdate>,
}

/// A register's next-value assignment — always non-blocking in Verilog.
#[derive(Debug)]
pub struct SHIRRegUpdate {
    /// Must be a name declared in `SHIRSeqBody::registers`.
    pub target: String,
    /// The next value expression. Conditional updates are expressed as `Mux`
    /// or `Case` — NOT as nested `SHIRStmt` trees.
    pub next_value: SHIRExpr,
}

// ── Output drive model ────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SHIROutputDrive {
    /// Single-tick, `emit!()` fires before the tick (Pattern A).
    /// Emits: `assign out = <expr>;`
    PreEdge(SHIRExpr),

    /// Single-tick, `emit!()` fires after the tick (Pattern B).
    /// Emits: `assign out = <expr>;` (usually a register reference).
    PostEdge(SHIRExpr),

    /// Multi-tick: output depends on which phase is active.
    /// Phase C generates: `assign out = (phase_r == K) ? expr_K : ...`
    PhaseConditional {
        arms: Vec<SHIRPhaseOutputArm>,
        /// Hold value when no phase emits (e.g. a register holding last output).
        /// `None` if there is no default (design-time error if reachable).
        default: Option<Box<SHIRExpr>>,
    },

    /// Combinational module — output is always active.
    Continuous(SHIRExpr),
}

#[derive(Debug)]
pub struct SHIRPhaseOutputArm {
    pub phase_idx: usize,
    pub value: SHIRExpr,
}

// ── Statements ────────────────────────────────────────────────────────────────

/// Statements that appear in `SHIRPhase::pre_edge` — always combinational.
#[derive(Debug)]
pub enum SHIRStmt {
    /// A combinational wire declaration (wire/logic in Verilog).
    Wire {
        name: String,
        ty: CHIRType,
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
    /// Case-as-expression. In sequential post_edge contexts (SHIRRegUpdate::next_value),
    /// Phase D lifts this into a VLIRFFStmt::Case block.
    Case {
        scrutinee: Box<SHIRExpr>,
        arms: Vec<SHIRCaseArm>,
        /// Required — every Case in SHIR must have a default (hold or explicit value).
        default: Box<SHIRExpr>,
    },
    Concat(Vec<SHIRExpr>),
    Slice {
        expr: Box<SHIRExpr>,
        high: usize,
        low: usize,
    },
    /// Used in `PhaseConditional` output drives to compare `phase_r`.
    /// Phase D emits this as `phase_r == <idx>`.
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
    /// `AwaitTick` appeared inside an `If` or `Match` arm — only allowed at flat loop top-level.
    TickInsideBranch { span: SourceSpan },

    /// `emit!()` appeared inside an `If` or `Match` arm — only flat top-level emits are
    /// supported in Milestone 1.
    EmitInsideBranch { span: SourceSpan },

    /// Multiple `emit!()` calls in the same timing segment.
    MultipleEmits { span: SourceSpan },

    /// Sequential module has no `AwaitTick` — should have been caught in Phase B.
    NoTick { span: SourceSpan },

    /// `AwaitTick` references a different clock than declared in `CHIRSeqBody::clock`.
    CrossClockTick { expected: String, found: String, span: SourceSpan },

    /// `emit!()` called in a module with no output port.
    EmitWithoutOutput { span: SourceSpan },

    /// Module has an output port but no `emit!()` in any segment.
    OutputWithoutEmit { span: SourceSpan },

    /// A construct is not supported at Phase C level.
    UnsupportedConstruct { description: String, span: SourceSpan },
}

impl std::fmt::Display for SHIRLowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SHIRLowerError::TickInsideBranch { .. } =>
                write!(f, "clk.tick().await inside a conditional branch is not supported"),
            SHIRLowerError::EmitInsideBranch { .. } =>
                write!(f, "emit!() inside a conditional branch is not supported in Milestone 1"),
            SHIRLowerError::MultipleEmits { .. } =>
                write!(f, "multiple emit!() calls in the same timing segment"),
            SHIRLowerError::NoTick { .. } =>
                write!(f, "sequential module has no clk.tick().await"),
            SHIRLowerError::CrossClockTick { expected, found, .. } =>
                write!(f, "tick on clock '{}' but module uses clock '{}'", found, expected),
            SHIRLowerError::EmitWithoutOutput { .. } =>
                write!(f, "emit!() used but module has no output port"),
            SHIRLowerError::OutputWithoutEmit { .. } =>
                write!(f, "module has output port but no emit!() found"),
            SHIRLowerError::UnsupportedConstruct { description, .. } =>
                write!(f, "unsupported construct: {}", description),
        }
    }
}
