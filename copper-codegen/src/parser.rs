use copper_core::frontend_ir::{
    ClockParamMeta, EnumVariant, ExprArray, ExprAssign, ExprAsync, ExprAwait, ExprBinary, ExprCall, ExprCast,
    ExprBlock, ExprBreak, ExprConst, ExprContinue, ExprField, ExprIf, ExprIndex, ExprLet, ExprLit, ExprLoop,
    ExprMacro, ExprMatch, ExprTry,
    ExprMatchArm, ExprMethodCall, ExprPath, ExprRange, ExprReference, ExprRepeat, ExprReturn, ExprStmt,
    ExprStruct, ExprStructField, ExprTuple, ExprType, ExprUnary, ExprWhile, ExprYield, FrontendClassification,
    FrontendFnIR, FrontendImplIR, FrontendModuleIR, FrontendSignature, FrontendTraitIR, GenericParamKind,
    GenericParamMeta, HardwareMode, ItemConst, ItemEnum,
    ItemMacro, ItemOther, ItemStmt, ItemStruct, ItemType, LocalStmt, RawParam, RawStmt, RawStmtKind,
    RawTypeRef, Receiver, SourceSpan, StructField,
};
use copper_core::{ModuleIR, Port};
use copper_core::ir::Statement;
use quote::{ToTokens, quote};
use syn::spanned::Spanned;
use syn::{BinOp, Expr, ItemFn, Stmt, UnOp};

// Span-carrying, matching the CHIRLowerError/SHIRLowerError convention downstream
// (copper-core/src/chir.rs, copper-core/src/shir.rs) so a location survives the
// whole FIR -> CHIR -> SHIR -> VLIR pipeline instead of being dropped at Phase A.
#[derive(Debug, Clone)]
pub enum LowerError {
    UnsupportedExpr { description: String, span: SourceSpan },
    UnsupportedStmt { description: String, span: SourceSpan },
    SignalNotFound { name: String, span: SourceSpan },
    MissingArgument { name: String, span: SourceSpan },
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            LowerError::UnsupportedExpr { description, span } =>
                write!(f, "{}:{}: unsupported expression: {}", span.start_line, span.start_col, description),
            LowerError::UnsupportedStmt { description, span } =>
                write!(f, "{}:{}: unsupported statement: {}", span.start_line, span.start_col, description),
            LowerError::SignalNotFound { name, span } =>
                write!(f, "{}:{}: signal not found: {}", span.start_line, span.start_col, name),
            LowerError::MissingArgument { name, span } =>
                write!(f, "{}:{}: missing argument: {}", span.start_line, span.start_col, name),
        }
    }
}

pub struct IRBuilder;

// Is this a NO-OP rn?
// TODO: Remove
impl IRBuilder {
    pub fn from_ast(_design_fn: &ItemFn, ports: Vec<Port>) -> Result<ModuleIR, LowerError> {
        Ok(ModuleIR {
            name: String::new(),
            ports,
            statements: Vec::<Statement>::new(),
            submodules: Vec::new(),
        })
    }
}

// grabs the frontend IR from the parsed rust function and returns it
pub fn capture_frontend_ir(design_fn: &ItemFn, hardware_fns: &std::collections::HashSet<String>) -> Result<FrontendModuleIR, LowerError> {
    let signature = capture_signature(&design_fn.sig);
    let clocks = capture_clock_metadata(design_fn);
    let classification = classify_module(design_fn);
    let raw_statements = capture_raw_statements(design_fn, hardware_fns);

    // Enums declared inside the function body. File-scope enums are injected by
    // the caller (see `transpile_source`).
    let enums = raw_statements
        .iter()
        .filter_map(|s| match &s.kind {
            RawStmtKind::Item(ItemStmt::Enum(e)) => Some(e.clone()),
            _ => None,
        })
        .collect();

    Ok(FrontendModuleIR {
        module_name: design_fn.sig.ident.to_string(),
        signature,
        classification,
        clocks,
        raw_statements,
        enums,
        declared_mode: capture_hardware_mode(design_fn),
        // File-scope items are injected by the caller (see `transpile_source`),
        // just like `enums`; a bare `ItemFn` has none.
        file_fns: Vec::new(),
        file_structs: Vec::new(),
        file_consts: Vec::new(),
        file_impls: Vec::new(),
        file_traits: Vec::new(),
        span: capture_source_span(design_fn),
    })
}

/// The mode written in `#[hardware(<mode>)]`, or `None` when the function has no
/// such attribute. Unlike `classify_module` (which only inspects async-ness),
/// this is the author's declared intent — the only way to tell `synchronizer`
/// (async, but a CDC crossing point) apart from `sequential`. Unknown arguments
/// are treated as absent rather than guessed; the `#[hardware]` proc-macro is
/// the layer that rejects a bad mode, so the transpiler need not re-diagnose it.
fn capture_hardware_mode(design_fn: &ItemFn) -> Option<HardwareMode> {
    let attr = design_fn
        .attrs
        .iter()
        .find(|a| a.path().segments.last().map(|s| s.ident == "hardware").unwrap_or(false))?;

    // `#[hardware(sequential)]` parses its argument as a single path meta.
    let mode = attr.parse_args::<syn::Path>().ok()?;
    match mode.get_ident()?.to_string().as_str() {
        "sequential" => Some(HardwareMode::Sequential),
        "combinational" => Some(HardwareMode::Combinational),
        "synchronizer" => Some(HardwareMode::Synchronizer),
        _ => None,
    }
}

// Captures the signature information from the design function
// This includes the parameter names, types, and return type (if any).
// (There should be no return types)
/// Capture a signature from any `syn::Signature` — a `#[hardware]` module's, a
/// free function's, or an impl/trait method's. A `self` receiver (present on
/// methods) is a `FnArg::Receiver`, skipped here and captured separately by
/// `extract_receiver`.
fn capture_signature(sig: &syn::Signature) -> FrontendSignature {
    let mut params = Vec::new();

    for input in &sig.inputs {
        if let syn::FnArg::Typed(pat_type) = input {
            if let syn::Pat::Ident(pat_ident) = &*pat_type.pat {
                let name = pat_ident.ident.to_string();
                let ty = RawTypeRef {
                    ty_text: pat_type.ty.to_token_stream().to_string(),
                    span: capture_source_span(&*pat_type.ty),
                };

                params.push(RawParam {
                    name,
                    ty,
                    raw_text: quote!(#pat_type).to_string(),
                    span: capture_source_span(pat_type),
                });
            }
        }
    }

    let return_ty = match &sig.output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(_, ty) => Some(RawTypeRef {
            ty_text: ty.to_token_stream().to_string(),
            span: capture_source_span(&**ty),
        }),
    };

    let generics = sig
        .generics
        .params
        .iter()
        .map(capture_generic_param)
        .collect();

    let where_clause_text = sig
        .generics
        .where_clause
        .as_ref()
        .map(|w| w.to_token_stream().to_string());

    FrontendSignature { params, generics, where_clause_text, return_ty }
}

/// Lower one `syn::GenericParam` to source-shaped FIR: `<const N: usize>`,
/// `<SrcD: ClockDomain>`, `<T = u8>`, `<'a: 'b>`. Bounds and defaults are kept
/// as raw token text (their internals are irrelevant to lowering, which only
/// needs the name, the const type, and which bound marks a clock domain).
fn capture_generic_param(param: &syn::GenericParam) -> GenericParamMeta {
    let raw_text = param.to_token_stream().to_string();
    let span = capture_source_span(param);
    match param {
        syn::GenericParam::Const(c) => GenericParamMeta {
            kind: GenericParamKind::Const,
            name: c.ident.to_string(),
            const_ty: Some(RawTypeRef {
                ty_text: c.ty.to_token_stream().to_string(),
                span: capture_source_span(&c.ty),
            }),
            bounds: Vec::new(),
            default: c.default.as_ref().map(|d| d.to_token_stream().to_string()),
            raw_text,
            span,
        },
        syn::GenericParam::Type(t) => GenericParamMeta {
            kind: GenericParamKind::Type,
            name: t.ident.to_string(),
            const_ty: None,
            bounds: t.bounds.iter().map(|b| b.to_token_stream().to_string()).collect(),
            default: t.default.as_ref().map(|d| d.to_token_stream().to_string()),
            raw_text,
            span,
        },
        syn::GenericParam::Lifetime(l) => GenericParamMeta {
            kind: GenericParamKind::Lifetime,
            name: l.lifetime.ident.to_string(),
            const_ty: None,
            bounds: l.bounds.iter().map(|b| b.to_token_stream().to_string()).collect(),
            default: None,
            raw_text,
            span,
        },
    }
}

fn classify_module(design_fn: &ItemFn) -> FrontendClassification {
    if design_fn.sig.asyncness.is_some() {
        FrontendClassification::AsyncSequentialFn
    } else {
        FrontendClassification::CombinationalFn
    }
}

fn capture_clock_metadata(design_fn: &ItemFn) -> Vec<ClockParamMeta> {
    let mut clocks = Vec::new();

    for (param_idx, input) in design_fn.sig.inputs.iter().enumerate() {
        if let syn::FnArg::Typed(pat_type) = input {
            let param_name = match &*pat_type.pat {
                syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                _ => continue,
            };

            let clock_ty = pat_type.ty.to_token_stream().to_string();
            let ty_compact: String = clock_ty.chars().filter(|c| !c.is_whitespace()).collect();
            if !ty_compact.starts_with("Clock<") {
                continue;
            }

            let domain_hint = ty_compact
                .strip_prefix("Clock<")
                .and_then(|s| s.strip_suffix('>'))
                .map(|s| s.to_string());

            clocks.push(ClockParamMeta {
                param_idx,
                param_name,
                clock_ty,
                domain_hint,
                span: capture_source_span(pat_type),
            });
        }
    }

    clocks
}

fn capture_raw_statements(design_fn: &ItemFn, hardware_fns: &std::collections::HashSet<String>) -> Vec<RawStmt> {
    parse_block_stmts(&design_fn.block, hardware_fns)
}

