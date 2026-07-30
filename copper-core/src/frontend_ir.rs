/// frontend capture output for one module
/// intentionally source-shaped and pre-normalization
#[derive(Debug, Clone)]
pub struct FrontendModuleIR {
    pub module_name: String,
    
    /// raw signature info
    pub signature: FrontendSignature,

    /// high-level execution classification
    pub classification: FrontendClassification,

    /// clock-related metadata from params
    pub clocks: Vec<ClockParamMeta>,

    /// source-ordered top-level body statements
    pub raw_statements: Vec<RawStmt>,

    /// Enum definitions visible to this module. Populated with enums declared
    /// inside the function body by `capture_frontend_ir`; file-scope enums are
    /// injected by the caller (they are not reachable from the `ItemFn` alone).
    pub enums: Vec<ItemEnum>,

    /// The mode declared in the `#[hardware(<mode>)]` attribute, when present.
    /// `None` when the function carries no `#[hardware(...)]` attribute (e.g. a
    /// bare `ItemFn` handed directly to `transpile_item_fn`). This is the
    /// *authoritative* mode as written; `classification` is only inferred from
    /// async-ness and cannot distinguish `synchronizer` (which is also async)
    /// from `sequential`.
    pub declared_mode: Option<HardwareMode>,

    /// File-scope items visible to this module but not reachable from its
    /// `ItemFn`, injected by the caller (see `transpile_source`) exactly like
    /// `enums`. Populated for later inlining/monomorphization; empty when a FIR
    /// is captured from an `ItemFn` alone. Nothing consumes these yet.
    pub file_fns: Vec<FrontendFnIR>,
    pub file_structs: Vec<ItemStruct>,
    pub file_consts: Vec<ItemConst>,
    pub file_impls: Vec<FrontendImplIR>,
    pub file_traits: Vec<FrontendTraitIR>,

    /// span for full module/function declaration
    pub span: SourceSpan,
}

/// A free function or a method body captured from file scope, source-shaped.
/// This is `capture_frontend_ir` minus the hardware-only fields (clocks,
/// classification, declared_mode) — enough to inline the callee later.
#[derive(Debug, Clone)]
pub struct FrontendFnIR {
    pub name: String,

    pub signature: FrontendSignature,

    /// The `self` receiver for a method (`&self`, `&mut self`, `self`); `None`
    /// for free functions and associated functions like `Opcode::from_bits`.
    pub receiver: Option<Receiver>,

    /// Body statements, source-ordered. Empty for a bodyless trait-method
    /// declaration (`fn read_ready(&self) -> bool;`).
    pub raw_statements: Vec<RawStmt>,

    pub span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receiver {
    /// `self`
    Value,
    /// `&self`
    Ref,
    /// `&mut self`
    RefMut,
}

/// An `impl` block captured from file scope.
#[derive(Debug, Clone)]
pub struct FrontendImplIR {
    /// The implementing type as source text: `Opcode`, `MainClk`.
    pub self_ty: String,

    /// The implemented trait, if any: `Some("ClockDomain")` for a trait impl,
    /// `None` for an inherent impl (`impl Opcode { ... }`).
    pub trait_name: Option<String>,

    pub methods: Vec<FrontendFnIR>,

    pub span: SourceSpan,
}

/// A `trait` definition captured from file scope. Methods carry their signature
/// and any default body (empty `raw_statements` for a bare signature decl).
#[derive(Debug, Clone)]
pub struct FrontendTraitIR {
    pub name: String,

    pub methods: Vec<FrontendFnIR>,

    pub span: SourceSpan,
}

/// The argument of the `#[hardware(<mode>)]` attribute. Mirrors the modes the
/// `copper-macros` `#[hardware]` proc-macro accepts, plus `Synchronizer` (the
/// sanctioned CDC crossing point). Kept in the FIR so later phases — CDC 2-FF
/// emission in particular — can act on the mode the author actually declared
/// rather than re-deriving it from the signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareMode {
    Sequential,
    Combinational,
    Synchronizer,
    /// A pure-hierarchy parent: receives one or more `Clock`s and *instantiates*
    /// clocked submodules, threading each child's clock through. It has no
    /// `always_ff` of its own — no top-level loop, no `tick`. This is the
    /// multi-clock enabler (item 4): a parent with no native clock domain that
    /// wires independently-clocked children into one coherent component.
    Structural,
}

#[derive(Debug, Clone)]
pub struct FrontendSignature {
    /// parameter list in declared order
    pub params: Vec<RawParam>,

    /// Generic parameters in declared order: const generics
    /// (`<const N: usize>` — shift_register, mux), generic clock domains
    /// (`<SrcD: ClockDomain>` — sync_2ff), and any type/lifetime params. Empty
    /// for a non-generic module. Needed for monomorphization: a const generic
    /// pairs with a call-site turbofish value, a `ClockDomain`-bounded type
    /// param names a domain to substitute.
    pub generics: Vec<GenericParamMeta>,

