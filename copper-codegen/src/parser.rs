use copper_core::frontend_ir::{
    ClockParamMeta, EnumVariant, ExprArray, ExprAssign, ExprAsync, ExprAwait, ExprBinary, ExprCall, ExprCast,
    ExprField, ExprIf, ExprLet, ExprLit, ExprLoop, ExprMatch, ExprMatchArm, ExprMethodCall,
    ExprRange, ExprReference, ExprRepeat, ExprReturn, ExprStmt, ExprType, ExprUnary, ExprWhile,
    ExprYield, FrontendClassification, FrontendModuleIR, FrontendSignature, ItemConst, ItemEnum,
    ItemMacro, ItemOther, ItemStmt, ItemStruct, ItemType, LocalStmt, RawParam, RawStmt, RawStmtKind,
    RawTypeRef, SourceSpan, StructField,
};
use copper_core::{ModuleIR, Port};
use copper_core::ir::Statement;
use quote::{ToTokens, quote};
use syn::spanned::Spanned;
use syn::{BinOp, Expr, ItemFn, Stmt, UnOp};

#[derive(Debug, Clone)]
pub enum LowerError {
    UnsupportedExpr(String),
    UnsupportedStmt(String),
    SignalNotFound(String),
    MissingArgument(String),
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            LowerError::UnsupportedExpr(expr) => write!(f, "Unsupported expression: {}", expr),
            LowerError::UnsupportedStmt(stmt) => write!(f, "Unsupported statement: {}", stmt),
            LowerError::SignalNotFound(name) => write!(f, "Signal not found: {}", name),
            LowerError::MissingArgument(arg) => write!(f, "Missing argument: {}", arg),
        }
    }
}

pub struct IRBuilder;

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

pub fn capture_frontend_ir(design_fn: &ItemFn) -> Result<FrontendModuleIR, LowerError> {
    let signature = capture_signature(design_fn);
    let clocks = capture_clock_metadata(design_fn);
    let classification = classify_module(design_fn);
    let raw_statements = capture_raw_statements(design_fn);

    Ok(FrontendModuleIR {
        module_name: design_fn.sig.ident.to_string(),
        signature,
        classification,
        clocks,
        raw_statements,
        span: capture_source_span(design_fn),
    })
}

