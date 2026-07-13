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

    /// span for full module/function declaration
    pub span: SourceSpan,
}

#[derive(Debug, Clone)]
pub struct FrontendSignature {
    /// parameter list in declared order
    pub params: Vec<RawParam>,

    /// return type info
    pub return_ty: Option<RawTypeRef>,
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
    If(ExprIf),
    Let(ExprLet),
    Lit(ExprLit),
    Loop(ExprLoop),
    Match(ExprMatch),
    MethodCall(ExprMethodCall),
    Range(ExprRange),
    Reference(ExprReference),
    Repeat(ExprRepeat),
    Return(ExprReturn),
    Unary(ExprUnary),
    While(ExprWhile),
    Yield(ExprYield),
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
pub struct ExprReturn {
    pub value: Option<Box<ExprType>>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprYield {
    pub value: Option<Box<ExprType>>,
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