    /// A `where` clause as raw source text, if the function declares one.
    /// Preserved verbatim because bounds there can be arbitrarily complex; no
    /// current example uses one, but the FIR must not silently drop it.
    pub where_clause_text: Option<String>,

    /// return type info
    pub return_ty: Option<RawTypeRef>,
}

/// One generic parameter from a module's signature, source-shaped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericParamMeta {
    pub kind: GenericParamKind,

    /// Parameter name without sigil: `N`, `SrcD`, `a` (for `'a`).
    pub name: String,

    /// For a `const` param, the declared type text (`usize`); `None` otherwise.
    pub const_ty: Option<RawTypeRef>,

    /// Trait or lifetime bounds as source text, e.g. `["ClockDomain"]` for
    /// `<SrcD: ClockDomain>`. Empty when the param is unbounded.
    pub bounds: Vec<String>,

    /// Default type/const as source text if declared (`T = u8`,
    /// `const N: usize = 8`); `None` otherwise.
    pub default: Option<String>,

    /// Full raw token text of the parameter, for a lossless round-trip.
    pub raw_text: String,

    pub span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericParamKind {
    Type,
    Const,
    Lifetime,
}

#[derive(Debug, Clone)]
pub struct RawParam {
    pub name: String,

    /// type as preserved source txt
    pub ty: RawTypeRef,

