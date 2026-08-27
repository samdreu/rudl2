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
    /// Module-level **constants** (SystemVerilog `localparam`s), lowered from the
    /// file-scope `const` items the module actually references. Unlike `params`
    /// these are not overridable at instantiation — a Rust `const` is a fixed
    /// value, not a knob — which is exactly the `localparam` contract.
    ///
    /// They are declared in the ANSI parameter port list rather than the module
    /// body because a const may appear in a **port width** (`In<Bits<WIDTH>, D>`
    /// emits `[WIDTH-1:0]`), and a body declaration is not in scope there.
    /// SystemVerilog permits `local_parameter_declaration` in a
    /// `parameter_port_list`; verified against Verilator 5.044.
    pub localparams: Vec<ModuleLocalParam>,
    pub ports: Vec<CHIRPort>,
    pub body: CHIRBody,
    pub span: SourceSpan,
}

/// A module-level constant, e.g. `localparam int WIDTH = 8`. `value_expr` is the
/// initializer as SystemVerilog text — the source expression, not an evaluated
/// number, so `const MOD: usize = 1 << PTR_W` stays legible as
/// `localparam int MOD = 1 << PTR_W`. Ordering within the list is dependency
/// order: a constant never precedes one it references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleLocalParam {
    pub name: String,
    pub value_expr: String,
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
    /// A registered output port (`RegOut<T,D>`): its value commits at the clock
    /// edge, so the transpiler drives it from `always_ff` (a real flip-flop)
    /// rather than combinationally, regardless of whether it is written on all
    /// paths. Always `false` for inputs, clocks, and plain `Out`.
    pub registered: bool,
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
    /// `[Bits<W>; ELS]` — a fixed-length array of a hardware type, emitted as a
    /// **packed 2-D** SystemVerilog vector: `[ELS-1:0][W-1:0]`.
    ///
    /// Packed rather than unpacked, and 2-D rather than a flat `[ELS*W-1:0]`:
    /// see `design_docs/ARRAY_PORT_ABI.md`. In short — both independent BaseJump
    /// references declare it this way, Verilator gives packed 2-D and flat 1-D
    /// the identical C++ interface (so the testbench harness is unaffected), and
    /// keeping the dimensions separate means neither needs width *arithmetic*:
    /// `len` and the element width are each independently `Concrete` or `Param`.
    Array { elem: Box<CHIRType>, len: Width },
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
    /// A symbolic width bound to a module parameter — `Bits<N>` → `Param("N")`,
    /// emitted as a SystemVerilog `parameter` and a `[N-1:0]` range (M2).
    Param(String),
    // M2 (later): Sub(Box<Width>, usize) — e.g. `N - 1`
}

impl Width {
    /// The resolved concrete width, if known. `None` for a symbolic (`Param`)
    /// width — callers that legitimately handle symbolic widths use this instead
    /// of `concrete()`.
    pub fn as_concrete(&self) -> Option<usize> {
        match self {
            Width::Concrete(n) => Some(*n),
            Width::Param(_) => None,
        }
    }

