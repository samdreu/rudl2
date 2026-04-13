use crate::frontend_ir::SourceSpan;

// ── Top-level module ─────────────────────────────────────────────────────────

pub struct CHIRModule {
    pub name: String,
    pub ports: Vec<CHIRPort>,
    pub body: CHIRBody,
    pub span: SourceSpan,
}

// ── Ports ─────────────────────────────────────────────────────────────────────

pub struct CHIRPort {
    pub name: String,
    pub direction: CHIRPortDir,
    pub kind: CHIRPortKind,
    pub span: SourceSpan,
}

pub enum CHIRPortDir {
    Input,
    Output,
}

pub enum CHIRPortKind {
    /// Clock — not a data signal; carries domain name for multi-clock future use.
    Clock { domain: String },
    /// Data — a hardware-typed signal of known width.
    Data { ty: CHIRType },
}

// ── Type system ───────────────────────────────────────────────────────────────

/// Hardware-native types only. All Rust-runtime types (Arc, Mutex, etc.) are
/// stripped in Phase B before this representation is constructed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CHIRType {
    UInt { width: usize },
    SInt { width: usize },
    Bool,
}

// ── Module body ───────────────────────────────────────────────────────────────

pub enum CHIRBody {
    Combinational(CHIRCombBody),
    Sequential(CHIRSeqBody),
}

/// Combinational module body: submodule instances, named wire values, output.
pub struct CHIRCombBody {
    /// #[hardware] submodule instantiations, in source order.
    pub submodules: Vec<CHIRSubmoduleInst>,
    /// Named intermediate wire values, in computation order.
    pub wires: Vec<CHIRWireDecl>,
    /// Expression driving the output port.
    pub output: CHIRExpr,
}

pub struct CHIRWireDecl {
    pub name: String,
    pub ty: CHIRType,
    pub value: CHIRExpr,
    pub span: SourceSpan,
}

/// Sequential module body: registers, submodule instances, and loop body.
pub struct CHIRSeqBody {
    /// Name of the clock parameter this module is clocked on.
    pub clock: String,
    /// State registers — variables that live across a tick boundary.
    pub registers: Vec<CHIRRegDecl>,
    /// #[hardware] submodule instantiations used in this module.
    /// These are wired combinationally — their outputs are wires, not registers.
    pub submodules: Vec<CHIRSubmoduleInst>,
    /// The infinite loop body containing one or more AwaitTick boundaries.
    /// Phase C splits this at each AwaitTick to produce SHIR phases.
    pub loop_body: Vec<CHIRStmt>,
}

pub struct CHIRRegDecl {
    pub name: String,
    pub ty: CHIRType,
    /// Initial value for simulation. Not emitted in synthesis output.
    pub init: Option<CHIRLit>,
    pub span: SourceSpan,
}

// ── Submodule instantiation ───────────────────────────────────────────────────

/// A #[hardware] function call site lowered to a module instantiation.
///
/// The call `let result = full_adder(a, b)` becomes:
///   - A CHIRSubmoduleInst with a generated inst_name and output_wire name
///   - All references to `result` in subsequent expressions replaced with
///     Var(output_wire)
pub struct CHIRSubmoduleInst {
    /// Unique instance name within this module, e.g. "full_adder_0".
    pub inst_name: String,
    /// The #[hardware] module being instantiated, e.g. "full_adder".
    pub module_name: String,
    /// Input port connections: (port_name, driving_expr).
    pub inputs: Vec<(String, CHIRExpr)>,
    /// Name of the wire carrying this instance's output in the parent module.
    pub output_wire: String,
    /// Type of the output.
    pub output_ty: CHIRType,
    pub span: SourceSpan,
}

// ── Statement model ───────────────────────────────────────────────────────────

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

    /// Drive the output port (from emit!()).
    /// Semantics: the last Emit before an AwaitTick wins in that phase.
    Emit {
        value: CHIRExpr,
        span: SourceSpan,
    },

    /// Clock-edge boundary — from clk.tick().await.
    /// Carries the clock name for future multi-clock support.
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