fn capture_signature(design_fn: &ItemFn) -> FrontendSignature {
    let mut params = Vec::new();

    for input in &design_fn.sig.inputs {
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

    let return_ty = match &design_fn.sig.output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(_, ty) => Some(RawTypeRef {
            ty_text: ty.to_token_stream().to_string(),
            span: capture_source_span(&**ty),
        }),
    };

    FrontendSignature { params, return_ty }
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

fn capture_raw_statements(design_fn: &ItemFn) -> Vec<RawStmt> {
    design_fn
        .block
        .stmts
        .iter()
        .enumerate()
        .map(|(order, stmt)| RawStmt {
            order,
            kind: classify_raw_stmt_kind(stmt),
            text: quote!(#stmt).to_string(),
            span: capture_source_span(stmt),
        })
        .collect()
}

fn classify_raw_stmt_kind(stmt: &Stmt) -> RawStmtKind {
    match stmt {
        Stmt::Local(local) => parse_local_stmt(local),
        Stmt::Item(item) => parse_item_stmt(item),
        Stmt::Expr(expr, semi) => parse_expr_stmt(expr, semi.is_some()),
        Stmt::Macro(stmt_macro) => RawStmtKind::Expr(ExprStmt {
            expr: ExprType::Lit(ExprLit {
                text: quote!(#stmt_macro).to_string(),
                span: capture_source_span(stmt_macro),
            }),
            has_semi: true,
            span: capture_source_span(stmt_macro),
        }),
    }
}

fn parse_local_stmt(local: &syn::Local) -> RawStmtKind {
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
        init: local.init.as_ref().map(|init| parse_expr_type(&init.expr)),
        attrs: local.attrs.iter().map(|a| quote!(#a).to_string()).collect(),
        span: capture_source_span(local),
    })
}

fn parse_item_stmt(item: &syn::Item) -> RawStmtKind {
    let item_stmt = match item {
        syn::Item::Const(const_item) => {
            let name = const_item.ident.to_string();
            let ty = RawTypeRef {
                ty_text: const_item.ty.to_token_stream().to_string(),
                span: capture_source_span(&*const_item.ty),
            };
            let value_text = const_item.expr.to_token_stream().to_string();
            let attrs = const_item.attrs.iter().map(|a| quote!(#a).to_string()).collect();
            ItemStmt::Const(ItemConst {
                name,
                ty,
                value_text,
                attrs,
                span: capture_source_span(item),
            })
        }
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
        syn::Item::Struct(struct_item) => {
            let name = struct_item.ident.to_string();
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
            let attrs = struct_item.attrs.iter().map(|a| quote!(#a).to_string()).collect();
            ItemStmt::Struct(ItemStruct {
                name,
                fields,
                attrs,
                span: capture_source_span(item),
            })
        }
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

fn parse_expr_stmt(expr: &Expr, has_semi: bool) -> RawStmtKind {
    RawStmtKind::Expr(ExprStmt {
        expr: parse_expr_type(expr),
        has_semi,
        span: capture_source_span(expr),
    })
}

fn parse_expr_type(expr: &Expr) -> ExprType {
    match expr {
        Expr::Array(e) => ExprType::Array(ExprArray {
            elements: e.elems.iter().map(parse_expr_type).collect(),
            span: capture_source_span(e),
        }),

        Expr::Assign(e) => ExprType::Assign(ExprAssign {
            left: Box::new(parse_expr_type(&e.left)),
            right: Box::new(parse_expr_type(&e.right)),
            span: capture_source_span(e),
        }),

        Expr::Async(e) => ExprType::Async(ExprAsync {
            is_move: e.capture.is_some(),
            block: e.block.stmts.iter().enumerate().map(|(order, stmt)| RawStmt {
                order,
                kind: classify_raw_stmt_kind(stmt),
                text: quote!(#stmt).to_string(),
                span: capture_source_span(stmt),
            }).collect(),
            span: capture_source_span(e),
        }),

        Expr::Await(e) => ExprType::Await(ExprAwait {
            base: Box::new(parse_expr_type(&e.base)),
            span: capture_source_span(e),
        }),

        Expr::Binary(e) => ExprType::Binary(ExprBinary {
            left: Box::new(parse_expr_type(&e.left)),
            op: format_binop(&e.op),
            right: Box::new(parse_expr_type(&e.right)),
            span: capture_source_span(e),
        }),

        Expr::Call(e) => ExprType::Call(ExprCall {
            func: Box::new(parse_expr_type(&e.func)),
            args: e.args.iter().map(parse_expr_type).collect(),
            span: capture_source_span(e),
        }),

        Expr::Cast(e) => ExprType::Cast(ExprCast {
            expr: Box::new(parse_expr_type(&e.expr)),
            target_ty: RawTypeRef {
                ty_text: e.ty.to_token_stream().to_string(),
                span: capture_source_span(&*e.ty),
            },
            span: capture_source_span(e),
        }),

        Expr::Field(e) => ExprType::Field(ExprField {
            base: Box::new(parse_expr_type(&e.base)),
            member: match &e.member {
                syn::Member::Named(ident) => ident.to_string(),
                syn::Member::Unnamed(index) => index.index.to_string(),
            },
            span: capture_source_span(e),
        }),

        Expr::If(e) => ExprType::If(ExprIf {
            condition: Box::new(parse_expr_type(&e.cond)),
            then_block: e.then_branch.stmts.iter().enumerate().map(|(order, stmt)| RawStmt {
                order,
                kind: classify_raw_stmt_kind(stmt),
                text: quote!(#stmt).to_string(),
                span: capture_source_span(stmt),
            }).collect(),
            else_branch: e.else_branch.as_ref().map(|(_, expr)| {
                Box::new(parse_expr_type(expr))
            }),
            span: capture_source_span(e),
        }),

        Expr::Let(e) => ExprType::Let(ExprLet {
            pattern_text: quote!(&e.pat).to_string(),
            expr: Box::new(parse_expr_type(&e.expr)),
            span: capture_source_span(e),
        }),

        Expr::Lit(e) => ExprType::Lit(ExprLit {
            text: e.to_token_stream().to_string(),
            span: capture_source_span(e),
        }),

        Expr::Loop(e) => ExprType::Loop(ExprLoop {
            body: e.body.stmts.iter().enumerate().map(|(order, stmt)| RawStmt {
                order,
                kind: classify_raw_stmt_kind(stmt),
                text: quote!(#stmt).to_string(),
                span: capture_source_span(stmt),
            }).collect(),
            span: capture_source_span(e),
        }),

        Expr::Match(e) => ExprType::Match(ExprMatch {
            scrutinee: Box::new(parse_expr_type(&e.expr)),
            arms: e.arms.iter().map(|arm| {
                ExprMatchArm {
                    pattern_text: quote!(&arm.pat).to_string(),
                    guard: arm.guard.as_ref().map(|(_, g)| Box::new(parse_expr_type(g))),
                    body: Box::new(parse_expr_type(&arm.body)),
                    span: capture_source_span(arm),
                }
            }).collect(),
            span: capture_source_span(e),
        }),

        Expr::MethodCall(e) => ExprType::MethodCall(ExprMethodCall {
            receiver: Box::new(parse_expr_type(&e.receiver)),
            method: e.method.to_string(),
            args: e.args.iter().map(parse_expr_type).collect(),
            span: capture_source_span(e),
        }),

        Expr::Range(e) => ExprType::Range(ExprRange {
            start: e.start.as_ref().map(|e| Box::new(parse_expr_type(e))),
            end: e.end.as_ref().map(|e| Box::new(parse_expr_type(e))),
            inclusive: matches!(e.limits, syn::RangeLimits::Closed(_)),
            span: capture_source_span(e),
        }),

        Expr::Reference(e) => ExprType::Reference(ExprReference {
            is_mut: e.mutability.is_some(),
            expr: Box::new(parse_expr_type(&e.expr)),
            span: capture_source_span(e),
        }),

        Expr::Repeat(e) => ExprType::Repeat(ExprRepeat {
            expr: Box::new(parse_expr_type(&e.expr)),
            len: Box::new(parse_expr_type(&e.len)),
            span: capture_source_span(e),
        }),

        Expr::Return(e) => ExprType::Return(ExprReturn {
            value: e.expr.as_ref().map(|e| Box::new(parse_expr_type(e))),
            span: capture_source_span(e),
        }),

        Expr::Unary(e) => ExprType::Unary(ExprUnary {
            op: format_unop(&e.op),
            expr: Box::new(parse_expr_type(&e.expr)),
            span: capture_source_span(e),
        }),

        Expr::While(e) => ExprType::While(ExprWhile {
            condition: Box::new(parse_expr_type(&e.cond)),
            body: e.body.stmts.iter().enumerate().map(|(order, stmt)| RawStmt {
                order,
                kind: classify_raw_stmt_kind(stmt),
                text: quote!(#stmt).to_string(),
                span: capture_source_span(stmt),
            }).collect(),
            span: capture_source_span(e),
        }),

        Expr::Yield(e) => ExprType::Yield(ExprYield {
            value: e.expr.as_ref().map(|e| Box::new(parse_expr_type(e))),
            span: capture_source_span(e),
        }),

        // Fallback for unsupported/unhandled expressions
        _ => ExprType::Lit(ExprLit {
            text: quote!(#expr).to_string(),
            span: capture_source_span(expr),
        }),
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
        // let x = Bits::<8>::from_u128(0)
        syn::Expr::Call(call) => match &*call.func {
            syn::Expr::Path(p) => Some(RawTypeRef {
                ty_text: p.path.to_token_stream().to_string(),
                span: capture_source_span(p),
            }),
            _ => None,
        },

        // let x = foo as T
        syn::Expr::Cast(cast) => Some(RawTypeRef {
            ty_text: cast.ty.to_token_stream().to_string(),
            span: capture_source_span(&*cast.ty),
        }),

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
    let _ = node;
    // Stable fallback until span location conversion is wired up.
    SourceSpan::default()
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

        let signature = capture_signature(&design_fn);
        assert_eq!(signature.params.len(), 2);
        assert_eq!(signature.params[0].name, "clk");
        assert_eq!(compact_ws(&signature.params[0].ty.ty_text), "Clock<MainClk>");
        assert_eq!(signature.params[1].name, "input");
        assert_eq!(compact_ws(&signature.params[1].ty.ty_text), "Arc<Mutex<u8>>");
        assert_eq!(
            compact_ws(&signature.return_ty.expect("return type").ty_text),
            "u8"
        );
    }

    #[test]
    fn test_capture_signature_no_return() {
        let design_fn: ItemFn = parse_quote! {
            async fn my_module(input: Arc<Mutex<u8>>) {
                // body
            }
        };

        let signature = capture_signature(&design_fn);
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

        let signature = capture_signature(&design_fn);
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

        let raw_statements = capture_raw_statements(&design_fn);
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

        let kind = parse_local_stmt(&local);
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
    fn test_parse_local_stmt_infers_type_hint_from_init_when_no_explicit_type() {
        let local = parse_local_from_stmt(parse_quote! {
            let mut count = Bits::<8>::from_u128(0);
        });

        let kind = parse_local_stmt(&local);
        match kind {
            RawStmtKind::Local(local_stmt) => {
                assert!(local_stmt.is_mut);
                assert_eq!(local_stmt.name, "count");
                let ty = local_stmt.ty.expect("inferred type hint should be captured");
                assert_eq!(compact_ws(&ty.ty_text), "Bits::<8>::from_u128");
            }
            _ => panic!("expected local stmt"),
        }
    }

    #[test]
    fn test_parse_local_stmt_prefers_explicit_type_over_inferred_hint() {
        let local = parse_local_from_stmt(parse_quote! {
            let count: u16 = Bits::<8>::from_u128(0);
        });

        let kind = parse_local_stmt(&local);
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

        let kind = parse_local_stmt(&local);
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

        let kind = parse_local_stmt(&local);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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

        let kind = classify_raw_stmt_kind(&stmt);
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
            ExprType::Assign(assign) => {
                assert!(matches!(*assign.left, ExprType::Lit(_)));
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
            ExprType::Async(async_expr) => {
                assert!(async_expr.is_move);
            }
            _ => panic!("expected async"),
        }
    }

    #[test]
    fn test_parse_expr_await() {
        let expr: Expr = parse_quote!(foo.await);
        match parse_expr_type(&expr) {
            ExprType::Await(await_expr) => {
                assert!(matches!(*await_expr.base, ExprType::Lit(_)));
            }
            _ => panic!("expected await"),
        }
    }

    #[test]
    fn test_parse_expr_binary_add() {
        let expr: Expr = parse_quote!(a + b);
        match parse_expr_type(&expr) {
            ExprType::Binary(bin) => {
                assert_eq!(bin.op, "+");
                assert!(matches!(*bin.left, ExprType::Lit(_)));
                assert!(matches!(*bin.right, ExprType::Lit(_)));
            }
            _ => panic!("expected binary"),
        }
    }

    #[test]
    fn test_parse_expr_binary_bitwise_or() {
        let expr: Expr = parse_quote!(a | b);
        match parse_expr_type(&expr) {
            ExprType::Binary(bin) => {
                assert_eq!(bin.op, "|");
            }
            _ => panic!("expected binary"),
        }
    }

    #[test]
    fn test_parse_expr_binary_logical_and() {
        let expr: Expr = parse_quote!(a && b);
        match parse_expr_type(&expr) {
            ExprType::Binary(bin) => {
                assert_eq!(bin.op, "&&");
            }
            _ => panic!("expected binary"),
        }
    }

    #[test]
    fn test_parse_expr_call() {
        let expr: Expr = parse_quote!(foo(1, 2));
        match parse_expr_type(&expr) {
            ExprType::Call(call) => {
                assert_eq!(call.args.len(), 2);
                assert!(matches!(&call.args[0], ExprType::Lit(_)));
                assert!(matches!(&call.args[1], ExprType::Lit(_)));
            }
            _ => panic!("expected call"),
        }
    }

    #[test]
    fn test_parse_expr_cast() {
        let expr: Expr = parse_quote!(x as u32);
        match parse_expr_type(&expr) {
            ExprType::Cast(cast) => {
                assert!(matches!(*cast.expr, ExprType::Lit(_)));
                assert_eq!(compact_ws(&cast.target_ty.ty_text), "u32");
            }
            _ => panic!("expected cast"),
        }
    }

    #[test]
    fn test_parse_expr_field() {
        let expr: Expr = parse_quote!(obj.field);
        match parse_expr_type(&expr) {
            ExprType::Field(field) => {
                assert_eq!(field.member, "field");
                assert!(matches!(*field.base, ExprType::Lit(_)));
            }
            _ => panic!("expected field"),
        }
    }

    #[test]
    fn test_parse_expr_if() {
        let expr: Expr = parse_quote!(if x > 0 { 1 } else { 2 });
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
            ExprType::Lit(lit) => {
                assert_eq!(compact_ws(&lit.text), "42");
            }
            _ => panic!("expected lit"),
        }
    }

    #[test]
    fn test_parse_expr_lit_string() {
        let expr: Expr = parse_quote!("hello");
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
            ExprType::Match(match_expr) => {
                assert_eq!(match_expr.arms.len(), 3);
                assert!(matches!(*match_expr.scrutinee, ExprType::Lit(_)));
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
        match parse_expr_type(&expr) {
            ExprType::Match(match_expr) => {
                assert!(match_expr.arms[0].guard.is_some());
            }
            _ => panic!("expected match"),
        }
    }

    #[test]
    fn test_parse_expr_method_call() {
        let expr: Expr = parse_quote!(obj.method(1, 2));
        match parse_expr_type(&expr) {
            ExprType::MethodCall(method) => {
                assert_eq!(method.method, "method");
                assert_eq!(method.args.len(), 2);
                assert!(matches!(*method.receiver, ExprType::Lit(_)));
            }
            _ => panic!("expected method call"),
        }
    }

    #[test]
    fn test_parse_expr_range_exclusive() {
        let expr: Expr = parse_quote!(1..10);
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
            ExprType::Range(range) => {
                assert!(range.inclusive);
            }
            _ => panic!("expected range"),
        }
    }

    #[test]
    fn test_parse_expr_range_open_start() {
        let expr: Expr = parse_quote!(..10);
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
            ExprType::Reference(ref_expr) => {
                assert!(!ref_expr.is_mut);
                assert!(matches!(*ref_expr.expr, ExprType::Lit(_)));
            }
            _ => panic!("expected reference"),
        }
    }

    #[test]
    fn test_parse_expr_reference_mut() {
        let expr: Expr = parse_quote!(&mut x);
        match parse_expr_type(&expr) {
            ExprType::Reference(ref_expr) => {
                assert!(ref_expr.is_mut);
                assert!(matches!(*ref_expr.expr, ExprType::Lit(_)));
            }
            _ => panic!("expected reference"),
        }
    }

    #[test]
    fn test_parse_expr_repeat() {
        let expr: Expr = parse_quote!([0; 8]);
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
            ExprType::Return(ret) => {
                assert!(ret.value.is_some());
            }
            _ => panic!("expected return"),
        }
    }

    #[test]
    fn test_parse_expr_return_without_value() {
        let expr: Expr = parse_quote!(return);
        match parse_expr_type(&expr) {
            ExprType::Return(ret) => {
                assert!(ret.value.is_none());
            }
            _ => panic!("expected return"),
        }
    }

    #[test]
    fn test_parse_expr_unary_neg() {
        let expr: Expr = parse_quote!(-x);
        match parse_expr_type(&expr) {
            ExprType::Unary(unary) => {
                assert_eq!(unary.op, "-");
                assert!(matches!(*unary.expr, ExprType::Lit(_)));
            }
            _ => panic!("expected unary"),
        }
    }

    #[test]
    fn test_parse_expr_unary_not() {
        let expr: Expr = parse_quote!(!flag);
        match parse_expr_type(&expr) {
            ExprType::Unary(unary) => {
                assert_eq!(unary.op, "!");
            }
            _ => panic!("expected unary"),
        }
    }

    #[test]
    fn test_parse_expr_unary_deref() {
        let expr: Expr = parse_quote!(*ptr);
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
            ExprType::Yield(yield_expr) => {
                assert!(yield_expr.value.is_some());
            }
            _ => panic!("expected yield"),
        }
    }

    #[test]
    fn test_parse_expr_yield_without_value() {
        let expr: Expr = parse_quote!(yield);
        match parse_expr_type(&expr) {
            ExprType::Yield(yield_expr) => {
                assert!(yield_expr.value.is_none());
            }
            _ => panic!("expected yield"),
        }
    }

    #[test]
    fn test_parse_expr_nested_binary_and_call() {
        let expr: Expr = parse_quote!(foo(a + b));
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
            ExprType::If(if_expr) => {
                assert!(if_expr.else_branch.is_some());
                // The else branch is present and should be a literal
                assert!(matches!(**if_expr.else_branch.as_ref().unwrap(), ExprType::Lit(_)));
            }
            _ => panic!("expected if"),
        }
    }

    #[test]
    fn test_parse_expr_if_else_if_chain() {
        let expr: Expr = parse_quote!(if x > 0 { 1 } else if x < 0 { 2 } else { 3 });
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
            ExprType::Binary(bin) => {
                assert_eq!(bin.op, "<<");
            }
            _ => panic!("expected binary"),
        }
    }

    #[test]
    fn test_parse_expr_cast_validates_target_type() {
        let expr: Expr = parse_quote!(foo() as Vec<String>);
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
            ExprType::MethodCall(method) => {
                assert_eq!(method.method, "process");
                assert_eq!(method.args.len(), 2);
                assert!(matches!(method.args[0], ExprType::Lit(_)));
                assert!(matches!(method.args[1], ExprType::Cast(_)));
            }
            _ => panic!("expected method call"),
        }
    }

    #[test]
    fn test_parse_expr_unary_validates_operand() {
        let expr: Expr = parse_quote!(!foo(x));
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
        match parse_expr_type(&expr) {
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
}