    /// The resolved width. Panics on a symbolic width — callers that can
    /// legitimately encounter one (M2+) must use `as_concrete`/match explicitly.
    pub fn concrete(&self) -> usize {
        match self {
            Width::Concrete(n) => *n,
            Width::Param(name) => {
                panic!("width `{name}` is symbolic (a module parameter); this path needs a concrete width")
            }
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
    Structural(CHIRStructuralBody),
}

/// A pure-hierarchy parent body (`#[hardware(structural)]`): internal nets that
/// wire children together, and the clocked submodule instances themselves. No
/// registers, no loop, no `always_ff` — the parent has no native clock domain.
#[derive(Debug, Clone)]
pub struct CHIRStructuralBody {
    /// Internal signals wiring children together, declared by `wire::<T,D>(init)`
    /// in the source. `(net_name, type)`.
    pub nets: Vec<(String, CHIRType)>,
    /// The clocked submodule instantiations, in source order.
    pub submodules: Vec<CHIRSubmoduleInst>,
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
    /// `Memory<..>` instances declared before the loop.
    pub memories: Vec<CHIRMemoryDecl>,
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

/// A `Memory<T, R, W, D, READ_LAT, WRITE_LAT>` declared in a sequential body.
///
/// A memory is a hardware *submodule*, not a local variable: an array of `depth`
/// elements of `elem_ty` wired to the parent through `read_ports` read buses and
/// `write_ports` write buses, each with its own latency pipeline.
#[derive(Debug, Clone)]
pub struct CHIRMemoryDecl {
    pub name: String,
    pub elem_ty: CHIRType,
    pub depth: usize,
    pub read_ports: usize,
    pub write_ports: usize,
    /// Cycles from presenting an address to the result reaching the port output
    /// (`READ_LAT`), and from staging a write to it committing (`WRITE_LAT`).
    /// Both are at least 1.
    pub read_lat: usize,
    pub write_lat: usize,
    /// Preloaded contents from `from_fn` / `from_contents`; `None` for `new`
    /// (which zero-fills, matching an unwritten array).
    pub init: Option<CHIRMemInit>,
    /// Read-during-write ordering, from the `.read_first()` / `.write_first()`
    /// builder. `ReadFirst` (the default) means a read sees the contents before
    /// this cycle's write commits; `WriteFirst` means it sees the new value.
    pub write_mode: crate::memory::WriteMode,
    pub span: SourceSpan,
}

/// How a memory's initial contents are described.
///
/// Both forms stay *expressions*, never evaluated constants: the transpiler does
/// not run Rust, so a preload is emitted as the fill it describes rather than as
/// the values it would produce. That is what makes `from_fn(clk, N, |i| f(i))`
/// representable at all — the body lowers with the closure parameter in scope as
/// the fill loop's index.
#[derive(Debug, Clone)]
pub enum CHIRMemInit {
    /// `from_fn(clk, N, |var| value)` — every word from one expression in `var`.
    Fill { var: String, value: CHIRExpr },
    /// `from_contents(clk, vec![a, b, c])` — one expression per word, in order.
    Words(Vec<CHIRExpr>),
}

// ── Submodule instantiation ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CHIRSubmoduleInst {
    pub inst_name: String,
    pub module_name: String,
    pub inputs: Vec<(String, CHIRExpr)>,
    pub output_wire: String,
    pub output_ty: CHIRType,
    /// Clock port connections: `(child_clock_port_name, parent_clock_signal)`.
    /// Empty for a submodule used as a combinational expression inside a normal
    /// module (the legacy expression model). A structurally-instantiated clocked
    /// child carries its clock port(s) here so emit can wire `.clk(parent_clk)`.
    pub clocks: Vec<(String, String)>,
    /// Port connections whose value is an internal net / parent port name rather
    /// than a lowered expression — `(child_port_name, net_name)`. Used by the
    /// structural (statement/port) instantiation form for both extra outputs and
    /// net-valued inputs. The legacy expression form leaves this empty and uses
    /// `inputs` + `output_wire`.
    pub port_nets: Vec<(String, String)>,
    /// The child's *output* port name for the single-output expression form
    /// (emit wires `.<output_port>(output_wire)`). `None` when the child's
    /// outputs are all carried in `port_nets` (structural form).
    pub output_port: Option<String>,
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

    /// A `for <var> in <start>..<end>` loop, emitted as a SystemVerilog `for`
    /// (Verilator unrolls it at elaboration, so `end` may be a parameter). The
    /// bound is exclusive (`<`).
    ForLoop {
        var: String,
        start: CHIRExpr,
        end: CHIRExpr,
        body: Vec<CHIRStmt>,
        span: SourceSpan,
    },

    /// A single-bit assignment `base[index] = value;` (LHS bit-assign). `base` is
    /// an already-declared signal; only the selected bit is driven.
    IndexAssign {
        base: String,
        index: CHIRExpr,
        value: CHIRExpr,
        span: SourceSpan,
    },

    /// Stage a read address — `mem.read_port::<port>().read(addr)`. The capture
    /// happens at the clock edge that ends the segment holding this statement.
    MemRead {
        mem: String,
        port: usize,
        addr: CHIRExpr,
        span: SourceSpan,
    },

    /// Stage a write — `mem.write_port::<port>().write(addr, value)`. The commit
    /// happens at the clock edge that ends the segment holding this statement.
    MemWrite {
        mem: String,
        port: usize,
        addr: CHIRExpr,
        value: CHIRExpr,
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
            CHIRStmt::ForLoop { span, .. } => span,
            CHIRStmt::IndexAssign { span, .. } => span,
            CHIRStmt::MemRead { span, .. } => span,
            CHIRStmt::MemWrite { span, .. } => span,
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
    /// Signedness reinterpretation — `$signed(expr)` / `$unsigned(expr)`.
    ///
    /// Produced by `chir_lower` for `as i*` / `as u*` casts, and PROPAGATED
    /// through bit-identical operators so composed signed arithmetic stays
    /// signed where it observably matters: comparisons keep the wrapper on both
    /// operands (SystemVerilog compares signed iff every operand is signed), a
    /// right shift keeps it on the left operand (the emitter renders `>>>` for a
    /// signed left operand — SystemVerilog's `>>` is logical even on signed
    /// values), and everything width- or bit-shaped strips it (two's complement
    /// makes `+ - * & | ^ <<` bit-identical). Before this variant existed the
    /// cast was stripped outright, so `(a as i32) < (b as i32)` compiled to an
    /// UNSIGNED compare and `as i32 >> 20` to a LOGICAL shift — both lint-clean
    /// and wrong (the signedness claim-ledger entries, fixed 2026-08-27).
    SignCast {
        signed: bool,
        expr: Box<CHIRExpr>,
    },
    Slice {
        expr: Box<CHIRExpr>,
        high: usize,
        low: usize,
    },
    /// A single-bit select at a *dynamic* (runtime) index — `base[index]`.
    /// A constant index uses `Slice` instead.
    DynBit {
        base: Box<CHIRExpr>,
        index: Box<CHIRExpr>,
    },
    /// Width-cast to a target width — emitted as the SV `width'(expr)` cast.
    /// Legal with a symbolic (parameter) width, so it cleans up assignments that
    /// mix concrete index quantities with parameter-width signals.
    Resize {
        expr: Box<CHIRExpr>,
        width: Width,
    },
    /// `mem.read_port::<port>().data()` — the read port's output value.
    MemData { mem: String, port: usize },
    /// `mem.read_port::<port>().is_ready()` — the read port's output-valid flag.
    MemValid { mem: String, port: usize },
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CHIRBinOp {
    Add { wrapping: bool },
    Sub { wrapping: bool },
    Mul { wrapping: bool },
    Rem,
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
