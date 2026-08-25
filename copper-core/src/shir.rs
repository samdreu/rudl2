use crate::chir::{CHIRBinOp, CHIRType, CHIRUnOp, ModuleLocalParam, ModuleParam, Width};
use crate::frontend_ir::SourceSpan;

// ── Module ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct SHIRModule {
    pub name: String,
    /// Module-level parameters (const generics), carried through from CHIR to
    /// VLIR emission (M2).
    pub params: Vec<ModuleParam>,
    /// Module-level constants (`localparam`s) carried through from CHIR.
    pub localparams: Vec<ModuleLocalParam>,
    pub ports: Vec<SHIRPort>,
    pub body: SHIRBody,
    pub span: SourceSpan,
}

#[derive(Debug)]
pub struct SHIRPort {
    pub name: String,
    pub direction: SHIRPortDir,
    pub kind: SHIRPortKind,
    /// Registered output port (`RegOut<T,D>`) — driven from `always_ff`. See
    /// `CHIRPort::registered`.
    pub registered: bool,
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
    Structural(SHIRStructuralBody),
}

/// Pure-hierarchy parent body — internal nets + clocked submodule instances.
/// 1:1 with `CHIRStructuralBody` (no timing regions to lower).
#[derive(Debug)]
pub struct SHIRStructuralBody {
    pub nets: Vec<(String, CHIRType)>,
    pub submodules: Vec<SHIRSubmoduleInst>,
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
    /// `Memory<..>` instances — 1:1 with `CHIRSeqBody::memories`.
    pub memories: Vec<SHIRMemory>,
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

/// A memory array instance. Mirrors `CHIRMemoryDecl` minus the span.
#[derive(Debug)]
pub struct SHIRMemory {
    pub name: String,
    pub elem_ty: CHIRType,
    pub depth: usize,
    pub read_ports: usize,
    pub write_ports: usize,
    /// Port latencies — see `CHIRMemoryDecl`.
    pub read_lat: usize,
    pub write_lat: usize,
    /// Preloaded contents — see `CHIRMemInit`.
    pub init: Option<SHIRMemInit>,
    /// Read-during-write ordering — see `CHIRMemoryDecl::write_mode`.
    pub write_mode: crate::memory::WriteMode,
}

/// 1:1 with `CHIRMemInit`, over `SHIRExpr`.
#[derive(Debug)]
pub enum SHIRMemInit {
    Fill { var: String, value: SHIRExpr },
    Words(Vec<SHIRExpr>),
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
    /// `(child_clock_port, parent_clock_signal)` — see `CHIRSubmoduleInst::clocks`.
    pub clocks: Vec<(String, String)>,
    /// `(child_port, net_name)` — see `CHIRSubmoduleInst::port_nets`.
    pub port_nets: Vec<(String, String)>,
    /// Child output port name for the expression form — see `CHIRSubmoduleInst::output_port`.
    pub output_port: Option<String>,
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
    ///
    /// Carries the drive in BOTH forms, because which one is correct depends on
    /// where the drive is finally emitted — a decision `vlir_lower` makes, later
    /// than this IR is built:
    ///
    /// * `value` — read as a continuous `assign`, i.e. AFTER the clock edge, when
    ///   the registers this segment assigns already hold their new values;
    /// * `edge_value` — the same drive written in PRE-edge terms (registers this
    ///   segment assigns are substituted for the values it assigns them, and its
    ///   `let` wires are inlined where that matters), for a drive that becomes a
    ///   non-blocking assignment inside `always_ff` and is therefore sampled BEFORE
    ///   the edge.
    ///
    /// The two are equal unless the segment assigns a register the drive reads.
    /// Carrying both is what stops this stage from having to PREDICT the
    /// registration decision — `vlir_lower` chooses at `split_output_reg`, the one
    /// point where a drive actually becomes edge-sampled. Predicting it here was
    /// tried and is not sound: `hoist_moore_output_defaults` un-registers some
    /// outputs after the fact, so the answer is not a function of this IR alone.
    /// See `TODO` causes L, L-1 and L-2.
    PortDrive {
        port_name: String,
        value: SHIRExpr,
        edge_value: SHIRExpr,
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
    /// Stage a read address on a memory read port — see `CHIRStmt::MemRead`.
    MemRead {
        mem: String,
        port: usize,
        addr: SHIRExpr,
    },
    /// Stage a write on a memory write port — see `CHIRStmt::MemWrite`.
    MemWrite {
        mem: String,
        port: usize,
        addr: SHIRExpr,
        value: SHIRExpr,
    },
}

#[derive(Debug)]
pub struct SHIRMatchArm {
    pub patterns: Vec<SHIRPattern>,
    pub guard: Option<SHIRExpr>,
    pub stmts: Vec<SHIRStmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SHIRPattern {
    Lit(SHIRLit),
    Wildcard,
    Tuple(Vec<SHIRPattern>),
    EnumVariant { name: String, inner: Option<Box<SHIRPattern>> },
}

// ── Expression model ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
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
    /// `mem.read_port::<port>().data()` — the read port's output value.
    MemData { mem: String, port: usize },
    /// `mem.read_port::<port>().is_ready()` — the read port's output-valid flag.
    MemValid { mem: String, port: usize },
}

#[derive(Debug, Clone, PartialEq)]
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