pub struct CHIRMatchArm {
    pub patterns: Vec<CHIRPattern>,
    pub guard: Option<CHIRExpr>,
    pub body: Vec<CHIRStmt>,
    pub span: SourceSpan,
}

/// Patterns preserved from source. Tuple patterns are kept intact (not
/// desugared to if/else chains) so Phase D can emit a Verilog `case` on a
/// concatenated selector.
#[derive(Debug, Clone)]
pub enum CHIRPattern {
    Lit(CHIRLit),
    Wildcard,
    /// Tuple pattern, e.g. (j, k) in a JK flip-flop match.
    Tuple(Vec<CHIRPattern>),
    EnumVariant { name: String, inner: Option<Box<CHIRPattern>> },
}

// ── Expression model ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum CHIRExpr {
    /// Variable reference — wire, register, or submodule output wire name.
    Var(String),

    /// Literal constant.
    Lit(CHIRLit),

    /// Binary operation.
    BinOp {
        left: Box<CHIRExpr>,
        op: CHIRBinOp,
        right: Box<CHIRExpr>,
    },

    /// Unary operation.
    UnOp {
        op: CHIRUnOp,
        expr: Box<CHIRExpr>,
    },

    /// Multiplexer (ternary) — lowered from if-as-expression.
    Mux {
        cond: Box<CHIRExpr>,
        then_val: Box<CHIRExpr>,
        else_val: Box<CHIRExpr>,
    },

    /// Match-as-value (let x = match { ... }).
    /// Distinct from CHIRStmt::Match which is used for side-effect statements.
    Case {
        scrutinee: Box<CHIRExpr>,
        arms: Vec<CHIRCaseArm>,
        /// Default arm value (from `_ => expr`). Required if patterns are
        /// non-exhaustive.
        default: Option<Box<CHIRExpr>>,
    },

    /// Bit concatenation {a, b, c}.
    Concat(Vec<CHIRExpr>),

    /// Bit slice [high:low] — both bounds are compile-time constants at CHIR level.
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
    // Arithmetic — wrapping flag is informational; hardware always wraps at width
    Add { wrapping: bool },
    Sub { wrapping: bool },
    Mul { wrapping: bool },

    // Bitwise
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,

    // Comparison — always produce Bool
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,

    // Logical
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
    /// A Rust construct that cannot be mapped to hardware.
    UnsupportedConstruct {
        description: String,
        span: SourceSpan,
        suggested_rewrite: Option<String>,
    },

    /// A type that cannot be resolved to a CHIRType.
    UnresolvableType {
        ty_text: String,
        span: SourceSpan,
    },

    /// A variable is used as both a register and a wire in the same context.
    RegisterWireConflict {
        name: String,
        span: SourceSpan,
    },

    /// AwaitTick found inside a conditional branch — not supported in Milestone 1.
    TickInsideBranch {
        span: SourceSpan,
    },

    /// emit!() used in a module with no output port.
    EmitWithoutOutput {
        span: SourceSpan,
    },

    /// Width cannot be inferred; explicit annotation required.
    AmbiguousWidth {
        span: SourceSpan,
    },
}

impl std::fmt::Display for CHIRLowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CHIRLowerError::UnsupportedConstruct { description, .. } =>
                write!(f, "unsupported construct: {}", description),
            CHIRLowerError::UnresolvableType { ty_text, .. } =>
                write!(f, "cannot resolve type '{}' to a hardware type", ty_text),
            CHIRLowerError::RegisterWireConflict { name, .. } =>
                write!(f, "variable '{}' used as both register and wire", name),
            CHIRLowerError::TickInsideBranch { .. } =>
                write!(f, "clk.tick().await inside a conditional branch is not supported"),
            CHIRLowerError::EmitWithoutOutput { .. } =>
                write!(f, "emit!() used in a module with no output port"),
            CHIRLowerError::AmbiguousWidth { .. } =>
                write!(f, "cannot infer bit width; add an explicit type annotation"),
        }
    }
}