// Lowers every statement in a `syn::Block` to `RawStmt`, preserving source order.
// Shared by every expression form that carries a nested block (async, block, if/else,
// loop, while) so the order/kind/text/span wiring lives in exactly one place.
fn parse_block_stmts(block: &syn::Block, hardware_fns: &std::collections::HashSet<String>) -> Vec<RawStmt> {
    block
        .stmts
        .iter()
        .enumerate()
        .map(|(order, stmt)| RawStmt {
            order,
            kind: classify_raw_stmt_kind(stmt, hardware_fns),
            text: quote!(#stmt).to_string(),
            span: capture_source_span(stmt),
        })
        .collect()
}

fn classify_raw_stmt_kind(stmt: &Stmt, hardware_fns: &std::collections::HashSet<String>) -> RawStmtKind {
    match stmt {
        Stmt::Local(local) => parse_local_stmt(local, hardware_fns),
        Stmt::Item(item) => parse_item_stmt(item),
        Stmt::Expr(expr, semi) => parse_expr_stmt(expr, semi.is_some(), hardware_fns),
        // A macro call in statement position (`println!(...);`, `emit!(v);`).
        // Captured as a structured `ExprMacro`, matching the expression-position
        // `Expr::Macro` arm, rather than an opaque literal.
        Stmt::Macro(stmt_macro) => RawStmtKind::Expr(ExprStmt {
            expr: ExprType::Macro(macro_to_expr(&stmt_macro.mac, capture_source_span(stmt_macro))),
            has_semi: stmt_macro.semi_token.is_some(),
            span: capture_source_span(stmt_macro),
        }),
    }
}

fn parse_local_stmt(local: &syn::Local, hardware_fns: &std::collections::HashSet<String>) -> RawStmtKind {
    let is_mut = match &local.pat {
        syn::Pat::Ident(pat_ident) => pat_ident.mutability.is_some(),
        syn::Pat::Type(pat_ty) => matches!(
            &*pat_ty.pat,
            syn::Pat::Ident(pat_ident) if pat_ident.mutability.is_some()
        ),
        _ => false,
    };

    let ty = extract_explicit_type(local)
        .or_else(|| infer_local_type_hint_from_init(local));

    let name = extract_local_name(local).unwrap_or_else(|| "_".to_string());

    RawStmtKind::Local(LocalStmt {
        is_mut,
        ty,
        name,
        init: local.init.as_ref().map(|init| parse_expr_type(&init.expr, hardware_fns)),
        attrs: local.attrs.iter().map(|a| quote!(#a).to_string()).collect(),
        span: capture_source_span(local),
    })
}

/// Capture enum definitions declared at **file scope**. These are not reachable
/// from an `ItemFn` alone, so callers inject them into each module's
/// `FrontendModuleIR::enums` (see `transpile_source`).
pub fn capture_file_enums(file: &syn::File) -> Vec<ItemEnum> {
    file.items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Enum(e) => Some(ItemEnum {
                name: e.ident.to_string(),
                variants: e
                    .variants
                    .iter()
                    .map(|v| EnumVariant {
                        name: v.ident.to_string(),
                        discriminant: v
                            .discriminant
                            .as_ref()
                            .map(|(_, expr)| expr.to_token_stream().to_string()),
                        span: capture_source_span(v),
                    })
                    .collect(),
                attrs: e.attrs.iter().map(|a| quote!(#a).to_string()).collect(),
                span: capture_source_span(item),
            }),
            _ => None,
        })
        .collect()
}

/// File-scope items visible to every module in a file but not reachable from an
/// `ItemFn` alone. Captured once per file by `capture_file_scope` and injected
/// into each module's FIR by `transpile_source`, mirroring `capture_file_enums`.
/// (Enums are captured separately, by `capture_file_enums`.)
pub struct FileScope {
    pub fns: Vec<FrontendFnIR>,
    pub structs: Vec<ItemStruct>,
    pub consts: Vec<ItemConst>,
    pub impls: Vec<FrontendImplIR>,
    pub traits: Vec<FrontendTraitIR>,
}

/// Capture the file-scope free fns, structs, consts, impls, and traits. Hardware
/// modules (`hardware_fns`) are excluded from `fns` — they are transpiled as
/// modules in their own right, not inlinable helpers. Nothing consumes the
/// result yet; this only closes the capture gap (see `TRANSPILATION_TODO.md` #7).
pub fn capture_file_scope(
    file: &syn::File,
    hardware_fns: &std::collections::HashSet<String>,
) -> FileScope {
    let mut scope = FileScope {
        fns: Vec::new(),
        structs: Vec::new(),
        consts: Vec::new(),
        impls: Vec::new(),
        traits: Vec::new(),
    };

    for item in &file.items {
        match item {
            // Free functions, minus the hardware modules themselves.
            syn::Item::Fn(f) if !hardware_fns.contains(&f.sig.ident.to_string()) => {
                scope.fns.push(capture_fn_from_parts(
                    &f.sig,
                    Some(&f.block),
                    hardware_fns,
                    capture_source_span(f),
                ));
            }
            syn::Item::Struct(s) => scope.structs.push(capture_item_struct(s)),
            syn::Item::Const(c) => scope.consts.push(capture_item_const(c)),
            syn::Item::Impl(imp) => scope.impls.push(capture_impl(imp, hardware_fns)),
            syn::Item::Trait(t) => scope.traits.push(capture_trait(t, hardware_fns)),
            _ => {}
        }
    }

    scope
}

/// Lower a function's parts to `FrontendFnIR`. `block` is `None` for a bodyless
/// trait-method declaration; its `raw_statements` are then empty.
fn capture_fn_from_parts(
    sig: &syn::Signature,
    block: Option<&syn::Block>,
    hardware_fns: &std::collections::HashSet<String>,
    span: SourceSpan,
) -> FrontendFnIR {
    FrontendFnIR {
        name: sig.ident.to_string(),
        signature: capture_signature(sig),
        receiver: extract_receiver(sig),
        raw_statements: block
            .map(|b| parse_block_stmts(b, hardware_fns))
            .unwrap_or_default(),
        span,
    }
}

/// The `self` receiver of a method, if any. `Opcode::from_bits(op: Bits<7>)` and
/// free functions have none.
fn extract_receiver(sig: &syn::Signature) -> Option<Receiver> {
    match sig.inputs.first() {
        Some(syn::FnArg::Receiver(r)) => Some(if r.reference.is_none() {
            Receiver::Value
        } else if r.mutability.is_some() {
            Receiver::RefMut
        } else {
            Receiver::Ref
        }),
        _ => None,
    }
}

fn capture_impl(
    imp: &syn::ItemImpl,
    hardware_fns: &std::collections::HashSet<String>,
) -> FrontendImplIR {
    let methods = imp
        .items
        .iter()
        .filter_map(|it| match it {
            syn::ImplItem::Fn(m) => Some(capture_fn_from_parts(
                &m.sig,
                Some(&m.block),
                hardware_fns,
                capture_source_span(m),
            )),
            _ => None,
        })
        .collect();

    FrontendImplIR {
        self_ty: imp.self_ty.to_token_stream().to_string(),
        trait_name: imp
            .trait_
            .as_ref()
            .map(|(_, path, _)| path.to_token_stream().to_string()),
        methods,
        span: capture_source_span(imp),
    }
}

fn capture_trait(
    t: &syn::ItemTrait,
    hardware_fns: &std::collections::HashSet<String>,
) -> FrontendTraitIR {
    let methods = t
        .items
        .iter()
        .filter_map(|it| match it {
            // A trait method may be a bare signature or carry a default body.
            syn::TraitItem::Fn(m) => Some(capture_fn_from_parts(
                &m.sig,
                m.default.as_ref(),
                hardware_fns,
                capture_source_span(m),
            )),
            _ => None,
        })
        .collect();

    FrontendTraitIR {
        name: t.ident.to_string(),
        methods,
        span: capture_source_span(t),
    }
}

/// Capture a `const` item (`const N: usize = 8;`). Shared by in-body items
/// (`parse_item_stmt`) and file-scope capture (`capture_file_scope`).
fn capture_item_const(const_item: &syn::ItemConst) -> ItemConst {
    ItemConst {
        name: const_item.ident.to_string(),
        ty: RawTypeRef {
            ty_text: const_item.ty.to_token_stream().to_string(),
            span: capture_source_span(&*const_item.ty),
        },
        value_text: const_item.expr.to_token_stream().to_string(),
        attrs: const_item.attrs.iter().map(|a| quote!(#a).to_string()).collect(),
        span: capture_source_span(const_item),
    }
}

/// Capture a `struct` item, named or tuple (unnamed fields become `field_0`,
/// `field_1`, …). Shared by in-body and file-scope capture.
fn capture_item_struct(struct_item: &syn::ItemStruct) -> ItemStruct {
    let fields = match &struct_item.fields {
        syn::Fields::Named(named) => named
            .named
            .iter()
            .map(|f| StructField {
                name: f.ident.as_ref().unwrap().to_string(),
                ty: RawTypeRef {
                    ty_text: f.ty.to_token_stream().to_string(),
                    span: capture_source_span(&f.ty),
                },
                span: capture_source_span(f),
            })
            .collect(),
        syn::Fields::Unnamed(unnamed) => unnamed
            .unnamed
            .iter()
            .enumerate()
            .map(|(idx, f)| StructField {
                name: format!("field_{}", idx),
                ty: RawTypeRef {
                    ty_text: f.ty.to_token_stream().to_string(),
                    span: capture_source_span(&f.ty),
                },
                span: capture_source_span(f),
            })
            .collect(),
        syn::Fields::Unit => Vec::new(),
    };
    ItemStruct {
        name: struct_item.ident.to_string(),
        fields,
        attrs: struct_item.attrs.iter().map(|a| quote!(#a).to_string()).collect(),
        span: capture_source_span(struct_item),
    }
}

fn parse_item_stmt(item: &syn::Item) -> RawStmtKind {
    let item_stmt = match item {
        syn::Item::Const(const_item) => ItemStmt::Const(capture_item_const(const_item)),
        syn::Item::Enum(enum_item) => {
            let name = enum_item.ident.to_string();
            let variants = enum_item
                .variants
                .iter()
                .map(|v| EnumVariant {
                    name: v.ident.to_string(),
                    discriminant: v.discriminant.as_ref().map(|(_, expr)| expr.to_token_stream().to_string()),
                    span: capture_source_span(v),
                })
                .collect();
            let attrs = enum_item.attrs.iter().map(|a| quote!(#a).to_string()).collect();
            ItemStmt::Enum(ItemEnum {
                name,
                variants,
                attrs,
                span: capture_source_span(item),
            })
        }
        syn::Item::Struct(struct_item) => ItemStmt::Struct(capture_item_struct(struct_item)),
        syn::Item::Type(type_item) => {
            let name = type_item.ident.to_string();
            let target_ty = RawTypeRef {
                ty_text: type_item.ty.to_token_stream().to_string(),
                span: capture_source_span(&*type_item.ty),
            };
            let attrs = type_item.attrs.iter().map(|a| quote!(#a).to_string()).collect();
            ItemStmt::Type(ItemType {
                name,
                target_ty,
                attrs,
                span: capture_source_span(item),
            })
        }
        syn::Item::Macro(macro_item) => {
            let name = macro_item
                .ident
                .as_ref()
                .map(|i| i.to_string())
                .unwrap_or_else(|| "_".to_string());
            let body_text = macro_item.mac.tokens.to_string();
            let attrs = macro_item.attrs.iter().map(|a| quote!(#a).to_string()).collect();
            ItemStmt::Macro(ItemMacro {
                name,
                body_text,
                attrs,
                span: capture_source_span(item),
            })
        }
        _ => ItemStmt::Other(ItemOther {
            text: quote!(#item).to_string(),
            span: capture_source_span(item),
        }),
    };

    RawStmtKind::Item(item_stmt)
}

fn parse_expr_stmt(expr: &Expr, has_semi: bool, hardware_fns: &std::collections::HashSet<String>) -> RawStmtKind {
    RawStmtKind::Expr(ExprStmt {
        expr: parse_expr_type(expr, hardware_fns),
        has_semi,
        span: capture_source_span(expr),
    })
}

fn parse_expr_type(expr: &Expr, hardware_fns: &std::collections::HashSet<String>) -> ExprType {
    match expr {
        Expr::Array(e) => ExprType::Array(ExprArray {
            elements: e.elems.iter().map(|e| parse_expr_type(e, hardware_fns)).collect(),
            span: capture_source_span(e),
        }),

        Expr::Assign(e) => ExprType::Assign(ExprAssign {
            left: Box::new(parse_expr_type(&e.left, hardware_fns)),
            right: Box::new(parse_expr_type(&e.right, hardware_fns)),
            span: capture_source_span(e),
        }),

        Expr::Async(e) => ExprType::Async(ExprAsync {
            is_move: e.capture.is_some(),
            block: parse_block_stmts(&e.block, hardware_fns),
            span: capture_source_span(e),
        }),

        Expr::Block(e) => ExprType::Block(ExprBlock {
            stmts: parse_block_stmts(&e.block, hardware_fns),
            span: capture_source_span(e),
        }),

        Expr::Await(e) => ExprType::Await(ExprAwait {
            base: Box::new(parse_expr_type(&e.base, hardware_fns)),
            span: capture_source_span(e),
        }),

        Expr::Binary(e) => ExprType::Binary(ExprBinary {
            left: Box::new(parse_expr_type(&e.left, hardware_fns)),
            op: format_binop(&e.op),
            right: Box::new(parse_expr_type(&e.right, hardware_fns)),
            span: capture_source_span(e),
        }),

        Expr::Call(e) => {
            let is_hardware_module = match &*e.func {
                Expr::Path(p) => p.path.segments.last()
                    .map(|seg| hardware_fns.contains(&seg.ident.to_string()))
                    .unwrap_or(false),
                _ => false,
            };
            ExprType::Call(ExprCall {
                func: Box::new(parse_expr_type(&e.func, hardware_fns)),
                args: e.args.iter().map(|a| parse_expr_type(a, hardware_fns)).collect(),
                is_hardware_module,
                span: capture_source_span(e),
            })
        }

        Expr::Path(e) => ExprType::Path(ExprPath {
            path_text: e.path.to_token_stream().to_string(),
            span: capture_source_span(e),
        }),

        Expr::Cast(e) => ExprType::Cast(ExprCast {
            expr: Box::new(parse_expr_type(&e.expr, hardware_fns)),
            target_ty: RawTypeRef {
                ty_text: e.ty.to_token_stream().to_string(),
                span: capture_source_span(&*e.ty),
            },
            span: capture_source_span(e),
        }),

        Expr::Tuple(e) => ExprType::Tuple(ExprTuple {
            elements: e.elems.iter().map(|elem| parse_expr_type(elem, hardware_fns)).collect(),
            span: capture_source_span(e),
        }),

        Expr::Struct(e) => ExprType::Struct(ExprStruct {
            path_text: e.path.to_token_stream().to_string(),
            fields: e.fields.iter().map(|field| ExprStructField {
                member: field.member.to_token_stream().to_string(),
                expr: Box::new(parse_expr_type(&field.expr, hardware_fns)),
                span: capture_source_span(field),
            }).collect(),
            rest: e.rest.as_ref().map(|rest| Box::new(parse_expr_type(rest, hardware_fns))),
            span: capture_source_span(e),
        }),

        Expr::Break(e) => ExprType::Break(ExprBreak {
            label: e.label.as_ref().map(|label| label.ident.to_string()),
            expr: e.expr.as_ref().map(|expr| Box::new(parse_expr_type(expr, hardware_fns))),
            span: capture_source_span(e),
        }),

        Expr::Continue(e) => ExprType::Continue(ExprContinue {
            label: e.label.as_ref().map(|label| label.ident.to_string()),
            span: capture_source_span(e),
        }),

        Expr::Field(e) => ExprType::Field(ExprField {
            base: Box::new(parse_expr_type(&e.base, hardware_fns)),
            member: match &e.member {
                syn::Member::Named(ident) => ident.to_string(),
                syn::Member::Unnamed(index) => index.index.to_string(),
            },
            span: capture_source_span(e),
        }),

        Expr::Index(e) => ExprType::Index(ExprIndex {
            base: Box::new(parse_expr_type(&e.expr, hardware_fns)),
            index: Box::new(parse_expr_type(&e.index, hardware_fns)),
            span: capture_source_span(e),
        }),

        // Parentheses carry no semantics — unwrap to the inner expression.
        // (Emission re-parenthesizes fully, so grouping is never lost.)
        Expr::Paren(e) => parse_expr_type(&e.expr, hardware_fns),

        Expr::If(e) => ExprType::If(ExprIf {
            condition: Box::new(parse_expr_type(&e.cond, hardware_fns)),
            then_block: parse_block_stmts(&e.then_branch, hardware_fns),
            else_branch: e.else_branch.as_ref().map(|(_, expr)| {
                Box::new(parse_expr_type(expr, hardware_fns))
            }),
            span: capture_source_span(e),
        }),

        Expr::Let(e) => ExprType::Let(ExprLet {
            pattern_text: e.pat.to_token_stream().to_string(),
            expr: Box::new(parse_expr_type(&e.expr, hardware_fns)),
            span: capture_source_span(e),
        }),

        Expr::Lit(e) => ExprType::Lit(ExprLit {
            text: e.to_token_stream().to_string(),
            span: capture_source_span(e),
        }),

        Expr::Loop(e) => ExprType::Loop(ExprLoop {
            body: parse_block_stmts(&e.body, hardware_fns),
            span: capture_source_span(e),
        }),

        Expr::Match(e) => ExprType::Match(ExprMatch {
            scrutinee: Box::new(parse_expr_type(&e.expr, hardware_fns)),
            arms: e.arms.iter().map(|arm| {
                ExprMatchArm {
                    pattern_text: arm.pat.to_token_stream().to_string(),
                    guard: arm.guard.as_ref().map(|(_, g)| Box::new(parse_expr_type(g, hardware_fns))),
                    body: Box::new(parse_expr_type(&arm.body, hardware_fns)),
                    span: capture_source_span(arm),
                }
            }).collect(),
            span: capture_source_span(e),
        }),

        Expr::MethodCall(e) => ExprType::MethodCall(ExprMethodCall {
            receiver: Box::new(parse_expr_type(&e.receiver, hardware_fns)),
            method: e.method.to_string(),
            args: e.args.iter().map(|a| parse_expr_type(a, hardware_fns)).collect(),
            turbofish: capture_method_turbofish(e.turbofish.as_ref()),
            span: capture_source_span(e),
        }),

        Expr::Range(e) => ExprType::Range(ExprRange {
            start: e.start.as_ref().map(|e| Box::new(parse_expr_type(e, hardware_fns))),
            end: e.end.as_ref().map(|e| Box::new(parse_expr_type(e, hardware_fns))),
            inclusive: matches!(e.limits, syn::RangeLimits::Closed(_)),
            span: capture_source_span(e),
        }),

        Expr::Reference(e) => ExprType::Reference(ExprReference {
            is_mut: e.mutability.is_some(),
            expr: Box::new(parse_expr_type(&e.expr, hardware_fns)),
            span: capture_source_span(e),
        }),

        Expr::Repeat(e) => ExprType::Repeat(ExprRepeat {
            expr: Box::new(parse_expr_type(&e.expr, hardware_fns)),
            len: Box::new(parse_expr_type(&e.len, hardware_fns)),
            span: capture_source_span(e),
        }),

        Expr::Return(e) => ExprType::Return(ExprReturn {
            value: e.expr.as_ref().map(|e| Box::new(parse_expr_type(e, hardware_fns))),
            span: capture_source_span(e),
        }),

        Expr::Unary(e) => ExprType::Unary(ExprUnary {
            op: format_unop(&e.op),
            expr: Box::new(parse_expr_type(&e.expr, hardware_fns)),
            span: capture_source_span(e),
        }),

        Expr::While(e) => ExprType::While(ExprWhile {
            condition: Box::new(parse_expr_type(&e.cond, hardware_fns)),
            body: parse_block_stmts(&e.body, hardware_fns),
            span: capture_source_span(e),
        }),

        Expr::Yield(e) => ExprType::Yield(ExprYield {
            value: e.expr.as_ref().map(|e| Box::new(parse_expr_type(e, hardware_fns))),
            span: capture_source_span(e),
        }),

        // `const { ... }` — a compile-time block (e.g. `const { assert!(...) }`).
        // Captured as a real block so lowering can elide it explicitly rather
        // than the block reaching CHIR as an opaque literal.
        Expr::Const(e) => ExprType::Const(ExprConst {
            stmts: parse_block_stmts(&e.block, hardware_fns),
            span: capture_source_span(e),
        }),

        // `expr?` — the try operator.
        Expr::Try(e) => ExprType::Try(ExprTry {
            expr: Box::new(parse_expr_type(&e.expr, hardware_fns)),
            span: capture_source_span(e),
        }),

        // `panic!(...)`, `println!(...)`, etc. — a macro call in expression
        // position. See `classify_raw_stmt_kind` for the statement-position form.
        Expr::Macro(e) => ExprType::Macro(macro_to_expr(&e.mac, capture_source_span(e))),

        // Fallback for unsupported/unhandled expressions
        _ => ExprType::Lit(ExprLit {
            text: quote!(#expr).to_string(),
            span: capture_source_span(expr),
        }),
    }
}

/// Shared lowering of a `syn::Macro` invocation to `ExprMacro`, used from both
/// expression position (`Expr::Macro`) and statement position (`Stmt::Macro`).
/// The macro's delimited tokens are kept verbatim; a macro body is not
/// Rust-expression-shaped, so there is nothing structured to descend into.
fn macro_to_expr(mac: &syn::Macro, span: SourceSpan) -> ExprMacro {
    let name = mac
        .path
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default();
    ExprMacro {
        name,
        tokens_text: mac.tokens.to_string(),
        span,
    }
}

/// Canonical token string per turbofish generic argument on a method call:
/// `read_port::<0>()` → `["0"]`, `part_select::<3>(12)` → `["3"]`. Returns an
/// empty vec when the call has no `::<...>`. Kept as raw arg strings (not parsed
/// to numbers) because a turbofish arg may be a const expression or a type, not
/// only a literal — later phases interpret it per method.
fn capture_method_turbofish(
    turbofish: Option<&syn::AngleBracketedGenericArguments>,
) -> Vec<String> {
    match turbofish {
        Some(tf) => tf
            .args
            .iter()
            .map(|arg| arg.to_token_stream().to_string())
            .collect(),
        None => Vec::new(),
    }
}

fn format_binop(op: &BinOp) -> String {
    match op {
        BinOp::Add(_) => "+".to_string(),
        BinOp::Sub(_) => "-".to_string(),
        BinOp::Mul(_) => "*".to_string(),
        BinOp::Div(_) => "/".to_string(),
        BinOp::Rem(_) => "%".to_string(),
        BinOp::And(_) => "&&".to_string(),
        BinOp::Or(_) => "||".to_string(),
        BinOp::BitXor(_) => "^".to_string(),
        BinOp::BitAnd(_) => "&".to_string(),
        BinOp::BitOr(_) => "|".to_string(),
        BinOp::Shl(_) => "<<".to_string(),
        BinOp::Shr(_) => ">>".to_string(),
        BinOp::Eq(_) => "==".to_string(),
        BinOp::Lt(_) => "<".to_string(),
        BinOp::Le(_) => "<=".to_string(),
        BinOp::Ne(_) => "!=".to_string(),
        BinOp::Ge(_) => ">=".to_string(),
        BinOp::Gt(_) => ">".to_string(),
        BinOp::AddAssign(_) => "+=".to_string(),
        BinOp::SubAssign(_) => "-=".to_string(),
        BinOp::MulAssign(_) => "*=".to_string(),
        BinOp::DivAssign(_) => "/=".to_string(),
        BinOp::RemAssign(_) => "%=".to_string(),
        BinOp::BitXorAssign(_) => "^=".to_string(),
        BinOp::BitAndAssign(_) => "&=".to_string(),
        BinOp::BitOrAssign(_) => "|=".to_string(),
        BinOp::ShlAssign(_) => "<<=".to_string(),
        BinOp::ShrAssign(_) => ">>=".to_string(),
        _ => "?".to_string(),
    }
}

fn format_unop(op: &UnOp) -> String {
    match op {
        UnOp::Deref(_) => "*".to_string(),
        UnOp::Not(_) => "!".to_string(),
        UnOp::Neg(_) => "-".to_string(),
        _ => "?".to_string(),
    }
}

fn extract_explicit_type(local: &syn::Local) -> Option<RawTypeRef> {
    match &local.pat {
        // let x: T = ...
        syn::Pat::Type(pat_ty) => Some(RawTypeRef {
            ty_text: pat_ty.ty.to_token_stream().to_string(),
            span: capture_source_span(&*pat_ty.ty),
        }),
        _ => None,
    }
}

fn infer_local_type_hint_from_init(local: &syn::Local) -> Option<RawTypeRef> {
    let init = local.init.as_ref()?;
    let expr = &*init.expr;

    match expr {
        // let x = foo as T — the cast target is a genuine type annotation.
        syn::Expr::Cast(cast) => Some(RawTypeRef {
            ty_text: cast.ty.to_token_stream().to_string(),
            span: capture_source_span(&*cast.ty),
        }),

        // A constructor call like `Bits::from_u32(1)` is NOT a type: its callee
        // path (`Bits::from_u32`) does not resolve as a type. Its width is
        // inferred in CHIR from the constructor name / turbofish — see
        // `chir_lower::infer_type_from_call`.
        _ => None,
    }
}

fn extract_local_name(local: &syn::Local) -> Option<String> {
    match &local.pat {
        syn::Pat::Ident(pat_ident) => Some(pat_ident.ident.to_string()),
        syn::Pat::Type(pat_ty) => match &*pat_ty.pat {
            syn::Pat::Ident(pat_ident) => Some(pat_ident.ident.to_string()),
            _ => None,
        },
        _ => None,
    }
}

fn capture_source_span(node: &impl Spanned) -> SourceSpan {
    let span = node.span();
    let start = span.start();
    let end = span.end();
    SourceSpan {
        start_line: start.line,
        start_col: start.column,
        end_line: end.line,
        end_col: end.column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    fn compact_ws(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    fn parse_local_from_stmt(stmt: Stmt) -> syn::Local {
        match stmt {
            Stmt::Local(local) => local,
            _ => panic!("expected local statement"),
        }
    }

    #[test]
    fn test_capture_signature_simple() {
        let design_fn: ItemFn = parse_quote! {
            async fn my_module(clk: Clock<MainClk>, input: Arc<Mutex<u8>>) -> u8 {
                // body
            }
        };

        let signature = capture_signature(&design_fn.sig);
        assert_eq!(signature.params.len(), 2);
        assert_eq!(signature.params[0].name, "clk");
        assert_eq!(compact_ws(&signature.params[0].ty.ty_text), "Clock<MainClk>");
        assert_eq!(signature.params[1].name, "input");
        assert_eq!(compact_ws(&signature.params[1].ty.ty_text), "Arc<Mutex<u8>>");
        assert_eq!(
            compact_ws(&signature.return_ty.expect("return type").ty_text),
            "u8"
        );
        // Non-generic module: no generics, no where clause.
        assert!(signature.generics.is_empty());
        assert!(signature.where_clause_text.is_none());
    }

    #[test]
    fn test_capture_signature_const_generics() {
        // shift_register-shaped: `<const N: usize, const N_1: usize>`.
        let design_fn: ItemFn = parse_quote! {
            async fn m<const N: usize, const N_1: usize>(
                clk: Clock<MainClk>,
                out: Out<Bits<N>, MainClk>,
            ) {}
        };

        let sig = capture_signature(&design_fn.sig);
        assert_eq!(sig.generics.len(), 2);

        assert_eq!(sig.generics[0].kind, GenericParamKind::Const);
        assert_eq!(sig.generics[0].name, "N");
        assert_eq!(
            compact_ws(&sig.generics[0].const_ty.as_ref().expect("const ty").ty_text),
            "usize"
        );
        assert!(sig.generics[0].bounds.is_empty());
        assert!(sig.generics[0].default.is_none());

        assert_eq!(sig.generics[1].name, "N_1");
        assert_eq!(sig.generics[1].kind, GenericParamKind::Const);
    }

    #[test]
    fn test_capture_signature_bounded_domain_generic() {
        // sync_2ff-shaped: a type param bounded by ClockDomain, used as a domain.
        let design_fn: ItemFn = parse_quote! {
            async fn sync_2ff<SrcD: ClockDomain>(
                clk: Clock<ClkSlow>,
                d: In<Logic, SrcD>,
                q: Out<Logic, ClkSlow>,
            ) {}
        };

        let sig = capture_signature(&design_fn.sig);
        assert_eq!(sig.generics.len(), 1);
        assert_eq!(sig.generics[0].kind, GenericParamKind::Type);
        assert_eq!(sig.generics[0].name, "SrcD");
        assert!(sig.generics[0].const_ty.is_none());
        assert_eq!(sig.generics[0].bounds, vec!["ClockDomain".to_string()]);
    }

    #[test]
    fn test_capture_signature_where_clause_preserved() {
        let design_fn: ItemFn = parse_quote! {
            async fn m<T>(x: T) where T: ClockDomain {}
        };

        let sig = capture_signature(&design_fn.sig);
        assert_eq!(sig.generics.len(), 1);
        assert_eq!(sig.generics[0].name, "T");
        // Unbounded at the param site; the bound lives in the where clause.
        assert!(sig.generics[0].bounds.is_empty());
        assert_eq!(
            compact_ws(&sig.where_clause_text.expect("where clause")),
            "whereT:ClockDomain"
        );
    }

    // ── File-scope item capture (#7a) ────────────────────────────────────────

    fn parse_file(src: &str) -> syn::File {
        syn::parse_str(src).expect("valid Rust file")
    }

    #[test]
    fn test_capture_file_scope_free_fns_and_consts() {
        let file = parse_file(
            r#"
            const CLKS_PER_BIT: usize = 434;
            fn sign_ext_i(instr: Bits<32>) -> Bits<32> { instr }
            fn decode(instr: Bits<32>) -> Option<InstrDecoded> {
                let opcode = Opcode::from_bits(instr.truncate::<7>())?;
                Some(InstrDecoded { opcode })
            }
            #[hardware(sequential)]
            async fn m(clk: Clock<MainClk>) {}
            "#,
        );
        let hw: std::collections::HashSet<String> = ["m".to_string()].into_iter().collect();
        let scope = capture_file_scope(&file, &hw);

        // Const captured with its value.
        assert_eq!(scope.consts.len(), 1);
        assert_eq!(scope.consts[0].name, "CLKS_PER_BIT");
        assert_eq!(scope.consts[0].value_text, "434");

        // Both free fns captured; the hardware module `m` is excluded.
        let fn_names: Vec<&str> = scope.fns.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(fn_names, vec!["sign_ext_i", "decode"]);

        // The `decode` body is captured structurally (return type, statements),
        // not flattened to opaque text; it exercises `?` + struct-literal return.
        let decode = scope.fns.iter().find(|f| f.name == "decode").unwrap();
        assert!(decode.receiver.is_none());
        assert_eq!(
            compact_ws(&decode.signature.return_ty.as_ref().unwrap().ty_text),
            "Option<InstrDecoded>"
        );
        assert!(!decode.raw_statements.is_empty());
    }

    #[test]
    fn test_capture_file_scope_structs() {
        let file = parse_file(
            r#"
            pub struct InstrDecoded { pub opcode: Opcode, pub rd: usize }
            struct Packet(Logic, Bits<8>);
            "#,
        );
        let scope = capture_file_scope(&file, &Default::default());
        assert_eq!(scope.structs.len(), 2);

        let decoded = &scope.structs[0];
        assert_eq!(decoded.name, "InstrDecoded");
        assert_eq!(decoded.fields.len(), 2);
        assert_eq!(decoded.fields[0].name, "opcode");
        assert_eq!(decoded.fields[1].name, "rd");

        // Tuple struct fields become field_0, field_1.
        let packet = &scope.structs[1];
        assert_eq!(packet.fields[0].name, "field_0");
        assert_eq!(packet.fields[1].name, "field_1");
    }

    #[test]
    fn test_capture_file_scope_impl_methods() {
        let file = parse_file(
            r#"
            impl Opcode {
                pub fn from_bits(op: Bits<7>) -> Option<Self> { None }
            }
            impl ClockDomain for MainClk {}
            "#,
        );
        let scope = capture_file_scope(&file, &Default::default());
        assert_eq!(scope.impls.len(), 2);

        // Inherent impl with an associated fn (no receiver).
        let inherent = &scope.impls[0];
        assert_eq!(inherent.self_ty, "Opcode");
        assert!(inherent.trait_name.is_none());
        assert_eq!(inherent.methods.len(), 1);
        assert_eq!(inherent.methods[0].name, "from_bits");
        assert!(inherent.methods[0].receiver.is_none());

        // Empty marker trait-impl captures harmlessly with no methods.
        let marker = &scope.impls[1];
        assert_eq!(marker.self_ty, "MainClk");
        assert_eq!(marker.trait_name.as_deref(), Some("ClockDomain"));
        assert!(marker.methods.is_empty());
    }

    #[test]
    fn test_capture_file_scope_traits_and_receivers() {
        let file = parse_file(
            r#"
            pub trait ReadOp {
                fn issue_read(&self, addr: usize);
                fn read_data(&self) -> Bits<32>;
                fn reset(&mut self);
            }
            "#,
        );
        let scope = capture_file_scope(&file, &Default::default());
        assert_eq!(scope.traits.len(), 1);

        let t = &scope.traits[0];
        assert_eq!(t.name, "ReadOp");
        assert_eq!(t.methods.len(), 3);
        // Bodyless trait-method decls: empty statements, but receiver captured.
        assert!(t.methods[0].raw_statements.is_empty());
        assert_eq!(t.methods[0].receiver, Some(Receiver::Ref));
        assert_eq!(t.methods[2].receiver, Some(Receiver::RefMut));
    }

    #[test]
    fn test_capture_frontend_ir_has_empty_file_scope() {
        // A FIR captured from an ItemFn alone carries no file-scope items;
        // transpile_source injects them (like enums).
        let design_fn: ItemFn = parse_quote! {
            #[hardware(sequential)]
            async fn m(clk: Clock<MainClk>) {}
        };
        let fir = capture_frontend_ir(&design_fn, &Default::default()).unwrap();
        assert!(fir.file_fns.is_empty());
        assert!(fir.file_structs.is_empty());
        assert!(fir.file_consts.is_empty());
        assert!(fir.file_impls.is_empty());
        assert!(fir.file_traits.is_empty());
    }

    #[test]
    fn test_capture_signature_no_return() {
        let design_fn: ItemFn = parse_quote! {
            async fn my_module(input: Arc<Mutex<u8>>) {
                // body
            }
        };

        let signature = capture_signature(&design_fn.sig);
        assert_eq!(signature.params.len(), 1);
        assert_eq!(signature.params[0].name, "input");
        assert_eq!(compact_ws(&signature.params[0].ty.ty_text), "Arc<Mutex<u8>>");
        assert!(signature.return_ty.is_none());
    }


    #[test]
    fn test_capture_signature_tuple_return() {
        let design_fn: ItemFn = parse_quote! {
            async fn my_module(input: Arc<Mutex<u8>>) -> (u8, u8) {
                // body
            }
        };

        let signature = capture_signature(&design_fn.sig);
        assert_eq!(signature.params.len(), 1);
        assert_eq!(signature.params[0].name, "input");
        assert_eq!(compact_ws(&signature.params[0].ty.ty_text), "Arc<Mutex<u8>>");
        assert_eq!(
            compact_ws(&signature.return_ty.expect("return type").ty_text),
            "(u8,u8)"
        );
    }

    #[test]
    fn test_classify_module_async_and_sync() {
        let async_fn: ItemFn = parse_quote! {
            async fn async_module() -> u8 { 0 }
        };
        let sync_fn: ItemFn = parse_quote! {
            fn sync_module() -> u8 { 0 }
        };

        assert_eq!(
            classify_module(&async_fn),
            FrontendClassification::AsyncSequentialFn
        );
        assert_eq!(
            classify_module(&sync_fn),
            FrontendClassification::CombinationalFn
        );
    }

    #[test]
    fn test_capture_hardware_mode_reads_attribute_arg() {
        let seq: ItemFn = parse_quote! {
            #[hardware(sequential)]
            async fn m(clk: Clock<MainClk>) {}
        };
        let comb: ItemFn = parse_quote! {
            #[hardware(combinational)]
            fn m(d: In<Logic, ()>) {}
        };
        let sync: ItemFn = parse_quote! {
            #[hardware(synchronizer)]
            async fn m(clk: Clock<Slow>) {}
        };

        assert_eq!(capture_hardware_mode(&seq), Some(HardwareMode::Sequential));
        assert_eq!(capture_hardware_mode(&comb), Some(HardwareMode::Combinational));
        // `synchronizer` is async like `sequential`, so only the attribute — not
        // async-ness — can tell them apart. This is the whole point of the field.
        assert_eq!(capture_hardware_mode(&sync), Some(HardwareMode::Synchronizer));
    }

    #[test]
    fn test_capture_hardware_mode_absent_or_unknown_is_none() {
        // No attribute at all (e.g. a bare fn handed to transpile_item_fn).
        let bare: ItemFn = parse_quote! {
            async fn m(clk: Clock<MainClk>) {}
        };
        // Attribute present but an argument we don't recognize — left to the
        // proc-macro to diagnose, not guessed here.
        let unknown: ItemFn = parse_quote! {
            #[hardware(bogus)]
            async fn m(clk: Clock<MainClk>) {}
        };

        assert_eq!(capture_hardware_mode(&bare), None);
        assert_eq!(capture_hardware_mode(&unknown), None);
    }

    #[test]
    fn test_capture_clock_metadata() {
        let design_fn: ItemFn = parse_quote! {
            async fn my_module(clk: Clock<MainClk>, input: Arc<Mutex<u8>>) -> u8 { 0 }
        };

        let clocks = capture_clock_metadata(&design_fn);
        assert_eq!(clocks.len(), 1);
        assert_eq!(clocks[0].param_idx, 0);
        assert_eq!(clocks[0].param_name, "clk");
        assert_eq!(compact_ws(&clocks[0].clock_ty), "Clock<MainClk>");
        assert_eq!(clocks[0].domain_hint.as_deref(), Some("MainClk"));
    }

    #[test]
    fn test_capture_clock_metadata_no_clock() {
        let design_fn: ItemFn = parse_quote! {
            async fn my_module(input: Arc<Mutex<u8>>) -> u8 { 0 }
        };

        let clocks = capture_clock_metadata(&design_fn);
        assert!(clocks.is_empty());
    }

    #[test]
    fn test_capture_raw_statements() {
        let design_fn: ItemFn = parse_quote! {
            async fn my_module(input: Arc<Mutex<u8>>) -> u8 {
                let x = 5;
                println!("Hello");
                input.lock().unwrap().clone()
            }
        };

        let raw_statements = capture_raw_statements(&design_fn, &Default::default());
        assert_eq!(raw_statements.len(), 3);
        assert_eq!(raw_statements[0].order, 0);
        assert!(matches!(raw_statements[0].kind, RawStmtKind::Local(_)));
        assert_eq!(raw_statements[1].order, 1);
        assert!(matches!(raw_statements[1].kind, RawStmtKind::Expr(_)));
        assert_eq!(raw_statements[2].order, 2);
        assert!(matches!(raw_statements[2].kind, RawStmtKind::Expr(_)));
    }

    #[test]
    fn test_parse_local_stmt_explicit_type() {
        let local = parse_local_from_stmt(parse_quote! {
            let mut count: Bits<8> = Bits::<8>::from_u128(0);
        });

        let kind = parse_local_stmt(&local, &Default::default());
        match kind {
            RawStmtKind::Local(local_stmt) => {
                assert!(local_stmt.is_mut);
                assert_eq!(local_stmt.name, "count");
                let ty = local_stmt.ty.expect("explicit type should be captured");
                assert_eq!(compact_ws(&ty.ty_text), "Bits<8>");
            }
            _ => panic!("expected local stmt"),
        }
    }

    #[test]
    fn test_parse_local_stmt_constructor_call_yields_no_type_hint() {
        // A constructor call is not a type annotation: the parser must NOT
        // extract the callee path as a type. Width is inferred later in CHIR.
        let local = parse_local_from_stmt(parse_quote! {
            let mut count = Bits::from_u32(1);
        });

        let kind = parse_local_stmt(&local, &Default::default());
        match kind {
            RawStmtKind::Local(local_stmt) => {
                assert!(local_stmt.is_mut);
                assert_eq!(local_stmt.name, "count");
                assert!(
                    local_stmt.ty.is_none(),
                    "constructor call must not produce a type hint, got {:?}",
                    local_stmt.ty
                );
            }
            _ => panic!("expected local stmt"),
        }
    }

    #[test]
    fn test_parse_local_stmt_prefers_explicit_type_over_inferred_hint() {
        let local = parse_local_from_stmt(parse_quote! {
            let count: u16 = Bits::<8>::from_u128(0);
        });

        let kind = parse_local_stmt(&local, &Default::default());
        match kind {
            RawStmtKind::Local(local_stmt) => {
                let ty = local_stmt.ty.expect("explicit type should be captured");
                assert_eq!(compact_ws(&ty.ty_text), "u16");
            }
            _ => panic!("expected local stmt"),
        }
    }

    #[test]
    fn test_parse_local_stmt_infers_type_hint_from_cast_init() {
        let local = parse_local_from_stmt(parse_quote! {
            let count = input as Bits<8>;
        });

        let kind = parse_local_stmt(&local, &Default::default());
        match kind {
            RawStmtKind::Local(local_stmt) => {
                let ty = local_stmt.ty.expect("cast type hint should be captured");
                assert_eq!(compact_ws(&ty.ty_text), "Bits<8>");
            }
            _ => panic!("expected local stmt"),
        }
    }

    #[test]
    fn test_parse_local_stmt_without_explicit_or_inferred_type_hint() {
        let local = parse_local_from_stmt(parse_quote! {
            let count = input;
        });

        let kind = parse_local_stmt(&local, &Default::default());
        match kind {
            RawStmtKind::Local(local_stmt) => {
                assert!(!local_stmt.is_mut);
                assert_eq!(local_stmt.name, "count");
                assert!(local_stmt.ty.is_none());
            }
            _ => panic!("expected local stmt"),
        }
    }

    #[test]
    fn test_classify_raw_stmt_kind_item_const() {
        let stmt: Stmt = parse_quote! {
            const WIDTH: usize = 8;
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Const(const_item)) => {
                assert_eq!(const_item.name, "WIDTH");
                assert_eq!(compact_ws(&const_item.ty.ty_text), "usize");
            }
            _ => panic!("expected const item stmt"),
        }
    }

    #[test]
    fn test_classify_raw_stmt_kind_item_enum() {
        let stmt: Stmt = parse_quote! {
            enum State { Idle, Busy }
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Enum(enum_item)) => {
                assert_eq!(enum_item.name, "State");
                assert_eq!(enum_item.variants.len(), 2);
                assert_eq!(enum_item.variants[0].name, "Idle");
                assert_eq!(enum_item.variants[1].name, "Busy");
            }
            _ => panic!("expected enum item stmt"),
        }
    }

    #[test]
    fn test_classify_raw_stmt_kind_item_macro() {
        let stmt: Stmt = parse_quote! {
            macro_rules! my_macro {
                () => { 0 };
            }
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Macro(macro_item)) => {
                assert_eq!(macro_item.name, "my_macro");
            }
            _ => panic!("expected macro item stmt"),
        }
    }

    #[test]
    fn test_classify_raw_stmt_kind_item_struct() {
        let stmt: Stmt = parse_quote! {
            struct Reg8 { value: u8 }
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Struct(struct_item)) => {
                assert_eq!(struct_item.name, "Reg8");
                assert_eq!(struct_item.fields.len(), 1);
                assert_eq!(struct_item.fields[0].name, "value");
                assert_eq!(compact_ws(&struct_item.fields[0].ty.ty_text), "u8");
            }
            _ => panic!("expected struct item stmt"),
        }
    }

    #[test]
    fn test_classify_raw_stmt_kind_item_type() {
        let stmt: Stmt = parse_quote! {
            type Word = u16;
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Type(type_item)) => {
                assert_eq!(type_item.name, "Word");
                assert_eq!(compact_ws(&type_item.target_ty.ty_text), "u16");
            }
            _ => panic!("expected type item stmt"),
        }
    }

    #[test]
    fn test_classify_raw_stmt_kind_item_other() {
        let stmt: Stmt = parse_quote! {
            fn helper() {}
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Other(_)) => {
                // Successfully matched Other variant
            }
            _ => panic!("expected other item stmt"),
        }
    }

    // ========== Item Statement Content Tests ==========

    #[test]
    fn test_parse_item_const_validates_name_and_type() {
        let stmt: Stmt = parse_quote! {
            const BUFFER_SIZE: usize = 256;
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Const(const_item)) => {
                assert_eq!(const_item.name, "BUFFER_SIZE");
                assert_eq!(compact_ws(&const_item.ty.ty_text), "usize");
                assert!(const_item.value_text.contains("256"));
            }
            _ => panic!("expected const item"),
        }
    }

    #[test]
    fn test_parse_item_const_with_attributes() {
        let stmt: Stmt = parse_quote! {
            #[doc = "A constant"]
            const WIDTH: u32 = 8;
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Const(const_item)) => {
                assert_eq!(const_item.name, "WIDTH");
                assert!(!const_item.attrs.is_empty());
            }
            _ => panic!("expected const item"),
        }
    }

    #[test]
    fn test_parse_item_enum_validates_name_and_variants() {
        let stmt: Stmt = parse_quote! {
            enum State { Idle, Busy, Done }
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Enum(enum_item)) => {
                assert_eq!(enum_item.name, "State");
                assert_eq!(enum_item.variants.len(), 3);
                assert_eq!(enum_item.variants[0].name, "Idle");
                assert_eq!(enum_item.variants[1].name, "Busy");
                assert_eq!(enum_item.variants[2].name, "Done");
            }
            _ => panic!("expected enum item"),
        }
    }

    #[test]
    fn test_parse_item_enum_with_discriminants() {
        let stmt: Stmt = parse_quote! {
            enum Code { Success = 0, Error = 1, Pending = 2 }
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Enum(enum_item)) => {
                assert_eq!(enum_item.name, "Code");
                assert_eq!(enum_item.variants.len(), 3);
                // Verify discriminants are captured
                assert!(enum_item.variants[0].discriminant.is_some());
                assert!(enum_item.variants[1].discriminant.is_some());
                assert!(enum_item.variants[2].discriminant.is_some());
            }
            _ => panic!("expected enum item"),
        }
    }

    #[test]
    fn test_parse_item_struct_validates_fields() {
        let stmt: Stmt = parse_quote! {
            struct Register { value: u32, mask: u32 }
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Struct(struct_item)) => {
                assert_eq!(struct_item.name, "Register");
                assert_eq!(struct_item.fields.len(), 2);
                assert_eq!(struct_item.fields[0].name, "value");
                assert_eq!(compact_ws(&struct_item.fields[0].ty.ty_text), "u32");
                assert_eq!(struct_item.fields[1].name, "mask");
                assert_eq!(compact_ws(&struct_item.fields[1].ty.ty_text), "u32");
            }
            _ => panic!("expected struct item"),
        }
    }

    #[test]
    fn test_parse_item_struct_tuple_fields() {
        let stmt: Stmt = parse_quote! {
            struct Point(i32, i32);
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Struct(struct_item)) => {
                assert_eq!(struct_item.name, "Point");
                assert_eq!(struct_item.fields.len(), 2);
                // Tuple fields get synthetic names
                assert_eq!(struct_item.fields[0].name, "field_0");
                assert_eq!(struct_item.fields[1].name, "field_1");
                assert_eq!(compact_ws(&struct_item.fields[0].ty.ty_text), "i32");
                assert_eq!(compact_ws(&struct_item.fields[1].ty.ty_text), "i32");
            }
            _ => panic!("expected struct item"),
        }
    }

    #[test]
    fn test_parse_item_struct_unit() {
        let stmt: Stmt = parse_quote! {
            struct Marker;
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Struct(struct_item)) => {
                assert_eq!(struct_item.name, "Marker");
                assert_eq!(struct_item.fields.len(), 0);
            }
            _ => panic!("expected struct item"),
        }
    }

    #[test]
    fn test_parse_item_struct_complex_types() {
        let stmt: Stmt = parse_quote! {
            struct Container { 
                inner: Box<Vec<u8>>,
                config: Arc<Mutex<Config>>
            }
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Struct(struct_item)) => {
                assert_eq!(struct_item.name, "Container");
                assert_eq!(struct_item.fields.len(), 2);
                assert_eq!(struct_item.fields[0].name, "inner");
                assert_eq!(struct_item.fields[1].name, "config");
                // Verify complex types are preserved
                assert!(struct_item.fields[0].ty.ty_text.contains("Box"));
                assert!(struct_item.fields[1].ty.ty_text.contains("Arc"));
            }
            _ => panic!("expected struct item"),
        }
    }

    #[test]
    fn test_parse_item_type_validates_alias() {
        let stmt: Stmt = parse_quote! {
            type Byte = u8;
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Type(type_item)) => {
                assert_eq!(type_item.name, "Byte");
                assert_eq!(compact_ws(&type_item.target_ty.ty_text), "u8");
            }
            _ => panic!("expected type item"),
        }
    }

    #[test]
    fn test_parse_item_type_complex_alias() {
        let stmt: Stmt = parse_quote! {
            type IntMap = std::collections::HashMap<String, i32>;
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Type(type_item)) => {
                assert_eq!(type_item.name, "IntMap");
                // Verify complex type is preserved
                assert!(type_item.target_ty.ty_text.contains("HashMap"));
            }
            _ => panic!("expected type item"),
        }
    }

    #[test]
    fn test_parse_item_type_generic_alias() {
        let stmt: Stmt = parse_quote! {
            type Result<T> = std::result::Result<T, Error>;
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Type(type_item)) => {
                assert_eq!(type_item.name, "Result");
                // Verify generic syntax is preserved
                assert!(type_item.target_ty.ty_text.contains("Error"));
            }
            _ => panic!("expected type item"),
        }
    }

    #[test]
    fn test_parse_item_macro_simple() {
        let stmt: Stmt = parse_quote! {
            macro_rules! assert_hw {
                ($cond:expr) => {
                    if !($cond) { panic!("assertion failed"); }
                };
            }
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Macro(macro_item)) => {
                assert_eq!(macro_item.name, "assert_hw");
                assert!(!macro_item.body_text.is_empty());
            }
            _ => panic!("expected macro item"),
        }
    }

    #[test]
    fn test_parse_item_multiple_enum_variants() {
        let stmt: Stmt = parse_quote! {
            enum Signal {
                Clock(u32),
                Reset,
                Enable(bool),
                Data(Vec<u8>),
            }
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Enum(enum_item)) => {
                assert_eq!(enum_item.name, "Signal");
                assert_eq!(enum_item.variants.len(), 4);
                assert_eq!(enum_item.variants[0].name, "Clock");
                assert_eq!(enum_item.variants[1].name, "Reset");
                assert_eq!(enum_item.variants[2].name, "Enable");
                assert_eq!(enum_item.variants[3].name, "Data");
            }
            _ => panic!("expected enum item"),
        }
    }

    #[test]
    fn test_parse_item_struct_with_attributes() {
        let stmt: Stmt = parse_quote! {
            #[derive(Clone, Copy)]
            struct Point { x: i32, y: i32 }
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Item(ItemStmt::Struct(struct_item)) => {
                assert_eq!(struct_item.name, "Point");
                assert!(!struct_item.attrs.is_empty());
                assert_eq!(struct_item.fields.len(), 2);
            }
            _ => panic!("expected struct item"),
        }
    }

    #[test]
    fn test_classify_raw_stmt_kind_expr_tracks_semi() {
        let stmt: Stmt = parse_quote! {
            foo();
        };

        let kind = classify_raw_stmt_kind(&stmt, &Default::default());
        match kind {
            RawStmtKind::Expr(expr_stmt) => {
                assert!(expr_stmt.has_semi);
            }
            _ => panic!("expected expr stmt"),
        }
    }

    // ========== Expression Type Tests ==========
    
    #[test]
    fn test_parse_expr_array() {
        let expr: Expr = parse_quote!([1, 2, 3]);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Array(arr) => {
                assert_eq!(arr.elements.len(), 3);
                match &arr.elements[0] {
                    ExprType::Lit(_) => {},
                    _ => panic!("expected literal"),
                }
            }
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn test_parse_expr_assign() {
        let expr: Expr = parse_quote!(x = 42);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Assign(assign) => {
                assert!(matches!(*assign.left, ExprType::Path(_)));
                assert!(matches!(*assign.right, ExprType::Lit(_)));
            }
            _ => panic!("expected assign"),
        }
    }

    #[test]
    fn test_parse_expr_async() {
        let expr: Expr = parse_quote!(async {
            let x = 5;
            x + 1
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Async(async_expr) => {
                assert_eq!(async_expr.block.len(), 2);
                assert!(!async_expr.is_move);
            }
            _ => panic!("expected async"),
        }
    }

    #[test]
    fn test_parse_expr_async_move() {
        let expr: Expr = parse_quote!(async move {
            let x = 5;
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Async(async_expr) => {
                assert!(async_expr.is_move);
            }
            _ => panic!("expected async"),
        }
    }

    #[test]
    fn test_parse_expr_await() {
        let expr: Expr = parse_quote!(foo.await);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Await(await_expr) => {
                assert!(matches!(*await_expr.base, ExprType::Path(_)));
            }
            _ => panic!("expected await"),
        }
    }

    #[test]
    fn test_parse_expr_paren_unwraps_to_inner() {
        // `(a >> 1)` must parse to the inner binary, not a raw `Lit` fallback.
        let expr: Expr = parse_quote!((a >> 1));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Binary(bin) => assert_eq!(bin.op, ">>"),
            other => panic!("expected inner binary, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_expr_binary_add() {
        let expr: Expr = parse_quote!(a + b);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Binary(bin) => {
                assert_eq!(bin.op, "+");
                assert!(matches!(*bin.left, ExprType::Path(_)));
                assert!(matches!(*bin.right, ExprType::Path(_)));
            }
            _ => panic!("expected binary"),
        }
    }

    #[test]
    fn test_parse_expr_binary_bitwise_or() {
        let expr: Expr = parse_quote!(a | b);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Binary(bin) => {
                assert_eq!(bin.op, "|");
            }
            _ => panic!("expected binary"),
        }
    }

    #[test]
    fn test_parse_expr_binary_logical_and() {
        let expr: Expr = parse_quote!(a && b);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Binary(bin) => {
                assert_eq!(bin.op, "&&");
            }
            _ => panic!("expected binary"),
        }
    }

    #[test]
    fn test_parse_expr_call() {
        let expr: Expr = parse_quote!(foo(1, 2));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Call(call) => {
                assert_eq!(call.args.len(), 2);
                assert!(matches!(&call.args[0], ExprType::Lit(_)));
                assert!(matches!(&call.args[1], ExprType::Lit(_)));
            }
            _ => panic!("expected call"),
        }
    }

    #[test]
    fn test_parse_expr_path() {
        let expr: Expr = parse_quote!(clk);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Path(path) => {
                assert_eq!(compact_ws(&path.path_text), "clk");
            }
            _ => panic!("expected path"),
        }
    }

    #[test]
    fn test_parse_expr_tuple() {
        let expr: Expr = parse_quote!((a, b));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Tuple(tuple) => {
                assert_eq!(tuple.elements.len(), 2);
                assert!(matches!(tuple.elements[0], ExprType::Path(_)));
                assert!(matches!(tuple.elements[1], ExprType::Path(_)));
            }
            _ => panic!("expected tuple"),
        }
    }

    #[test]
    fn test_parse_expr_struct() {
        let expr: Expr = parse_quote!(Point { x: a, y: b });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Struct(strukt) => {
                assert_eq!(compact_ws(&strukt.path_text), "Point");
                assert_eq!(strukt.fields.len(), 2);
                assert_eq!(strukt.fields[0].member, "x");
                assert_eq!(strukt.fields[1].member, "y");
            }
            _ => panic!("expected struct"),
        }
    }

    #[test]
    fn test_parse_expr_break() {
        let expr: Expr = parse_quote!(break 'outer value);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Break(brk) => {
                assert_eq!(brk.label.as_deref(), Some("outer"));
                assert!(brk.expr.is_some());
            }
            _ => panic!("expected break"),
        }
    }

    #[test]
    fn test_parse_expr_continue() {
        let expr: Expr = parse_quote!(continue 'outer);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Continue(cont) => {
                assert_eq!(cont.label.as_deref(), Some("outer"));
            }
            _ => panic!("expected continue"),
        }
    }

    #[test]
    fn test_parse_expr_cast() {
        let expr: Expr = parse_quote!(x as u32);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Cast(cast) => {
                assert!(matches!(*cast.expr, ExprType::Path(_)));
                assert_eq!(compact_ws(&cast.target_ty.ty_text), "u32");
            }
            _ => panic!("expected cast"),
        }
    }

    #[test]
    fn test_parse_expr_field() {
        let expr: Expr = parse_quote!(obj.field);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Field(field) => {
                assert_eq!(field.member, "field");
                assert!(matches!(*field.base, ExprType::Path(_)));
            }
            _ => panic!("expected field"),
        }
    }

    #[test]
    fn test_parse_expr_if() {
        let expr: Expr = parse_quote!(if x > 0 { 1 } else { 2 });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::If(if_expr) => {
                assert!(matches!(*if_expr.condition, ExprType::Binary(_)));
                assert_eq!(if_expr.then_block.len(), 1);
                assert!(if_expr.else_branch.is_some());
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn test_parse_expr_if_without_else() {
        let expr: Expr = parse_quote!(if x > 0 { 1 });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::If(if_expr) => {
                assert!(matches!(*if_expr.condition, ExprType::Binary(_)));
                assert_eq!(if_expr.then_block.len(), 1);
                assert!(if_expr.else_branch.is_none());
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn test_parse_expr_let() {
        let expr: Expr = parse_quote!(let x = foo());
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Let(let_expr) => {
                // pattern_text contains the pattern, may have various formats
                assert!(!let_expr.pattern_text.is_empty());
                assert!(matches!(*let_expr.expr, ExprType::Call(_)));
            }
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn test_parse_expr_lit_int() {
        let expr: Expr = parse_quote!(42);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Lit(lit) => {
                assert_eq!(compact_ws(&lit.text), "42");
            }
            _ => panic!("expected lit"),
        }
    }

    #[test]
    fn test_parse_expr_lit_string() {
        let expr: Expr = parse_quote!("hello");
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Lit(lit) => {
                assert!(lit.text.contains("hello"));
            }
            _ => panic!("expected lit"),
        }
    }

    #[test]
    fn test_parse_expr_loop() {
        let expr: Expr = parse_quote!(loop {
            if x { break; }
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Loop(loop_expr) => {
                assert_eq!(loop_expr.body.len(), 1);
            }
            _ => panic!("expected loop"),
        }
    }

    #[test]
    fn test_parse_expr_match() {
        let expr: Expr = parse_quote!(match x {
            1 => "one",
            2 => "two",
            _ => "other",
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Match(match_expr) => {
                assert_eq!(match_expr.arms.len(), 3);
                assert!(matches!(*match_expr.scrutinee, ExprType::Path(_)));
                assert!(!match_expr.arms[0].pattern_text.is_empty());
            }
            _ => panic!("expected match"),
        }
    }

    #[test]
    fn test_parse_expr_match_with_guard() {
        let expr: Expr = parse_quote!(match x {
            1 if x > 0 => "positive",
            _ => "other",
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Match(match_expr) => {
                assert!(match_expr.arms[0].guard.is_some());
            }
            _ => panic!("expected match"),
        }
    }

    #[test]
    fn test_parse_expr_method_call() {
        let expr: Expr = parse_quote!(obj.method(1, 2));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::MethodCall(method) => {
                assert_eq!(method.method, "method");
                assert_eq!(method.args.len(), 2);
                assert!(matches!(*method.receiver, ExprType::Path(_)));
            }
            _ => panic!("expected method call"),
        }
    }

    #[test]
    fn test_parse_expr_range_exclusive() {
        let expr: Expr = parse_quote!(1..10);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Range(range) => {
                assert!(range.start.is_some());
                assert!(range.end.is_some());
                assert!(!range.inclusive);
            }
            _ => panic!("expected range"),
        }
    }

    #[test]
    fn test_parse_expr_range_inclusive() {
        let expr: Expr = parse_quote!(1..=10);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Range(range) => {
                assert!(range.inclusive);
            }
            _ => panic!("expected range"),
        }
    }

    #[test]
    fn test_parse_expr_range_open_start() {
        let expr: Expr = parse_quote!(..10);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Range(range) => {
                assert!(range.start.is_none());
                assert!(range.end.is_some());
            }
            _ => panic!("expected range"),
        }
    }

    #[test]
    fn test_parse_expr_range_open_end() {
        let expr: Expr = parse_quote!(1..);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Range(range) => {
                assert!(range.start.is_some());
                assert!(range.end.is_none());
            }
            _ => panic!("expected range"),
        }
    }

    #[test]
    fn test_parse_expr_reference_immut() {
        let expr: Expr = parse_quote!(&x);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Reference(ref_expr) => {
                assert!(!ref_expr.is_mut);
                assert!(matches!(*ref_expr.expr, ExprType::Path(_)));
            }
            _ => panic!("expected reference"),
        }
    }

    #[test]
    fn test_parse_expr_reference_mut() {
        let expr: Expr = parse_quote!(&mut x);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Reference(ref_expr) => {
                assert!(ref_expr.is_mut);
                assert!(matches!(*ref_expr.expr, ExprType::Path(_)));
            }
            _ => panic!("expected reference"),
        }
    }

    #[test]
    fn test_parse_expr_repeat() {
        let expr: Expr = parse_quote!([0; 8]);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Repeat(repeat) => {
                assert!(matches!(*repeat.expr, ExprType::Lit(_)));
                assert!(matches!(*repeat.len, ExprType::Lit(_)));
            }
            _ => panic!("expected repeat"),
        }
    }

    #[test]
    fn test_parse_expr_return_with_value() {
        let expr: Expr = parse_quote!(return x);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Return(ret) => {
                assert!(ret.value.is_some());
            }
            _ => panic!("expected return"),
        }
    }

    #[test]
    fn test_parse_expr_return_without_value() {
        let expr: Expr = parse_quote!(return);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Return(ret) => {
                assert!(ret.value.is_none());
            }
            _ => panic!("expected return"),
        }
    }

    #[test]
    fn test_parse_expr_unary_neg() {
        let expr: Expr = parse_quote!(-x);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Unary(unary) => {
                assert_eq!(unary.op, "-");
                assert!(matches!(*unary.expr, ExprType::Path(_)));
            }
            _ => panic!("expected unary"),
        }
    }

    #[test]
    fn test_parse_expr_unary_not() {
        let expr: Expr = parse_quote!(!flag);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Unary(unary) => {
                assert_eq!(unary.op, "!");
            }
            _ => panic!("expected unary"),
        }
    }

    #[test]
    fn test_parse_expr_unary_deref() {
        let expr: Expr = parse_quote!(*ptr);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Unary(unary) => {
                assert_eq!(unary.op, "*");
            }
            _ => panic!("expected unary"),
        }
    }

    #[test]
    fn test_parse_expr_while() {
        let expr: Expr = parse_quote!(while x > 0 {
            x -= 1;
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::While(while_expr) => {
                assert!(matches!(*while_expr.condition, ExprType::Binary(_)));
                assert_eq!(while_expr.body.len(), 1);
            }
            _ => panic!("expected while"),
        }
    }

    #[test]
    fn test_parse_expr_yield_with_value() {
        let expr: Expr = parse_quote!(yield x);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Yield(yield_expr) => {
                assert!(yield_expr.value.is_some());
            }
            _ => panic!("expected yield"),
        }
    }

    #[test]
    fn test_parse_expr_yield_without_value() {
        let expr: Expr = parse_quote!(yield);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Yield(yield_expr) => {
                assert!(yield_expr.value.is_none());
            }
            _ => panic!("expected yield"),
        }
    }

    #[test]
    fn test_parse_expr_nested_binary_and_call() {
        let expr: Expr = parse_quote!(foo(a + b));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Call(call) => {
                assert_eq!(call.args.len(), 1);
                assert!(matches!(&call.args[0], ExprType::Binary(_)));
            }
            _ => panic!("expected call"),
        }
    }

    #[test]
    fn test_parse_expr_deeply_nested() {
        let expr: Expr = parse_quote!(if foo(a + b) { (x as u32).field } else { 42 });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::If(_) => {
                // Just verify it parses without panicking
            }
            _ => panic!("expected if"),
        }
    }

    // ========== Content Validation Tests ==========
    
    #[test]
    fn test_parse_expr_if_validates_condition_type() {
        let expr: Expr = parse_quote!(if x > 5 { 1 });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::If(if_expr) => {
                // Verify condition is a binary operation
                assert!(matches!(*if_expr.condition, ExprType::Binary(ref b) if b.op == ">"));
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn test_parse_expr_if_validates_then_block_statement_count() {
        let expr: Expr = parse_quote!(if x > 5 { 
            let a = 1;
            a + 2
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::If(if_expr) => {
                assert_eq!(if_expr.then_block.len(), 2);
                assert!(matches!(if_expr.then_block[0].kind, RawStmtKind::Local(_)));
                assert!(matches!(if_expr.then_block[1].kind, RawStmtKind::Expr(_)));
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn test_parse_expr_if_else_validates_else_expr_type() {
        let expr: Expr = parse_quote!(if x > 0 { 1 } else { 2 });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::If(if_expr) => {
                assert!(if_expr.else_branch.is_some());
                // The else branch `else { 2 }` is parsed as a Block containing the literal
                assert!(matches!(**if_expr.else_branch.as_ref().unwrap(), ExprType::Block(_)));
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn test_parse_expr_if_else_if_chain() {
        let expr: Expr = parse_quote!(if x > 0 { 1 } else if x < 0 { 2 } else { 3 });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::If(if_expr) => {
                // else_branch should be another If
                assert!(matches!(**if_expr.else_branch.as_ref().unwrap(), ExprType::If(_)));
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn test_parse_expr_match_validates_arm_bodies() {
        let expr: Expr = parse_quote!(match status {
            0 => success(),
            1 => error(),
            _ => unknown(),
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Match(match_expr) => {
                assert_eq!(match_expr.arms.len(), 3);
                // All arms should be calls
                assert!(matches!(*match_expr.arms[0].body, ExprType::Call(_)));
                assert!(matches!(*match_expr.arms[1].body, ExprType::Call(_)));
                assert!(matches!(*match_expr.arms[2].body, ExprType::Call(_)));
            }
            _ => panic!("expected match"),
        }
    }

    #[test]
    fn test_parse_expr_match_arm_with_guard_validates_guard_expr() {
        let expr: Expr = parse_quote!(match x {
            val if val > 10 => "big",
            _ => "small",
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Match(match_expr) => {
                let guard = match_expr.arms[0].guard.as_ref().unwrap();
                assert!(matches!(**guard, ExprType::Binary(ref b) if b.op == ">"));
            }
            _ => panic!("expected match"),
        }
    }

    #[test]
    fn test_parse_expr_call_validates_arg_types() {
        let expr: Expr = parse_quote!(foo(42, "string", x + y));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Call(call) => {
                assert_eq!(call.args.len(), 3);
                assert!(matches!(call.args[0], ExprType::Lit(_)));
                assert!(matches!(call.args[1], ExprType::Lit(_)));
                assert!(matches!(call.args[2], ExprType::Binary(_)));
            }
            _ => panic!("expected call"),
        }
    }

    #[test]
    fn test_parse_expr_call_validates_func_expr() {
        let expr: Expr = parse_quote!(obj.method()(1));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Call(call) => {
                // func should be a method call (obj.method())
                assert!(matches!(*call.func, ExprType::MethodCall(_)));
            }
            _ => panic!("expected call"),
        }
    }

    #[test]
    fn test_parse_expr_binary_validates_operands() {
        let expr: Expr = parse_quote!(foo(1) + bar(2));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Binary(bin) => {
                assert_eq!(bin.op, "+");
                assert!(matches!(*bin.left, ExprType::Call(_)));
                assert!(matches!(*bin.right, ExprType::Call(_)));
            }
            _ => panic!("expected binary"),
        }
    }

    #[test]
    fn test_parse_expr_binary_validates_operator_string() {
        let expr: Expr = parse_quote!(a << b);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Binary(bin) => {
                assert_eq!(bin.op, "<<");
            }
            _ => panic!("expected binary"),
        }
    }

    #[test]
    fn test_parse_expr_const_block() {
        // `const { assert!(...) }` — the compile-time check form used by
        // shift_register/mux. Captured as a real block, not opaque text.
        let expr: Expr = parse_quote!(const { assert!(N == 8) });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Const(c) => {
                assert_eq!(c.stmts.len(), 1);
                // The inner `assert!` is itself a macro statement.
                match &c.stmts[0].kind {
                    RawStmtKind::Expr(es) => match &es.expr {
                        ExprType::Macro(m) => assert_eq!(m.name, "assert"),
                        other => panic!("expected assert! macro, got {other:?}"),
                    },
                    other => panic!("expected expr stmt, got {other:?}"),
                }
            }
            other => panic!("expected const block, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_expr_try_operator() {
        // `expr?` — used inside decode()'s `Opcode::from_bits(...)?`.
        let expr: Expr = parse_quote!(Opcode::from_bits(x)?);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Try(t) => assert!(matches!(*t.expr, ExprType::Call(_))),
            other => panic!("expected try, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_expr_macro_invocation() {
        // `panic!(...)` in expression position (rv32i_cpu decode arm).
        let expr: Expr = parse_quote!(panic!("bad instr 0x{:x}", w));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Macro(m) => {
                assert_eq!(m.name, "panic");
                assert!(m.tokens_text.contains("bad instr"), "tokens: {}", m.tokens_text);
            }
            other => panic!("expected macro, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_expr_cast_validates_target_type() {
        let expr: Expr = parse_quote!(foo() as Vec<String>);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Cast(cast) => {
                assert!(matches!(*cast.expr, ExprType::Call(_)));
                assert_eq!(compact_ws(&cast.target_ty.ty_text), "Vec<String>");
            }
            _ => panic!("expected cast"),
        }
    }

    #[test]
    fn test_parse_expr_field_validates_base_expr() {
        let expr: Expr = parse_quote!(foo().bar.baz);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Field(field) => {
                assert_eq!(field.member, "baz");
                // base should be another Field
                assert!(matches!(*field.base, ExprType::Field(ref f) if f.member == "bar"));
            }
            _ => panic!("expected field"),
        }
    }

    #[test]
    fn test_parse_expr_array_validates_element_types() {
        let expr: Expr = parse_quote!([1, foo(), x + y, &z]);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Array(arr) => {
                assert_eq!(arr.elements.len(), 4);
                assert!(matches!(arr.elements[0], ExprType::Lit(_)));
                assert!(matches!(arr.elements[1], ExprType::Call(_)));
                assert!(matches!(arr.elements[2], ExprType::Binary(_)));
                assert!(matches!(arr.elements[3], ExprType::Reference(_)));
            }
            _ => panic!("expected array"),
        }
    }

    #[test]
    fn test_parse_expr_reference_validates_nested_content() {
        let expr: Expr = parse_quote!(&obj.method());
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Reference(ref_expr) => {
                // expr should be a method call
                assert!(matches!(*ref_expr.expr, ExprType::MethodCall(_)));
            }
            _ => panic!("expected reference"),
        }
    }

    #[test]
    fn test_parse_expr_repeat_validates_array_elements() {
        let expr: Expr = parse_quote!([foo(x); 5]);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Repeat(repeat) => {
                assert!(matches!(*repeat.expr, ExprType::Call(_)));
                assert!(matches!(*repeat.len, ExprType::Lit(_)));
            }
            _ => panic!("expected repeat"),
        }
    }

    #[test]
    fn test_parse_expr_loop_validates_body_structure() {
        let expr: Expr = parse_quote!(loop {
            let count = 0;
            if done { break; }
            count += 1;
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Loop(loop_expr) => {
                assert_eq!(loop_expr.body.len(), 3);
                assert!(matches!(loop_expr.body[0].kind, RawStmtKind::Local(_)));
                assert!(matches!(loop_expr.body[1].kind, RawStmtKind::Expr(_)));
                assert!(matches!(loop_expr.body[2].kind, RawStmtKind::Expr(_)));
            }
            _ => panic!("expected loop"),
        }
    }

    #[test]
    fn test_parse_expr_while_validates_condition_and_body() {
        let expr: Expr = parse_quote!(while x < 10 {
            x = x + 1;
            process(x);
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::While(while_expr) => {
                assert!(matches!(*while_expr.condition, ExprType::Binary(ref b) if b.op == "<"));
                assert_eq!(while_expr.body.len(), 2);
            }
            _ => panic!("expected while"),
        }
    }

    #[test]
    fn test_parse_expr_async_block_validates_statement_count() {
        let expr: Expr = parse_quote!(async {
            let x = 1;
            let y = 2;
            foo(x, y).await;
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Async(async_expr) => {
                assert_eq!(async_expr.block.len(), 3);
                assert!(matches!(async_expr.block[0].kind, RawStmtKind::Local(_)));
                assert!(matches!(async_expr.block[1].kind, RawStmtKind::Local(_)));
                assert!(matches!(async_expr.block[2].kind, RawStmtKind::Expr(_)));
            }
            _ => panic!("expected async"),
        }
    }

    #[test]
    fn test_parse_expr_method_call_validates_receiver_and_args() {
        let expr: Expr = parse_quote!(obj.process(x, y as u32));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::MethodCall(method) => {
                assert_eq!(method.method, "process");
                assert_eq!(method.args.len(), 2);
                assert!(matches!(method.args[0], ExprType::Path(_)));
                assert!(matches!(method.args[1], ExprType::Cast(_)));
                // No `::<...>` on this call.
                assert!(method.turbofish.is_empty());
            }
            _ => panic!("expected method call"),
        }
    }

    #[test]
    fn test_parse_expr_method_call_captures_turbofish() {
        // Memory port index: `memory.read_port::<0>()` — the `0` must survive.
        let expr: Expr = parse_quote!(memory.read_port::<0>());
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::MethodCall(method) => {
                assert_eq!(method.method, "read_port");
                assert!(method.args.is_empty());
                assert_eq!(method.turbofish, vec!["0".to_string()]);
            }
            _ => panic!("expected method call"),
        }
    }

    #[test]
    fn test_parse_expr_method_call_turbofish_and_args_are_distinct() {
        // `instr.part_select::<3>(12)` — width `3` is turbofish, offset `12` is an arg.
        let expr: Expr = parse_quote!(instr.part_select::<3>(12));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::MethodCall(method) => {
                assert_eq!(method.method, "part_select");
                assert_eq!(method.turbofish, vec!["3".to_string()]);
                assert_eq!(method.args.len(), 1);
                assert!(matches!(method.args[0], ExprType::Lit(_)));
            }
            _ => panic!("expected method call"),
        }
    }

    #[test]
    fn test_parse_expr_unary_validates_operand() {
        let expr: Expr = parse_quote!(!foo(x));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Unary(unary) => {
                assert_eq!(unary.op, "!");
                assert!(matches!(*unary.expr, ExprType::Call(_)));
            }
            _ => panic!("expected unary"),
        }
    }

    #[test]
    fn test_parse_expr_assign_validates_both_sides() {
        let expr: Expr = parse_quote!(x.field = foo());
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Assign(assign) => {
                assert!(matches!(*assign.left, ExprType::Field(_)));
                assert!(matches!(*assign.right, ExprType::Call(_)));
            }
            _ => panic!("expected assign"),
        }
    }

    #[test]
    fn test_parse_expr_complex_nested_if_match_call() {
        let expr: Expr = parse_quote!(if handler(
            match status {
                Ready => foo(),
                Pending => bar(),
                _ => baz(),
            }
        ) { success } else { failure });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::If(if_expr) => {
                // condition should be a call
                assert!(matches!(*if_expr.condition, ExprType::Call(_)));
                // If the call's arg is the match
                if let ExprType::Call(call) = &*if_expr.condition {
                    assert_eq!(call.args.len(), 1);
                    assert!(matches!(call.args[0], ExprType::Match(ref m) if m.arms.len() == 3));
                }
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn test_parse_expr_range_with_function_bounds() {
        let expr: Expr = parse_quote!(start() ..= end());
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Range(range) => {
                assert!(matches!(**range.start.as_ref().unwrap(), ExprType::Call(_)));
                assert!(matches!(**range.end.as_ref().unwrap(), ExprType::Call(_)));
                assert!(range.inclusive);
            }
            _ => panic!("expected range"),
        }
    }

    #[test]
    fn test_parse_expr_double_reference() {
        let expr: Expr = parse_quote!(&&x);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Reference(ref1) => {
                assert!(!ref1.is_mut);
                let inner_is_ref_and_immut = matches!(*ref1.expr, ExprType::Reference(ref r2) if !r2.is_mut);
                assert!(inner_is_ref_and_immut);
            }
            _ => panic!("expected reference"),
        }
    }

    #[test]
    fn test_parse_expr_reference_to_mutable_field() {
        let expr: Expr = parse_quote!(&mut obj.inner);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Reference(ref_expr) => {
                assert!(ref_expr.is_mut);
                assert!(matches!(*ref_expr.expr, ExprType::Field(_)));
            }
            _ => panic!("expected reference"),
        }
    }

    #[test]
    fn test_parse_expr_match_with_multiple_guards() {
        let expr: Expr = parse_quote!(match x {
            0 if ready => "go",
            1 if enable => "active",
            _ => "inactive",
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Match(match_expr) => {
                assert!(match_expr.arms[0].guard.is_some());
                assert!(match_expr.arms[1].guard.is_some());
                assert!(match_expr.arms[2].guard.is_none());
            }
            _ => panic!("expected match"),
        }
    }

    #[test]
    fn test_parse_expr_chained_method_calls_with_args() {
        let expr: Expr = parse_quote!(obj.first(a).second(b).third(c));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::MethodCall(method) => {
                assert_eq!(method.method, "third");
                assert_eq!(method.args.len(), 1);
                // receiver should be another method call
                match &*method.receiver {
                    ExprType::MethodCall(m) => assert_eq!(m.method, "second"),
                    _ => panic!("expected method call"),
                }
            }
            _ => panic!("expected method call"),
        }
    }

    #[test]
    fn test_parse_expr_await_on_method_result() {
        let expr: Expr = parse_quote!(obj.async_method().await);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Await(await_expr) => {
                assert!(matches!(*await_expr.base, ExprType::MethodCall(_)));
            }
            _ => panic!("expected await"),
        }
    }

    #[test]
    fn test_parse_expr_return_with_match_expression() {
        let expr: Expr = parse_quote!(return match x {
            1 => "one",
            _ => "other",
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Return(ret) => {
                assert!(ret.value.is_some());
                assert!(matches!(**ret.value.as_ref().unwrap(), ExprType::Match(_)));
            }
            _ => panic!("expected return"),
        }
    }

    #[test]
    fn test_parse_expr_yield_with_complex_expression() {
        let expr: Expr = parse_quote!(yield foo() + bar());
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Yield(yield_expr) => {
                assert!(yield_expr.value.is_some());
                assert!(matches!(**yield_expr.value.as_ref().unwrap(), ExprType::Binary(_)));
            }
            _ => panic!("expected yield"),
        }
    }

    #[test]
    fn test_parse_expr_cast_chain() {
        let expr: Expr = parse_quote!(x as u32 as u16);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Cast(outer_cast) => {
                assert_eq!(compact_ws(&outer_cast.target_ty.ty_text), "u16");
                // expr should be another cast
                match &*outer_cast.expr {
                    ExprType::Cast(inner) => {
                        assert_eq!(compact_ws(&inner.target_ty.ty_text), "u32");
                    }
                    _ => panic!("expected inner cast"),
                }
            }
            _ => panic!("expected cast"),
        }
    }

    #[test]
    fn test_parse_expr_if_with_local_statement_in_block() {
        let expr: Expr = parse_quote!(if condition {
            let result = compute();
            result
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::If(if_expr) => {
                assert_eq!(if_expr.then_block.len(), 2);
                if let RawStmtKind::Local(local) = &if_expr.then_block[0].kind {
                    assert_eq!(local.name, "result");
                } else {
                    panic!("expected local statement");
                }
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn test_parse_expr_match_arm_pattern_variations() {
        let expr: Expr = parse_quote!(match val {
            0 => "zero",
            1 | 2 => "one or two",
            3..=5 => "few",
            _ => "many",
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Match(match_expr) => {
                assert_eq!(match_expr.arms.len(), 4);
                // Just verify patterns aren't empty
                for arm in &match_expr.arms {
                    assert!(!arm.pattern_text.is_empty());
                }
            }
            _ => panic!("expected match"),
        }
    }

    // ========== Hardware Module Detection Tests ==========

    fn hw(names: &[&str]) -> std::collections::HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_expr_call_is_hardware_module_when_in_set() {
        let expr: Expr = parse_quote!(full_adder(a, b));
        match parse_expr_type(&expr, &hw(&["full_adder"])) {
            ExprType::Call(call) => {
                assert!(call.is_hardware_module);
            }
            _ => panic!("expected call"),
        }
    }

    #[test]
    fn test_expr_call_is_not_hardware_module_when_not_in_set() {
        let expr: Expr = parse_quote!(full_adder(a, b));
        match parse_expr_type(&expr, &hw(&["some_other_fn"])) {
            ExprType::Call(call) => {
                assert!(!call.is_hardware_module);
            }
            _ => panic!("expected call"),
        }
    }

    #[test]
    fn test_expr_call_is_not_hardware_module_with_empty_set() {
        let expr: Expr = parse_quote!(foo(x));
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Call(call) => {
                assert!(!call.is_hardware_module);
            }
            _ => panic!("expected call"),
        }
    }

    #[test]
    fn test_expr_call_hardware_module_with_multiple_fns_in_set() {
        let fns = hw(&["alu", "full_adder", "mux"]);
        for name in &["alu", "full_adder", "mux"] {
            let expr: Expr = syn::parse_str(&format!("{}(x)", name)).unwrap();
            match parse_expr_type(&expr, &fns) {
                ExprType::Call(call) => {
                    assert!(call.is_hardware_module, "{} should be hardware module", name);
                }
                _ => panic!("expected call"),
            }
        }
    }

    #[test]
    fn test_expr_call_method_call_is_never_hardware_module() {
        // MethodCall is a separate variant; function calls via method syntax
        // are never matched as hardware module instantiations
        let expr: Expr = parse_quote!(obj.full_adder(a, b));
        match parse_expr_type(&expr, &hw(&["full_adder"])) {
            ExprType::MethodCall(_) => {
                // Correct: method calls produce MethodCall, not Call
            }
            ExprType::Call(call) => {
                // If it parsed as Call for some reason, is_hardware_module should be false
                // since it goes through the method call path, not the path check
                assert!(!call.is_hardware_module);
            }
            _ => {}
        }
    }

    // ========== Pattern Text Bug Fix Tests ==========

    #[test]
    fn test_expr_let_pattern_text_simple_ident() {
        // Bug fix: was quote!(&e.pat) which produced literal "& e . pat"
        let expr: Expr = parse_quote!(let x = foo());
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Let(let_expr) => {
                assert_eq!(let_expr.pattern_text, "x");
            }
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn test_expr_let_pattern_text_tuple_destructure() {
        let expr: Expr = parse_quote!(let (a, b) = pair());
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Let(let_expr) => {
                // Should look like "(a , b)" or "(a, b)" — not "& e . pat"
                let compact: String = let_expr.pattern_text.chars().filter(|c| !c.is_whitespace()).collect();
                assert_eq!(compact, "(a,b)");
            }
            _ => panic!("expected let"),
        }
    }

    #[test]
    fn test_expr_match_arm_pattern_text_literal() {
        // Bug fix: was quote!(&arm.pat) which produced literal "& arm . pat"
        let expr: Expr = parse_quote!(match x { 0 => "zero", _ => "other" });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Match(match_expr) => {
                assert_eq!(match_expr.arms[0].pattern_text, "0");
                assert_eq!(match_expr.arms[1].pattern_text, "_");
            }
            _ => panic!("expected match"),
        }
    }

    #[test]
    fn test_expr_match_arm_pattern_text_tuple_pattern() {
        let expr: Expr = parse_quote!(match pair {
            (0, 0) => "origin",
            (x, y) => "other",
        });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Match(match_expr) => {
                let compact0: String = match_expr.arms[0].pattern_text.chars().filter(|c| !c.is_whitespace()).collect();
                let compact1: String = match_expr.arms[1].pattern_text.chars().filter(|c| !c.is_whitespace()).collect();
                assert_eq!(compact0, "(0,0)");
                assert_eq!(compact1, "(x,y)");
            }
            _ => panic!("expected match"),
        }
    }

    #[test]
    fn test_expr_match_arm_pattern_text_wildcard() {
        let expr: Expr = parse_quote!(match x { _ => 0 });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Match(match_expr) => {
                assert_eq!(match_expr.arms[0].pattern_text, "_");
            }
            _ => panic!("expected match"),
        }
    }

    #[test]
    fn test_expr_match_arm_pattern_text_or_pattern() {
        let expr: Expr = parse_quote!(match x { 1 | 2 | 3 => "low", _ => "high" });
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Match(match_expr) => {
                // Should contain the | separators, not "& arm . pat"
                assert!(match_expr.arms[0].pattern_text.contains('|'));
            }
            _ => panic!("expected match"),
        }
    }

    #[test]
    fn test_pattern_text_not_containing_legacy_bug_string() {
        // Regression: quote!(&e.pat) produced "& e . pat" literally
        let let_expr: Expr = parse_quote!(let x = 0);
        let match_expr: Expr = parse_quote!(match x { _ => 0 });

        match parse_expr_type(&let_expr, &Default::default()) {
            ExprType::Let(e) => assert!(!e.pattern_text.contains("e . pat")),
            _ => panic!(),
        }
        match parse_expr_type(&match_expr, &Default::default()) {
            ExprType::Match(m) => assert!(!m.arms[0].pattern_text.contains("arm . pat")),
            _ => panic!(),
        }
    }

    // ========== Recognized Hardware Pattern Tests ==========

    #[test]
    fn test_macro_statement_captured_as_expr_macro() {
        // A macro call in statement position is captured via the Stmt::Macro path
        // as a structured ExprType::Macro { name, tokens_text }, not opaque text.
        let design_fn: ItemFn = parse_quote! {
            async fn counter(clk: Clock<MainClk>, input: Arc<Mutex<u8>>) {
                emit!(42u8);
            }
        };
        let stmts = capture_raw_statements(&design_fn, &Default::default());
        assert_eq!(stmts.len(), 1);
        match &stmts[0].kind {
            RawStmtKind::Expr(expr_stmt) => {
                assert!(expr_stmt.has_semi);
                match &expr_stmt.expr {
                    ExprType::Macro(m) => {
                        assert_eq!(m.name, "emit");
                        assert!(m.tokens_text.contains("42"), "tokens: {}", m.tokens_text);
                    }
                    _ => panic!("expected Macro for emit! statement, got {:?}", expr_stmt.expr),
                }
            }
            _ => panic!("expected Expr stmt for emit!"),
        }
    }

    #[test]
    fn test_clk_tick_await_fir_shape() {
        // clk.tick().await must produce:
        // ExprType::Await { base: ExprType::MethodCall { receiver: Lit("clk"), method: "tick", args: [] } }
        let expr: Expr = parse_quote!(clk.tick().await);
        match parse_expr_type(&expr, &Default::default()) {
            ExprType::Await(await_expr) => {
                match &*await_expr.base {
                    ExprType::MethodCall(method) => {
                        assert_eq!(method.method, "tick");
                        assert_eq!(method.args.len(), 0);
                        // receiver should be the clk variable (Path)
                        assert!(matches!(*method.receiver, ExprType::Path(_)));
                    }
                    _ => panic!("expected MethodCall as await base, got {:?}", await_expr.base),
                }
            }
            _ => panic!("expected Await expression"),
        }
    }

    #[test]
    fn test_multiple_tick_boundaries_in_loop_body() {
        // A loop with two tick boundaries must capture both in order
        let design_fn: ItemFn = parse_quote! {
            async fn two_phase(clk: Clock<MainClk>, input: Arc<Mutex<u8>>) {
                loop {
                    let stage1 = *input.lock().unwrap();
                    clk.tick().await;
                    let stage2 = stage1;
                    clk.tick().await;
                }
            }
        };
        let stmts = capture_raw_statements(&design_fn, &Default::default());
        // The loop is the single top-level statement
        assert_eq!(stmts.len(), 1);
        match &stmts[0].kind {
            RawStmtKind::Expr(expr_stmt) => {
                match &expr_stmt.expr {
                    ExprType::Loop(loop_expr) => {
                        assert_eq!(loop_expr.body.len(), 4, "loop body should have 4 statements");
                        // Statements 1 and 3 (0-indexed) should be await expressions
                        let tick1 = &loop_expr.body[1];
                        let tick2 = &loop_expr.body[3];
                        for tick in &[tick1, tick2] {
                            match &tick.kind {
                                RawStmtKind::Expr(es) => {
                                    assert!(matches!(es.expr, ExprType::Await(_)),
                                        "expected Await at tick boundary");
                                }
                                _ => panic!("expected expr stmt at tick boundary"),
                            }
                        }
                    }
                    _ => panic!("expected loop"),
                }
            }
            _ => panic!("expected expr stmt"),
        }
    }

    #[test]
    fn test_arc_mutex_type_text_preserved() {
        // Arc<Mutex<T>> in parameter type must be preserved verbatim in ty_text
        // Phase B strips it — FIR just captures the raw text
        let design_fn: ItemFn = parse_quote! {
            async fn counter(clk: Clock<MainClk>, data: Arc<Mutex<u8>>) { }
        };
        let sig = capture_signature(&design_fn.sig);
        let data_param = &sig.params[1];
        assert_eq!(data_param.name, "data");
        let compact: String = data_param.ty.ty_text.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(compact, "Arc<Mutex<u8>>");
    }

    #[test]
    fn test_hardware_call_is_not_annotated_in_method_calls() {
        // hardware_fns should only annotate Expr::Call (function call syntax),
        // not Expr::MethodCall. Receiver-style calls are never hardware modules.
        let design_fn: ItemFn = parse_quote! {
            async fn counter(clk: Clock<MainClk>, data: Arc<Mutex<u8>>) {
                data.lock().unwrap();
            }
        };
        let stmts = capture_raw_statements(&design_fn, &hw(&["lock", "unwrap"]));
        match &stmts[0].kind {
            RawStmtKind::Expr(expr_stmt) => {
                // Should be a MethodCall chain, not ExprType::Call with is_hardware_module=true
                assert!(matches!(expr_stmt.expr, ExprType::MethodCall(_)));
            }
            _ => panic!("expected expr stmt"),
        }
    }
}