    /// full raw param text
    pub raw_text: String,

    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTypeRef {
    /// best-effort canonical string from parsed type tokens
    pub ty_text: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontendClassification {
    CombinationalFn,
    AsyncSequentialFn,
    /// A pure-hierarchy parent (`#[hardware(structural)]`): submodule
    /// instantiations + internal wiring, no top-level loop/tick. Cannot be
    /// inferred from async-ness alone (a structural parent is also async), so it
    /// is set only from the declared `#[hardware(structural)]` mode.
    StructuralFn,
}

#[derive(Debug, Clone)]
pub struct ClockParamMeta {
    /// index in signature.params
    pub param_idx: usize,

    /// name of parameter
    pub param_name: String,

    /// Raw type text, usually "Clock<Domain>"
    pub clock_ty: String,

    /// domain text if present, e.g. "MainClk"
    // maybe rename this??
    pub domain_hint: Option<String>,

    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawStmtKind {
    Local(LocalStmt), // let statement
    Expr(ExprStmt),
    Item(ItemStmt),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalStmt {
    pub is_mut: bool,
    pub ty: Option<RawTypeRef>,
    pub name: String,
    pub init: Option<ExprType>,
    pub attrs: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemStmt {
    Const(ItemConst),
    Enum(ItemEnum),
    Struct(ItemStruct),
    Type(ItemType),
    Macro(ItemMacro),
    Other(ItemOther),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemConst {
    pub name: String,
    pub ty: RawTypeRef,
    pub value_text: String,
    pub attrs: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemEnum {
    pub name: String,
    pub variants: Vec<EnumVariant>,
    pub attrs: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumVariant {
    pub name: String,
    pub discriminant: Option<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStruct {
    pub name: String,
    pub fields: Vec<StructField>,
    pub attrs: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructField {
    pub name: String,
    pub ty: RawTypeRef,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemType {
    pub name: String,
    pub target_ty: RawTypeRef,
    pub attrs: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemMacro {
    pub name: String,
    pub body_text: String,
    pub attrs: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemOther {
    pub text: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprStmt {
    pub expr: ExprType,
    pub has_semi: bool,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprType {
    Array(ExprArray),
    Assign(ExprAssign),
    Async(ExprAsync),
    Await(ExprAwait),
    Binary(ExprBinary),
    Block(ExprBlock),
    Call(ExprCall),
    Cast(ExprCast),
    Field(ExprField),
    Index(ExprIndex),
    If(ExprIf),
    Let(ExprLet),
    Lit(ExprLit),
    Loop(ExprLoop),
    Match(ExprMatch),
    MethodCall(ExprMethodCall),
    Path(ExprPath),
    Range(ExprRange),
    Reference(ExprReference),
    Repeat(ExprRepeat),
    Return(ExprReturn),
    Struct(ExprStruct),
    Tuple(ExprTuple),
    Unary(ExprUnary),
    Break(ExprBreak),
    Continue(ExprContinue),
    While(ExprWhile),
    Yield(ExprYield),
    Const(ExprConst),
    Try(ExprTry),
    Macro(ExprMacro),
    ForLoop(ExprForLoop),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprBlock {
    pub stmts: Vec<RawStmt>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprArray {
    pub elements: Vec<ExprType>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprAssign {
    pub left: Box<ExprType>,
    pub right: Box<ExprType>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprAsync {
    pub is_move: bool,
    pub block: Vec<RawStmt>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprAwait {
    pub base: Box<ExprType>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprBinary {
    pub left: Box<ExprType>,
    pub op: String,
    pub right: Box<ExprType>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprCall {
    pub func: Box<ExprType>,
    pub args: Vec<ExprType>,
    /// True if the callee was annotated with #[hardware] — this call site
    /// will be lowered to a CHIRSubmoduleInst rather than an inlined call.
    pub is_hardware_module: bool,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprCast {
    pub expr: Box<ExprType>,
    pub target_ty: RawTypeRef,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprField {
    pub base: Box<ExprType>,
    pub member: String,
    pub span: SourceSpan,
}

/// Index expression: `base[index]` (e.g. bit selection `state[0]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprIndex {
    pub base: Box<ExprType>,
    pub index: Box<ExprType>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprIf {
    pub condition: Box<ExprType>,
    pub then_block: Vec<RawStmt>,
    pub else_branch: Option<Box<ExprType>>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprLet {
    pub pattern_text: String,
    pub expr: Box<ExprType>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprLit {
    pub text: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprLoop {
    pub body: Vec<RawStmt>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprMatchArm {
    pub pattern_text: String,
    pub guard: Option<Box<ExprType>>,
    pub body: Box<ExprType>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprMatch {
    pub scrutinee: Box<ExprType>,
    pub arms: Vec<ExprMatchArm>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprMethodCall {
    pub receiver: Box<ExprType>,
    pub method: String,
    pub args: Vec<ExprType>,
    /// Turbofish generic arguments on the method itself, one canonical token
    /// string per argument: `port.read_port::<0>()` → `["0"]`,
    /// `instr.part_select::<3>(12)` → `["3"]` (the `12` is an `arg`, not here).
    /// Empty when the call has no `::<...>`. Path-form turbofish
    /// (`Bits::<32>::from_lit::<4>()`) is preserved separately inside
    /// `ExprPath::path_text`; this field is only for the *method* form, which
    /// syn exposes as `ExprMethodCall::turbofish`.
    pub turbofish: Vec<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprPath {
    pub path_text: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprRange {
    pub start: Option<Box<ExprType>>,
    pub end: Option<Box<ExprType>>,
    pub inclusive: bool,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprReference {
    pub is_mut: bool,
    pub expr: Box<ExprType>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprRepeat {
    pub expr: Box<ExprType>,
    pub len: Box<ExprType>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprStructField {
    pub member: String,
    pub expr: Box<ExprType>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprStruct {
    pub path_text: String,
    pub fields: Vec<ExprStructField>,
    pub rest: Option<Box<ExprType>>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprTuple {
    pub elements: Vec<ExprType>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprReturn {
    pub value: Option<Box<ExprType>>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprBreak {
    pub label: Option<String>,
    pub expr: Option<Box<ExprType>>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprContinue {
    pub label: Option<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprUnary {
    pub op: String,
    pub expr: Box<ExprType>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprWhile {
    pub condition: Box<ExprType>,
    pub body: Vec<RawStmt>,
    pub span: SourceSpan,
}

/// A `for <pat> in <iter> { <body> }` loop. `pat_text` is the loop-variable
/// pattern as source text (`i`, `_`); `iter` is the iterator expression, usually
/// a range (`0..N`). Lowering unrolls or emits an SV loop (a later increment);
/// this captures it structurally instead of dropping it to opaque text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprForLoop {
    pub pat_text: String,
    pub iter: Box<ExprType>,
    pub body: Vec<RawStmt>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprYield {
    pub value: Option<Box<ExprType>>,
    pub span: SourceSpan,
}

/// A `const { ... }` block. In hardware bodies these are compile-time checks
/// (`const { assert!(N_LOG == safe_clog2(N)) }` — shift_register, mux,
/// rotate_right, priority_encode); a lowering pass elides them, but the body is
/// captured here rather than flattened to opaque text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprConst {
    pub stmts: Vec<RawStmt>,
    pub span: SourceSpan,
}

/// The `?` try operator: `expr?`. Appears in `decode()`'s
/// `Opcode::from_bits(...)?` (rv32i_cpu) once file-scope helpers are inlined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprTry {
    pub expr: Box<ExprType>,
    pub span: SourceSpan,
}

/// A macro invocation in expression position: `panic!(...)` (rv32i_cpu decode
/// arm), `println!(...)`. The delimited tokens are kept as raw text — a macro
/// body is not Rust-expression-shaped, so there is nothing structured to
/// descend into. `name` is the macro path (`panic`, `assert`, `println`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprMacro {
    pub name: String,
    pub tokens_text: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawStmt {
    pub order: usize,
    pub kind: RawStmtKind,
    pub text: String,
    pub span: SourceSpan,
}

// keep independent of syn/proc-macro span types
// populate from parser-side span conversion
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceSpan {
    pub start_line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
}
