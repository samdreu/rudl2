use copper_core::chir::{
    CHIRBinOp, CHIRBody, CHIRCaseArm, CHIRCombBody, CHIRExpr, CHIRLit, CHIRLowerError,
    CHIRMatchArm, CHIRMemInit, CHIRMemoryDecl, CHIRModule, CHIRPattern, CHIRPort, CHIRPortDir, CHIRPortKind, CHIRRegDecl,
    CHIRSeqBody, CHIRStmt, CHIRStructuralBody, CHIRSubmoduleInst, CHIRType, CHIRUnOp, Width,
};
use copper_core::memory::WriteMode;
use copper_core::frontend_ir::{
    ExprCall, ExprIndex, ExprLoop, ExprRepeat, ExprStruct, ExprType, FrontendClassification,
    FrontendFnIR, FrontendModuleIR, ItemStruct, LocalStmt, RawStmt, RawStmtKind, SourceSpan,
};

// ── Public type aliases ───────────────────────────────────────────────────────

/// Registry mapping module name → its FIR, used by Phase B to resolve
/// port names and output types of `#[hardware]` callees.
pub type ModuleRegistry = std::collections::HashMap<String, FrontendModuleIR>;

// ── Public entry point ────────────────────────────────────────────────────────

pub fn lower_to_chir(
    fir: &FrontendModuleIR,
    hardware_fns: &std::collections::HashSet<String>,
    registry: &ModuleRegistry,
) -> Result<CHIRModule, CHIRLowerError> {
    let ports = lower_ports(fir)?;

    let body = match fir.classification {
        FrontendClassification::CombinationalFn => {
            CHIRBody::Combinational(lower_comb_body(fir, hardware_fns, registry)?)
        }
        FrontendClassification::AsyncSequentialFn => {
            CHIRBody::Sequential(lower_seq_body(fir, hardware_fns, registry)?)
        }
        FrontendClassification::StructuralFn => {
            CHIRBody::Structural(lower_structural_body(fir, registry)?)
        }
    };

    let module = CHIRModule {
        name: fir.module_name.clone(),
        params: build_module_params(fir),
        // Every file-scope const the module *could* reference. Which ones are
        // actually emitted is decided at emission, from the rendered
        // SystemVerilog — an unused `localparam` is a Verilator `UNUSEDPARAM`
        // error under `-Wall`, so the set has to be exact, and the emitted text
        // is the only exact source for it.
        localparams: crate::file_consts::candidates(fir),
        ports,
        body,
        span: fir.span,
    };

    validate_module(&module, fir)?;

    Ok(module)
}

/// Every name that resolves to a SystemVerilog `parameter`/`localparam` rather
/// than a signal: the module's const generics plus the file-scope constants it
/// can reference. Both emit as `int`, so both are 32 bits wide.
fn param_names(fir: &FrontendModuleIR) -> std::collections::HashSet<String> {
    build_module_params(fir)
        .into_iter()
        .map(|p| p.name)
        .chain(crate::file_consts::candidates(fir).into_iter().map(|lp| lp.name))
        .collect()
}

/// Module-level parameters from the FIR's const generics (`<const N: usize>`).
/// Type/lifetime/domain generics are not module parameters. `default` is the
/// source-declared default (`<const N: usize = 8>`) when present — the transpiler
/// is invoked on a definition, so a caller otherwise supplies `N` at instantiation.
fn build_module_params(fir: &FrontendModuleIR) -> Vec<copper_core::chir::ModuleParam> {
    use copper_core::frontend_ir::GenericParamKind;
    fir.signature
        .generics
        .iter()
        .filter(|g| g.kind == GenericParamKind::Const)
        .map(|g| copper_core::chir::ModuleParam {
            name: g.name.clone(),
            default: g.default.as_ref().and_then(|d| d.trim().parse::<usize>().ok()),
        })
        .collect()
}

// ── Type resolution ───────────────────────────────────────────────────────────

/// Compact a **type** text: drop whitespace, then strip a leading module-path
/// qualifier so `copper_core::Clock<MainClk>` matches the same rules as a bare
/// `Clock<MainClk>`.
///
/// Every prefix test in this file (`starts_with("Clock<")`, `"In<"`, `"Out<"`,
/// …) is a *textual* match, so a fully-qualified path silently failed all of
/// them — `sipo_block` was the one example written that way and was reported
/// unresolvable. Type texts go through here; literal, path and pattern texts do
/// **not** (a qualifier is meaningful there — `Logic::One`, `Opcode::LUI`).
pub(crate) fn compact_type(ty_text: &str) -> String {
    let compact: String = ty_text.chars().filter(|c| !c.is_whitespace()).collect();
    strip_path_qualifier(&compact).to_string()
}

/// `copper_core::Clock<D>` → `Clock<D>`; `::a::b::In<T,D>` → `In<T,D>`;
/// `Bits<8>` unchanged.
///
/// Only the **head** path is stripped: the search stops at the first `<`, so a
/// qualifier inside a generic argument (`In<Bits<8>, some_mod::Dom>`) is left
/// alone for the domain logic to deal with.
fn strip_path_qualifier(compact: &str) -> &str {
    let head_end = compact.find('<').unwrap_or(compact.len());
    match compact[..head_end].rfind("::") {
        Some(i) => &compact[i + 2..],
        None => compact,
    }
}

/// Resolve a raw Copper type text to a `CHIRType`.
pub fn resolve_type(ty_text: &str, span: SourceSpan) -> Result<CHIRType, CHIRLowerError> {
    let compact = compact_type(ty_text);

    if let Some(inner) = strip_arc_mutex(&compact) {
        return resolve_type(inner, span);
    }

    match compact.as_str() {
        "u8"   => Ok(CHIRType::UInt { width: Width::Concrete(8) }),
        "u16"  => Ok(CHIRType::UInt { width: Width::Concrete(16) }),
        "u32"  => Ok(CHIRType::UInt { width: Width::Concrete(32) }),
        "u64"  => Ok(CHIRType::UInt { width: Width::Concrete(64) }),
        "u128" => Ok(CHIRType::UInt { width: Width::Concrete(128) }),
        "i8"   => Ok(CHIRType::SInt { width: Width::Concrete(8) }),
        "i16"  => Ok(CHIRType::SInt { width: Width::Concrete(16) }),
        "i32"  => Ok(CHIRType::SInt { width: Width::Concrete(32) }),
        "i64"  => Ok(CHIRType::SInt { width: Width::Concrete(64) }),
        "i128" => Ok(CHIRType::SInt { width: Width::Concrete(128) }),
        // `usize`/`isize` have no fixed hardware width; used almost entirely as
        // loop/index quantities, so treat them as 32-bit (matching the SV `int`
        // loop variable, keeping index arithmetic width-consistent).
        "usize" => Ok(CHIRType::UInt { width: Width::Concrete(32) }),
        "isize" => Ok(CHIRType::SInt { width: Width::Concrete(32) }),
        "bool" => Ok(CHIRType::Bool),
        "Bit"  => Ok(CHIRType::UInt { width: Width::Concrete(1) }),
        "Logic" => Ok(CHIRType::UInt { width: Width::Concrete(1) }),
        _ if compact.starts_with("Bits<") => parse_bits_type(&compact, span),
        _ if compact.starts_with('[') => parse_array_type(&compact, span),
        _ => Err(CHIRLowerError::UnresolvableType {
            ty_text: ty_text.to_string(),
            span,
        }),
    }
}

fn strip_arc_mutex(compact: &str) -> Option<&str> {
    let inner = compact.strip_prefix("Arc<Mutex<")?.strip_suffix(">>")?;
    Some(inner)
}

fn parse_bits_type(compact: &str, span: SourceSpan) -> Result<CHIRType, CHIRLowerError> {
    let inner = compact
        .strip_prefix("Bits<")
        .and_then(|s| s.strip_suffix('>'));
    match inner {
        // `Bits<8>` — a concrete width.
        Some(s) if s.parse::<usize>().is_ok() => {
            Ok(CHIRType::UInt { width: Width::Concrete(s.parse().unwrap()) })
        }
        // `Bits<N>` — a const-generic parameter: a symbolic width (M2).
        Some(s) if is_ident(s) => Ok(CHIRType::UInt { width: Width::Param(s.to_string()) }),
        _ => Err(CHIRLowerError::UnresolvableType {
            ty_text: compact.to_string(),
            span,
        }),
    }
}

/// Parse `[Bits<W>; ELS]` — a fixed-length array of a hardware type.
///
/// Both the element type and the length may be symbolic: `ELS` is a const
/// generic (`mux`) or a file-scope const lowered to a `localparam`
/// (`bsg_mux_one_hot`). Neither dimension needs width arithmetic, which is the
/// point of the packed-2-D ABI — see `design_docs/ARRAY_PORT_ABI.md`.
fn parse_array_type(compact: &str, span: SourceSpan) -> Result<CHIRType, CHIRLowerError> {
    let unresolvable = || CHIRLowerError::UnresolvableType {
        ty_text: compact.to_string(),
        span,
    };
    let inner = compact
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(unresolvable)?;
    // Split on the LAST `;` so a nested element type keeps its own punctuation.
    let (elem_text, len_text) = inner.rsplit_once(';').ok_or_else(unresolvable)?;
    let elem = resolve_type(elem_text, span)?;
    let len = if let Ok(n) = len_text.parse::<usize>() {
        Width::Concrete(n)
    } else if is_ident(len_text) {
        Width::Param(len_text.to_string())
    } else {
        return Err(unresolvable());
    };
    // An array OF arrays would need a third packed dimension and has no instance
    // in the corpus; refuse it rather than emit a shape that was never verified.
    if matches!(elem, CHIRType::Array { .. }) {
        return Err(unresolvable());
    }
    Ok(CHIRType::Array { elem: Box::new(elem), len })
}

// ── Type inference from expressions ──────────────────────────────────────────

/// A symbol table mapping in-scope names (ports, wires, registers) to their
/// resolved hardware types. Used to infer widths of expressions that reference
/// signals — e.g. `port.read()` or `wire & other`.
type SymbolTable = std::collections::HashMap<String, CHIRType>;

/// Infer `CHIRType` from an init expression when no explicit annotation exists.
///
/// Handles:
/// - Typed integer literals (`0u8`, `42u32`) and booleans
/// - Cast expressions (`x as u8`) → cast target type
/// - Signal references (`x`) and `x.read()` → the signal's declared type,
///   looked up in `symbols` (this is what lets `Logic`/`Bits<N>` ports flow)
/// - Unary ops (`!x`, `~x`, `-x`) → operand width
/// - Binary ops: comparisons/logical (`==`, `<`, `&&`, …) → 1-bit `Bool`;
///   arithmetic/bitwise/shift (`+`, `&`, `<<`, …) → operand width
/// - `if`/`match` used as expressions → a branch's type
///
/// Returns `AmbiguousWidth` when no width can be determined.
fn infer_type_from_expr(
    expr: &ExprType,
    span: SourceSpan,
    symbols: &SymbolTable,
    enums: &EnumRegistry,
) -> Result<CHIRType, CHIRLowerError> {
    match expr {
        ExprType::Lit(lit) => infer_type_from_text(&lit.text, span, symbols, enums),
        ExprType::Path(path) => infer_type_from_text(&path.path_text, span, symbols, enums),
        ExprType::Cast(cast) => resolve_type(&cast.target_ty.ty_text, cast.target_ty.span),
        ExprType::Reference(r) => infer_type_from_expr(&r.expr, span, symbols, enums),
        ExprType::Unary(un) => infer_type_from_expr(&un.expr, span, symbols, enums),
        ExprType::Binary(bin) => {
            if is_comparison_or_logical_op(&bin.op) {
                Ok(CHIRType::Bool)
            } else {
                // Width follows the operands; try left, then right.
                infer_type_from_expr(&bin.left, span, symbols, enums)
                    .or_else(|_| infer_type_from_expr(&bin.right, span, symbols, enums))
            }
        }
        ExprType::MethodCall(mc) => match mc.method.as_str() {
            // `.read()` on a port, and wrapping/lock/unwrap/clone wrappers, all
            // carry the width of their receiver.
            "read" | "lock" | "unwrap" | "clone" | "wrapping_add" | "wrapping_sub"
            | "wrapping_mul" | "as_u8" | "as_u16" | "as_u32" | "as_u64" | "as_u128"
            | "as_usize" | "as_bits" => infer_type_from_expr(&mc.receiver, span, symbols, enums),
            "as_bool" => Ok(CHIRType::Bool),
            _ => infer_type_from_expr(&mc.receiver, span, symbols, enums),
        },
        ExprType::Call(call) => match identity_pack_call(call) {
            Some(inner) => infer_type_from_expr(inner, span, symbols, enums),
            None => infer_type_from_call(call, span),
        },
        // A field read of a flattened struct local: `ex_mem.alu_result` is the
        // `ex_mem_alu_result` net.
        ExprType::Field(f) => {
            if let ExprType::Path(pth) = f.base.as_ref() {
                let name = format!("{}_{}", compact_ident(&pth.path_text), f.member);
                if let Some(t) = symbols.get(&name) {
                    return Ok(t.clone());
                }
            }
            Err(CHIRLowerError::AmbiguousWidth { span })
        }
        // A single-bit index `x[i]` is 1-bit.
        ExprType::Index(_) => Ok(CHIRType::UInt { width: Width::Concrete(1) }),
        // `[Logic::Zero; N]` — a repeated array of hardware elements is a packed
        // vector: N elements of width W is an (N*W)-bit value. This is the same
        // representation `Bits<N>` already has, and `a[k]` already lowers as a
        // 1-bit select, so indexed reads and writes into the local need nothing
        // extra.
        ExprType::Repeat(rep) => {
            let elem_w = infer_type_from_expr(&rep.expr, span, symbols, enums)
                .ok()
                .and_then(|t| chir_type_width(&t).as_concrete());
            match (elem_w, repeat_len(&rep.len)) {
                (Some(w), Some(Width::Concrete(n))) =>
                    Ok(CHIRType::UInt { width: Width::Concrete(w * n) }),
                // A symbolic length only yields a symbolic width when each
                // element is one bit: `Width` has no product form, so
                // `[Bits<8>; N]` stays ambiguous rather than silently mis-sized.
                (Some(1), Some(Width::Param(name))) =>
                    Ok(CHIRType::UInt { width: Width::Param(name) }),
                _ => Err(CHIRLowerError::AmbiguousWidth { span }),
            }
        }
        // A tuple is the concatenation of its elements.
        ExprType::Tuple(t) => {
            let mut total = 0usize;
            for e in &t.elements {
                total += width_of_type(&infer_type_from_expr(e, span, symbols, enums)?);
            }
            Ok(CHIRType::UInt { width: Width::Concrete(total) })
        }
        ExprType::If(if_expr) => match &if_expr.else_branch {
            Some(else_br) => infer_type_from_expr(else_br, span, symbols, enums),
            None => Err(CHIRLowerError::AmbiguousWidth { span }),
        },
        ExprType::Match(m) => m
            .arms
            .first()
            .map(|a| infer_type_from_expr(&a.body, span, symbols, enums))
            .unwrap_or(Err(CHIRLowerError::AmbiguousWidth { span })),
        // A block's type is its tail expression's type.
        ExprType::Block(b) => b
            .stmts
            .iter()
            .rev()
            .find_map(|s| match &s.kind {
                RawStmtKind::Expr(es) if !es.has_semi => Some(&es.expr),
                _ => None,
            })
            .map(|e| infer_type_from_expr(e, span, symbols, enums))
            .unwrap_or(Err(CHIRLowerError::AmbiguousWidth { span })),
        _ => Err(CHIRLowerError::AmbiguousWidth { span }),
    }
}

fn infer_type_from_text(
    text: &str,
    span: SourceSpan,
    symbols: &SymbolTable,
    enums: &EnumRegistry,
) -> Result<CHIRType, CHIRLowerError> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(ty) = infer_type_from_suffix(&compact) {
        return Ok(ty);
    }
    match compact.as_str() {
        "true" | "false" => return Ok(CHIRType::Bool),
        "Logic::One" | "Logic::Zero" => {
            return Ok(CHIRType::UInt { width: Width::Concrete(1) })
        }
        _ => {}
    }
    if is_ident(&compact) {
        if let Some(ty) = symbols.get(&compact) {
            return Ok(ty.clone());
        }
    }
    // An enum variant path (`State::IDLE`) carries its enum's width.
    if let Some((ty, _)) = resolve_enum_path(&compact, enums) {
        return Ok(ty);
    }
    Err(CHIRLowerError::AmbiguousWidth { span })
}

/// True for operators that always produce a 1-bit boolean result.
fn is_comparison_or_logical_op(op: &str) -> bool {
    matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||")
}

/// The whitespace-stripped callee path of a call, e.g. `"Bits::from_u32"`.
fn call_path(call: &ExprCall) -> Option<String> {
    match &*call.func {
        ExprType::Lit(lit) => Some(lit.text.chars().filter(|c| !c.is_whitespace()).collect()),
        ExprType::Path(path) => Some(path.path_text.chars().filter(|c| !c.is_whitespace()).collect()),
        _ => None,
    }
}

// ── Free-function inlining (#7b) ───────────────────────────────────────────────

/// Inline a call to a file-scope free function. Substitutes arguments for
/// parameters, folds the body's `let` bindings into the tail expression, then
/// lowers the result. Nested helper calls (e.g. `decode` → `sign_ext_i`) inline
/// recursively through `lower_expr`. Only pure combinational bodies are
/// supported: `let name = expr;` bindings ending in a tail expression, with no
/// early return, `?`, `.await`, or statement-level side effects.
fn lower_inlined_fn_call(call: &ExprCall, ctx: &mut LowerCtx) -> Result<CHIRExpr, CHIRLowerError> {
    let name = call_path(call).expect("caller checked call_path names a known fn");
    let fn_ir = ctx.fns.get(&name).cloned().expect("caller checked fns contains name");

    if ctx.inlining.contains(&name) {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: format!("cannot inline recursive function `{name}` into hardware"),
            span: call.span,
            suggested_rewrite: None,
        });
    }

    let inlined = build_inlined_expr(&fn_ir, &call.args, call.span)?;

    ctx.inlining.insert(name.clone());
    let result = lower_expr(&inlined, ctx);
    ctx.inlining.remove(&name);
    result
}

/// The caller-side expression a free-fn call is equivalent to: parameter→argument
/// substitution plus folding of the body's `let` bindings into the tail.
fn build_inlined_expr(
    fn_ir: &FrontendFnIR,
    args: &[ExprType],
    call_span: SourceSpan,
) -> Result<ExprType, CHIRLowerError> {
    let params = &fn_ir.signature.params;
    if params.len() != args.len() {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: format!(
                "call to `{}` has {} argument(s) but it declares {} parameter(s)",
                fn_ir.name,
                args.len(),
                params.len()
            ),
            span: call_span,
            suggested_rewrite: None,
        });
    }

    // Each parameter starts bound to its argument expression.
    let mut subst: std::collections::HashMap<String, ExprType> = params
        .iter()
        .zip(args.iter())
        .map(|(p, a)| (p.name.clone(), a.clone()))
        .collect();

    let body = &fn_ir.raw_statements;
    let tail_idx = tail_expr_index(body).ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
        description: format!("cannot inline `{}`: body has no tail expression to return", fn_ir.name),
        span: fn_ir.span,
        suggested_rewrite: None,
    })?;

    // Fold each `let` binding into the substitution, applying earlier bindings to
    // its initializer. Anything other than a `let` before the tail is a
    // statement-level effect we can't fold into a single expression — reject it.
    for (i, stmt) in body.iter().enumerate() {
        if i == tail_idx {
            break;
        }
        match &stmt.kind {
            RawStmtKind::Local(local) => {
                let init = local.init.as_ref().ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
                    description: format!(
                        "cannot inline `{}`: `let {}` has no initializer",
                        fn_ir.name, local.name
                    ),
                    span: local.span,
                    suggested_rewrite: None,
                })?;
                let value = substitute_expr(init, &subst);
                subst.insert(local.name.clone(), value);
            }
            // Nested items (e.g. a `const`) don't participate in value flow.
            RawStmtKind::Item(_) => {}
            RawStmtKind::Expr(_) => {
                return Err(CHIRLowerError::UnsupportedConstruct {
                    description: format!(
                        "cannot inline `{}`: only `let` bindings and a tail expression are \
                         supported (found a non-binding statement before the tail)",
                        fn_ir.name
                    ),
                    span: stmt.span,
                    suggested_rewrite: None,
                });
            }
        }
    }

    let tail = match &body[tail_idx].kind {
        RawStmtKind::Expr(es) => &es.expr,
        _ => unreachable!("tail_expr_index returns an expression statement"),
    };
    Ok(substitute_expr(tail, &subst))
}

/// Index of the tail expression: the last statement, when it is an expression
/// statement with no trailing semicolon.
fn tail_expr_index(body: &[RawStmt]) -> Option<usize> {
    let last = body.len().checked_sub(1)?;
    match &body[last].kind {
        RawStmtKind::Expr(es) if !es.has_semi => Some(last),
        _ => None,
    }
}

/// Return a copy of `expr` with every simple-identifier reference that appears in
/// `subst` replaced by its bound expression. Used to inline free-fn bodies.
/// Limitation: does not track shadowing introduced by nested `let`/match binders,
/// which pure combinational helpers do not rely on.
fn substitute_expr(expr: &ExprType, subst: &std::collections::HashMap<String, ExprType>) -> ExprType {
    let mut e = expr.clone();
    subst_in_place(&mut e, subst);
    e
}

fn subst_in_place(e: &mut ExprType, subst: &std::collections::HashMap<String, ExprType>) {
    match e {
        ExprType::Path(p) => {
            if let Some(rep) = simple_ident(&p.path_text).and_then(|id| subst.get(&id)) {
                *e = rep.clone();
            }
        }
        ExprType::Lit(_) => {}
        ExprType::Binary(b) => {
            subst_in_place(&mut b.left, subst);
            subst_in_place(&mut b.right, subst);
        }
        ExprType::Unary(u) => subst_in_place(&mut u.expr, subst),
        ExprType::Cast(c) => subst_in_place(&mut c.expr, subst),
        ExprType::Reference(r) => subst_in_place(&mut r.expr, subst),
        ExprType::Call(c) => {
            subst_in_place(&mut c.func, subst);
            for a in &mut c.args {
                subst_in_place(a, subst);
            }
        }
        ExprType::MethodCall(m) => {
            subst_in_place(&mut m.receiver, subst);
            for a in &mut m.args {
                subst_in_place(a, subst);
            }
        }
        ExprType::Index(i) => {
            subst_in_place(&mut i.base, subst);
            subst_in_place(&mut i.index, subst);
        }
        ExprType::Field(f) => subst_in_place(&mut f.base, subst),
        ExprType::Tuple(t) => t.elements.iter_mut().for_each(|el| subst_in_place(el, subst)),
        ExprType::Array(a) => a.elements.iter_mut().for_each(|el| subst_in_place(el, subst)),
        ExprType::If(f) => {
            subst_in_place(&mut f.condition, subst);
            subst_in_stmts(&mut f.then_block, subst);
            if let Some(eb) = &mut f.else_branch {
                subst_in_place(eb, subst);
            }
        }
        ExprType::Match(m) => {
            subst_in_place(&mut m.scrutinee, subst);
            for arm in &mut m.arms {
                if let Some(g) = &mut arm.guard {
                    subst_in_place(g, subst);
                }
                subst_in_place(&mut arm.body, subst);
            }
        }
        ExprType::Block(b) => subst_in_stmts(&mut b.stmts, subst),
        ExprType::Struct(s) => {
            for f in &mut s.fields {
                subst_in_place(&mut f.expr, subst);
            }
            if let Some(r) = &mut s.rest {
                subst_in_place(r, subst);
            }
        }
        ExprType::Range(r) => {
            if let Some(s) = &mut r.start {
                subst_in_place(s, subst);
            }
            if let Some(en) = &mut r.end {
                subst_in_place(en, subst);
            }
        }
        ExprType::Repeat(r) => {
            subst_in_place(&mut r.expr, subst);
            subst_in_place(&mut r.len, subst);
        }
        ExprType::Try(t) => subst_in_place(&mut t.expr, subst),
        ExprType::Return(r) => {
            if let Some(v) = &mut r.value {
                subst_in_place(v, subst);
            }
        }
        // Await/Async/Assign/Let/Loop/While/Macro/Break/Continue/Yield/Const do
        // not appear in the pure combinational tails we inline; a body using them
        // is rejected before substitution or fails to lower afterward.
        _ => {}
    }
}

fn subst_in_stmts(stmts: &mut [RawStmt], subst: &std::collections::HashMap<String, ExprType>) {
    for s in stmts {
        match &mut s.kind {
            RawStmtKind::Local(l) => {
                if let Some(init) = &mut l.init {
                    subst_in_place(init, subst);
                }
            }
            RawStmtKind::Expr(es) => subst_in_place(&mut es.expr, subst),
            RawStmtKind::Item(_) => {}
        }
    }
}

/// A simple identifier reference (`w`), not a path (`Opcode::LUI`) or empty.
fn simple_ident(path_text: &str) -> Option<String> {
    let compact: String = path_text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.contains("::") || !is_ident(&compact) {
        return None;
    }
    Some(compact)
}

/// If `path` is a `Bits`-style value constructor, return its declared bit width.
/// An explicit turbofish (`Bits::<8>::from_u8`) wins; otherwise the `from_uNN`
/// name implies an NN-bit value (`from_u32` → 32) — the width the constructor
/// names for its source value.
fn constructor_width(path: &str) -> Option<usize> {
    match classify_value_ctor(path) {
        Some(ValueCtor::FromInt { width }) => Some(width),
        _ => None,
    }
}

/// A `Bits`-style value constructor, and how its value and width are determined.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ValueCtor {
    /// `Bits::from_u32(x)` / `Bits::<8>::from_u8(x)` — the value is the single
    /// argument, at a width named by the constructor (or the type turbofish).
    FromInt { width: usize },
    /// A zero-argument constructor with a fixed value whose *width comes from
    /// context*: `Bits::from_lit::<V>()` (value V) and `Bits::zero()` (0).
    /// `width` is the TYPE-position turbofish when the author wrote one
    /// (`Bits::<32>::from_lit::<1>` / `Bits::<32>::zero`) — before it was
    /// honoured, such a literal fell back to the 64-bit default and tripped
    /// WIDTHTRUNC wherever no sibling operand supplied the width (the
    /// `lit_width_in_ternary` ledger entry).
    Const { value: u128, width: Option<usize> },
}

/// Classify a constructor call path.
///
/// The turbofish position matters: a **trailing** one is a method const-parameter
/// (a value — `Bits::from_lit::<1>`), while one **earlier** in the path is the
/// type's width (`Bits::<8>::from_u8`).
fn classify_value_ctor(path: &str) -> Option<ValueCtor> {
    if path.contains("from_lit") {
        return trailing_const_param(path)
            .map(|value| ValueCtor::Const { value, width: leading_type_width(path) });
    }
    if path.ends_with("::zero") || path == "zero" {
        return Some(ValueCtor::Const { value: 0, width: type_turbofish_width(path) });
    }
    for (name, w) in [
        ("from_u128", 128usize),
        ("from_u64", 64),
        ("from_u32", 32),
        ("from_u16", 16),
        ("from_u8", 8),
        // `from_usize` mirrors the 32-bit `usize` resolution; the value is the
        // argument, retyped to the assignment/operand context.
        ("from_usize", 32),
    ] {
        if path.ends_with(name) {
            // An explicit type turbofish overrides the name's implied width.
            return Some(ValueCtor::FromInt {
                width: type_turbofish_width(path).unwrap_or(w),
            });
        }
    }
    None
}

/// The TYPE-position turbofish width of a path that ALSO ends in a method
/// const-parameter: `Bits::<32>::from_lit::<1>` → 32. `None` when the only
/// turbofish is the trailing one (`Bits::from_lit::<1>`), whose value position
/// `type_turbofish_width` already refuses.
fn leading_type_width(path: &str) -> Option<usize> {
    let first = path.find("::<")?;
    let last = path.rfind("::<")?;
    if first == last {
        return None;
    }
    let rest = &path[first + 3..];
    let end = rest.find('>')?;
    rest[..end].trim().parse::<usize>().ok()
}

/// Value of a **trailing** method const-parameter: `Bits::from_lit::<3>` → 3.
fn trailing_const_param(path: &str) -> Option<u128> {
    if !path.ends_with('>') {
        return None;
    }
    let start = path.rfind("::<")? + 3;
    let rest = &path[start..];
    let end = rest.find('>')?;
    rest[..end].trim().parse::<u128>().ok()
}

/// Width from a **type** turbofish: `Bits::<8>::from_u8` → 8. Returns `None` when
/// the turbofish is trailing, since that is a method const-parameter, not a width.
fn type_turbofish_width(path: &str) -> Option<usize> {
    if path.ends_with('>') {
        return None;
    }
    width_from_turbofish(path)
}

/// Extract `N` from a turbofish segment, e.g. `Bits::<8>::from_u8` → `8`.
fn width_from_turbofish(path: &str) -> Option<usize> {
    let start = path.find("::<")? + 3;
    let rest = &path[start..];
    let end = rest.find('>')?;
    rest[..end].trim().parse::<usize>().ok()
}

/// Infer the type of a constructor-style call such as `Bits::from_u32(x)`.
fn infer_type_from_call(call: &ExprCall, span: SourceSpan) -> Result<CHIRType, CHIRLowerError> {
    match call_path(call).as_deref().and_then(constructor_width) {
        Some(w) => Ok(CHIRType::UInt { width: Width::Concrete(w) }),
        None => Err(CHIRLowerError::AmbiguousWidth { span }),
    }
}

// ── Enum encoding ─────────────────────────────────────────────────────────────

/// A hardware encoding for a Rust enum used as state: each variant maps to a
/// concrete value, and the enum occupies enough bits to hold the largest one.
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub width: usize,
    /// Variant name → encoded value.
    pub variants: std::collections::HashMap<String, u128>,
}

/// Enum name → its encoding.
pub type EnumRegistry = std::collections::HashMap<String, EnumDef>;

/// Build encodings for every enum visible to a module.
///
/// Variants use their explicit discriminant when given (`IDLE = 0`), otherwise
/// they continue sequentially from the previous variant — matching Rust's own
/// discriminant rules. Width is the bits needed for the largest value (min 1).
fn build_enum_registry(fir: &FrontendModuleIR) -> EnumRegistry {
    let mut registry = EnumRegistry::new();
    for item in &fir.enums {
        let mut variants = std::collections::HashMap::new();
        let mut next: u128 = 0;
        let mut max: u128 = 0;
        for v in &item.variants {
            let value = v
                .discriminant
                .as_ref()
                .and_then(|d| {
                    let compact: String = d.chars().filter(|c| !c.is_whitespace()).collect();
                    parse_int_literal(&compact).map(|(val, _)| val)
                })
                .unwrap_or(next);
            variants.insert(v.name.clone(), value);
            max = max.max(value);
            next = value + 1;
        }
        registry.insert(item.name.clone(), EnumDef { width: bits_for(max), variants });
    }
    registry
}

/// Registry of functions available for inlining (#7b), by the name a call site
/// uses to reach them:
/// - file-scope free fns, keyed by bare name (`sign_ext_i`);
/// - impl-block **associated** fns, keyed by qualified name (`Opcode::from_bits`)
///   — the form `call_path` yields for a `Type::method(args)` call.
/// Only receiver-less functions are registered; inlining a `self`-taking instance
/// method (called via `receiver.method(..)`) is a later increment.
fn build_fn_registry(fir: &FrontendModuleIR) -> std::collections::HashMap<String, FrontendFnIR> {
    let mut fns: std::collections::HashMap<String, FrontendFnIR> = fir
        .file_fns
        .iter()
        .filter(|f| f.receiver.is_none())
        .map(|f| (f.name.clone(), f.clone()))
        .collect();

    for imp in &fir.file_impls {
        // `self_ty` is raw type text; a plain type name is what a `Type::method`
        // call path uses. Skip anything more complex (generics, references).
        let self_ty: String = imp.self_ty.chars().filter(|c| !c.is_whitespace()).collect();
        for m in imp.methods.iter().filter(|m| m.receiver.is_none()) {
            fns.insert(format!("{self_ty}::{}", m.name), m.clone());
        }
    }

    fns
}

/// Registry of file-scope struct definitions by name, for struct lowering
/// (milestone 2). A struct-valued binding becomes one wire per field.
fn build_struct_registry(fir: &FrontendModuleIR) -> std::collections::HashMap<String, ItemStruct> {
    fir.file_structs
        .iter()
        .map(|s| (s.name.clone(), s.clone()))
        .collect()
}

/// The bit width of a resolved hardware type when it is CONCRETE; `None` for a
/// symbolic (module-parameter) width. `width_of_chir_expr` is an Option-typed
/// query and must DECLINE parametric widths rather than panic — reaching
/// `Width::concrete()` from it took down every parametric module the moment the
/// mux-arm literal balancing asked a width question the binop path had never
/// happened to ask (found by sv-baseline on the BaseJump generics, 2026-08-27).
fn width_of_type_concrete(ty: &CHIRType) -> Option<usize> {
    match ty {
        CHIRType::UInt { width } | CHIRType::SInt { width } => match width {
            Width::Concrete(n) => Some(*n),
            Width::Param(_) => None,
        },
        CHIRType::Bool => Some(1),
        CHIRType::Array { elem, .. } => width_of_type_concrete(elem),
    }
}

/// The bit width of a resolved hardware type.
fn width_of_type(ty: &CHIRType) -> usize {
    match ty {
        CHIRType::UInt { width } | CHIRType::SInt { width } => width.concrete(),
        CHIRType::Bool => 1,
        // The only value obtainable from an array is an element, so every
        // expression-level width question about one is about the element. The
        // outer dimension travels on the port declaration instead.
        CHIRType::Array { elem, .. } => width_of_type(elem),
    }
}

/// The `Width` of a hardware type, symbolic widths included (unlike
/// `width_of_type`, which panics on a `Param`).
fn chir_type_width(ty: &CHIRType) -> Width {
    match ty {
        CHIRType::UInt { width } | CHIRType::SInt { width } => width.clone(),
        CHIRType::Bool => Width::Concrete(1),
        // Element width — see `width_of_type`.
        CHIRType::Array { elem, .. } => chir_type_width(elem),
    }
}

/// The length of an array-repeat expression `[elem; len]`.
///
/// A literal gives a concrete length; a bare identifier is taken as a module
/// parameter, the same rule `Bits<N>` uses in `parse_bits_type`. A computed
/// length (`WIDTH + 1`) is not representable and yields `None`.
fn repeat_len(len: &ExprType) -> Option<Width> {
    match len {
        ExprType::Lit(lit) => {
            let compact: String = lit.text.chars().filter(|c| !c.is_whitespace()).collect();
            parse_int_literal(&compact).map(|(v, _)| Width::Concrete(v as usize))
        }
        ExprType::Path(path) => {
            let compact: String = path.path_text.chars().filter(|c| !c.is_whitespace()).collect();
            is_ident(&compact).then_some(Width::Param(compact))
        }
        _ => None,
    }
}

/// `Bits::from_array(a)` / `Bits::from_slice(&a)` — a no-op on Copper's packed
/// representation, returning the argument to lower in its place.
///
/// `Bits<N>` *is* `[Logic; N]` in `copper-core` (element k at bit k), and an
/// array local lowers to that same packing, so packing one into the other moves
/// no bits. Recognising the identity is what makes an array local usable: build
/// it up with indexed writes, then hand it to a port.
fn identity_pack_call(call: &ExprCall) -> Option<&ExprType> {
    let path = call_path(call)?;
    if !(path.ends_with("from_array") || path.ends_with("from_slice")) {
        return None;
    }
    match call.args.first()? {
        ExprType::Reference(r) => Some(&r.expr),
        other => Some(other),
    }
}

/// Number of bits needed to represent values `0..=max` (minimum 1).
fn bits_for(max: u128) -> usize {
    let mut bits = 1;
    while max >= (1u128 << bits) {
        bits += 1;
    }
    bits
}

/// Resolve a path like `State::IDLE` against the enum registry, returning the
/// enum's hardware type and the variant's encoded value.
fn resolve_enum_path(path: &str, enums: &EnumRegistry) -> Option<(CHIRType, u128)> {
    let (enum_name, variant) = path.rsplit_once("::")?;
    let def = enums.get(enum_name)?;
    let value = *def.variants.get(variant)?;
    Some((CHIRType::UInt { width: Width::Concrete(def.width) }, value))
}

/// Build a symbol table of a module's data ports (`In<T,D>` / `Out<T,D>`) mapping
/// port name → inner hardware type. Clock ports are excluded.
/// Forward width-inference: map an un-annotated local to the type of the output
/// port it is later written to. Copper users write
/// `let mut acc = Bits::zero(); …; out.write(acc);` and Rust infers `acc`'s width
/// from the write; the bottom-up pass can't, so we look ahead for the write.
fn build_write_inferred_types(fir: &FrontendModuleIR, port_symbols: &SymbolTable) -> SymbolTable {
    let out_ports: std::collections::HashSet<String> = fir
        .signature
        .params
        .iter()
        .filter(|p| {
            let c = compact_type(&p.ty.ty_text);
            c.starts_with("Out<")
        })
        .map(|p| p.name.clone())
        .collect();

    let mut inferred = SymbolTable::new();
    collect_writes_in_stmts(&fir.raw_statements, port_symbols, &out_ports, &mut inferred);

    // Propagate widths across `a = b` local-to-local assignments (e.g.
    // `out_n = shifted` gives `shifted` out_n's width, which itself came from
    // `out.write(out_n)`). Fixpoint over the collected pairs. This only ever adds
    // fallback entries used when bottom-up inference is ambiguous, so a spurious
    // pair can't override a width that inference already determines.
    let mut pairs = Vec::new();
    collect_local_assign_pairs(&fir.raw_statements, &mut pairs);
    loop {
        let mut changed = false;
        for (a, b) in &pairs {
            if let Some(ty) = inferred.get(a).cloned() {
                if inferred.insert(b.clone(), ty).is_none() { changed = true; }
            }
            if let Some(ty) = inferred.get(b).cloned() {
                if inferred.insert(a.clone(), ty).is_none() { changed = true; }
            }
        }
        if !changed {
            break;
        }
    }
    inferred
}

/// Collect `(lhs, rhs)` identifier pairs from `a = b` assignments in the body,
/// recursing into loops/branches. Used to propagate forward-inferred widths.
fn collect_local_assign_pairs(stmts: &[RawStmt], out: &mut Vec<(String, String)>) {
    for s in stmts {
        match &s.kind {
            RawStmtKind::Expr(es) => collect_pairs_in_expr(&es.expr, out),
            RawStmtKind::Local(l) => {
                if let Some(i) = &l.init {
                    collect_pairs_in_expr(i, out);
                }
            }
            RawStmtKind::Item(_) => {}
        }
    }
}

fn collect_pairs_in_expr(e: &ExprType, out: &mut Vec<(String, String)>) {
    match e {
        ExprType::Assign(a) => {
            if let (Some(l), Some(r)) = (ident_of_expr(&a.left), ident_of_expr(&a.right)) {
                out.push((l, r));
            }
        }
        ExprType::Loop(l) => collect_local_assign_pairs(&l.body, out),
        ExprType::While(w) => collect_local_assign_pairs(&w.body, out),
        ExprType::Block(b) => collect_local_assign_pairs(&b.stmts, out),
        ExprType::If(f) => {
            collect_local_assign_pairs(&f.then_block, out);
            if let Some(eb) = &f.else_branch {
                collect_pairs_in_expr(eb, out);
            }
        }
        ExprType::Match(m) => {
            for arm in &m.arms {
                collect_pairs_in_expr(&arm.body, out);
            }
        }
        _ => {}
    }
}

fn collect_writes_in_stmts(
    stmts: &[RawStmt],
    ports: &SymbolTable,
    out_ports: &std::collections::HashSet<String>,
    out: &mut SymbolTable,
) {
    for s in stmts {
        match &s.kind {
            RawStmtKind::Local(l) => {
                if let Some(init) = &l.init {
                    collect_writes_in_expr(init, ports, out_ports, out);
                }
            }
            RawStmtKind::Expr(es) => collect_writes_in_expr(&es.expr, ports, out_ports, out),
            RawStmtKind::Item(_) => {}
        }
    }
}

fn collect_writes_in_expr(
    e: &ExprType,
    ports: &SymbolTable,
    out_ports: &std::collections::HashSet<String>,
    out: &mut SymbolTable,
) {
    match e {
        // `<out_port>.write(<local>)` — the local takes the port's type.
        ExprType::MethodCall(mc) if mc.method == "write" && mc.args.len() == 1 => {
            if let (Some(port), Some(local)) =
                (ident_of_expr(&mc.receiver), ident_of_expr(&mc.args[0]))
            {
                if out_ports.contains(&port) {
                    if let Some(ty) = ports.get(&port) {
                        out.entry(local).or_insert_with(|| ty.clone());
                    }
                }
            }
        }
        ExprType::Loop(l) => collect_writes_in_stmts(&l.body, ports, out_ports, out),
        ExprType::While(w) => collect_writes_in_stmts(&w.body, ports, out_ports, out),
        ExprType::Block(b) => collect_writes_in_stmts(&b.stmts, ports, out_ports, out),
        ExprType::If(f) => {
            collect_writes_in_stmts(&f.then_block, ports, out_ports, out);
            if let Some(eb) = &f.else_branch {
                collect_writes_in_expr(eb, ports, out_ports, out);
            }
        }
        ExprType::Match(m) => {
            for arm in &m.arms {
                collect_writes_in_expr(&arm.body, ports, out_ports, out);
            }
        }
        _ => {}
    }
}

/// A plain identifier (`acc`, `out`) from a `Path`/`Lit` expression.
fn ident_of_expr(e: &ExprType) -> Option<String> {
    let text = match e {
        ExprType::Path(p) => &p.path_text,
        ExprType::Lit(l) => &l.text,
        _ => return None,
    };
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    is_ident(&compact).then_some(compact)
}

/// The names in scope for width inference before any local is seen: the module's
/// ports, plus every name that resolves to a SystemVerilog `parameter`/
/// `localparam`.
///
/// Parameters are typed 32-bit because that is what they emit as — `parameter int`
/// / `localparam int` — matching both the `usize` resolution and the `int` a
/// `for`-loop variable already gets a few hundred lines below. Without them,
/// `let mut k = WIDTH - 1;` had no inferable width at all: `WIDTH` was not a
/// signal, `1` is a bare literal, so both sides of the subtraction came back
/// ambiguous and the module was rejected.
///
/// A local that shadows one of these simply overwrites its entry, which is what
/// Rust's own shadowing does.
fn build_symbol_table(fir: &FrontendModuleIR) -> SymbolTable {
    let mut symbols = SymbolTable::new();
    for name in param_names(fir) {
        symbols.insert(name, CHIRType::UInt { width: Width::Concrete(32) });
    }
    for p in &fir.signature.params {
        let compact = compact_type(&p.ty.ty_text);
        let inner = strip_port_wrapper("In<", &compact)
            .or_else(|| strip_port_wrapper("Out<", &compact));
        if let Some(inner) = inner {
            if let Ok(ty) = resolve_type(inner, p.span) {
                symbols.insert(p.name.clone(), ty);
            }
        }
    }
    symbols
}

/// Returns `Some(CHIRType)` if the literal text has an explicit Rust integer suffix.
fn infer_type_from_suffix(compact: &str) -> Option<CHIRType> {
    let suffixes: &[(&str, CHIRType)] = &[
        ("u128", CHIRType::UInt { width: Width::Concrete(128) }),
        ("u64",  CHIRType::UInt { width: Width::Concrete(64) }),
        ("u32",  CHIRType::UInt { width: Width::Concrete(32) }),
        ("u16",  CHIRType::UInt { width: Width::Concrete(16) }),
        ("u8",   CHIRType::UInt { width: Width::Concrete(8) }),
        ("i128", CHIRType::SInt { width: Width::Concrete(128) }),
        ("i64",  CHIRType::SInt { width: Width::Concrete(64) }),
        ("i32",  CHIRType::SInt { width: Width::Concrete(32) }),
        ("i16",  CHIRType::SInt { width: Width::Concrete(16) }),
        ("i8",   CHIRType::SInt { width: Width::Concrete(8) }),
        // 32, not 64: `resolve_type` decided `usize` is 32-bit (matching the SV
        // `int` loop variable, keeping index arithmetic width-consistent) and
        // `from_usize` mirrors it. This table is the LITERAL-SUFFIX path for the
        // same type, so `let x = 0usize` and `let x: usize = 0` must agree —
        // they used to emit a 64-bit and a 32-bit signal, and could then be
        // added together in one expression.
        ("usize", CHIRType::UInt { width: Width::Concrete(32) }),
        ("isize", CHIRType::SInt { width: Width::Concrete(32) }),
    ];
    for (suffix, ty) in suffixes {
        if let Some(base) = compact.strip_suffix(suffix) {
            if !base.is_empty() {
                return Some(ty.clone());
            }
        }
    }
    None
}

/// Try to lower a simple init expression to a `CHIRLit`.
/// Returns `None` for complex expressions (variable references, binary ops, etc.).
fn lower_init_to_lit(expr: &ExprType, enums: &EnumRegistry) -> Option<CHIRLit> {
    let compact: String = match expr {
        ExprType::Lit(lit) => lit.text.chars().filter(|c| !c.is_whitespace()).collect(),
        ExprType::Path(p) => p.path_text.chars().filter(|c| !c.is_whitespace()).collect(),
        _ => return None,
    };

    if let Some((value, _)) = parse_int_literal(&compact) {
        let ty = infer_type_from_suffix(&compact)
            .unwrap_or(CHIRType::UInt { width: Width::Concrete(64) });
        return Some(CHIRLit { ty, value });
    }

    match compact.as_str() {
        "true" => return Some(CHIRLit { ty: CHIRType::Bool, value: 1 }),
        "false" => return Some(CHIRLit { ty: CHIRType::Bool, value: 0 }),
        "Logic::One" => {
            return Some(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(1) }, value: 1 })
        }
        "Logic::Zero" => {
            return Some(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(1) }, value: 0 })
        }
        _ => {}
    }

    // An enum variant path (`State::IDLE`) → its encoded reset value.
    resolve_enum_path(&compact, enums).map(|(ty, value)| CHIRLit { ty, value })
}

// ── Port wrapper helpers ──────────────────────────────────────────────────────

/// Extract inner type `T` from `In<T,D>` or `Out<T,D>`.
/// `prefix` is `"In<"` or `"Out<"`, `compact` is the whitespace-stripped type text.
fn strip_port_wrapper<'a>(prefix: &str, compact: &'a str) -> Option<&'a str> {
    let after_open = compact.strip_prefix(prefix)?;
    let content = outer_generic_content(after_open)?;
    Some(first_comma_split(content))
}

/// Content before the first `>` at bracket depth 0 in `after_open`.
/// "Logic,MainClk>" → "Logic,MainClk"
/// "Bits<8>,MainClk>" → "Bits<8>,MainClk"
fn outer_generic_content(after_open: &str) -> Option<&str> {
    let mut depth = 0usize;
    for (i, c) in after_open.char_indices() {
        match c {
            '<' => depth += 1,
            '>' if depth == 0 => return Some(&after_open[..i]),
            '>' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Text before the first top-level comma in `s`.
/// "T,D" → "T"   "Bits<8>,D" → "Bits<8>"
fn first_comma_split(s: &str) -> &str {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return &s[..i],
            _ => {}
        }
    }
    s
}

// ── Port extraction ───────────────────────────────────────────────────────────

/// The module's `Memory<…>` PARAMETERS, as received-memory declarations.
///
/// `Memory<ELEM, R, W, DOMAIN, READ_LAT, WRITE_LAT>` — the depth is a runtime
/// constructor argument and is NOT in the type, which is exactly why a received
/// memory's address width becomes a module parameter at emission. The collision
/// policy is likewise the owner's object, so `write_mode` is fixed at
/// `ReadFirst` here and never consulted for a received memory (no child-side
/// forwarding mux exists — the read-data net is an input).
fn received_memory_decls(fir: &FrontendModuleIR) -> Result<Vec<CHIRMemoryDecl>, CHIRLowerError> {
    let mut out = Vec::new();
    for param in &fir.signature.params {
        let compact = compact_type(&param.ty.ty_text);
        let Some(args) = compact.strip_prefix("Memory<").and_then(|s| s.strip_suffix('>')) else {
            continue;
        };
        let parts = split_top_level_commas(args);
        if parts.len() != 6 {
            return Err(CHIRLowerError::UnsupportedConstruct {
                description: format!(
                    "`Memory` parameter `{}` must spell all six generics \
                     (Memory<T, R, W, Domain, READ_LAT, WRITE_LAT>)",
                    param.name
                ),
                span: param.span,
                suggested_rewrite: None,
            });
        }
        let elem_ty = resolve_type(parts[0].trim(), param.ty.span)?;
        let int = |i: usize| -> Result<usize, CHIRLowerError> {
            parts[i].trim().parse::<usize>().map_err(|_| CHIRLowerError::UnsupportedConstruct {
                description: format!(
                    "`Memory` parameter `{}`: generic argument `{}` must be a literal integer",
                    param.name,
                    parts[i].trim()
                ),
                span: param.span,
                suggested_rewrite: None,
            })
        };
        let write_lat = int(5)?;
        if write_lat != 1 {
            // The bus carries the freshly-staged write nets; at WRITE_LAT > 1
            // the value that commits is a child-side pipeline register instead,
            // and wiring the stage-0 nets to the owner would commit a write
            // WRITE_LAT-1 edges early. Exposing the committing stage is a
            // straightforward extension — refuse honestly until it is built
            // and verified, rather than emit the early-commit silently.
            return Err(CHIRLowerError::UnsupportedConstruct {
                description: format!(
                    "received `Memory` parameter `{}` has WRITE_LAT = {write_lat}; the \
                     bus ABI currently supports WRITE_LAT = 1 only",
                    param.name
                ),
                span: param.span,
                suggested_rewrite: Some(
                    "declare the memory inside the module, or use WRITE_LAT = 1".to_string(),
                ),
            });
        }
        out.push(CHIRMemoryDecl {
            name: param.name.clone(),
            elem_ty,
            depth: 0,
            read_ports: int(1)?,
            write_ports: int(2)?,
            read_lat: int(4)?,
            write_lat,
            init: None,
            received: true,
            write_mode: copper_core::memory::WriteMode::ReadFirst,
            span: param.span,
        });
    }
    Ok(out)
}

fn lower_ports(fir: &FrontendModuleIR) -> Result<Vec<CHIRPort>, CHIRLowerError> {
    let mut ports = Vec::new();

    for param in &fir.signature.params {
        let compact = compact_type(&param.ty.ty_text);

        if compact.starts_with("Clock<") {
            let domain = compact
                .strip_prefix("Clock<")
                .and_then(|s| s.strip_suffix('>'))
                .unwrap_or("default")
                .to_string();
            ports.push(CHIRPort {
                name: param.name.clone(),
                direction: CHIRPortDir::Input,
                kind: CHIRPortKind::Clock { domain },
                registered: false,
                span: param.span,
            });
        } else if let Some(inner) = strip_port_wrapper("In<", &compact) {
            let ty = resolve_type(inner, param.ty.span)?;
            ports.push(CHIRPort {
                name: param.name.clone(),
                direction: CHIRPortDir::Input,
                kind: CHIRPortKind::Data { ty },
                registered: false,
                span: param.span,
            });
        } else if let Some(inner) = strip_port_wrapper("RegOut<", &compact) {
            // Registered output: same data port as `Out`, but its value commits at
            // the clock edge → driven from `always_ff` (an enabled flip-flop), so a
            // held/conditional output is a real register, not a latch, and matches
            // the simulator's `RegOut` (+1) timing. See design_docs/REGISTERED_OUTPUTS.md.
            let ty = resolve_type(inner, param.ty.span)?;
            ports.push(CHIRPort {
                name: param.name.clone(),
                direction: CHIRPortDir::Output,
                kind: CHIRPortKind::Data { ty },
                registered: true,
                span: param.span,
            });
        } else if let Some(inner) = strip_port_wrapper("Out<", &compact) {
            let ty = resolve_type(inner, param.ty.span)?;
            ports.push(CHIRPort {
                name: param.name.clone(),
                direction: CHIRPortDir::Output,
                kind: CHIRPortKind::Data { ty },
                registered: false,
                span: param.span,
            });
        } else if compact.starts_with("Memory<") {
            // A RECEIVED memory is not a data port: it lowers to a bus (address/
            // data/enable ports) synthesized at emission, with the array on the
            // owner's side — see `CHIRMemoryDecl::received` and
            // design_docs/RECEIVED_MEMORY_ABI.md. Collected by
            // `received_memory_decls`, not here.
            continue;
        } else {
            // Plain type — input data port (for combinational modules and hardware submodules)
            let ty = resolve_type(&param.ty.ty_text, param.ty.span)?;
            ports.push(CHIRPort {
                name: param.name.clone(),
                direction: CHIRPortDir::Input,
                kind: CHIRPortKind::Data { ty },
                registered: false,
                span: param.span,
            });
        }
    }

    // Return type → output port named "out" (combinational-module style)
    if let Some(ret_ty) = &fir.signature.return_ty {
        let ty = resolve_type(&ret_ty.ty_text, ret_ty.span)?;
        ports.push(CHIRPort {
            name: "out".to_string(),
            direction: CHIRPortDir::Output,
            kind: CHIRPortKind::Data { ty },
            registered: false,
            span: ret_ty.span,
        });
    }

    Ok(ports)
}

// ── Combinational body ────────────────────────────────────────────────────────

fn lower_comb_body(
    fir: &FrontendModuleIR,
    hardware_fns: &std::collections::HashSet<String>,
    registry: &ModuleRegistry,
) -> Result<CHIRCombBody, CHIRLowerError> {
    let has_return = fir.signature.return_ty.is_some();
    let mut ctx = LowerCtx::new(hardware_fns, registry);
    ctx.params = param_names(fir);
    ctx.output_ports = fir.signature.params.iter()
        .filter_map(|p| {
            let compact = compact_type(&p.ty.ty_text);
            if compact.starts_with("Out<") { Some(p.name.clone()) } else { None }
        })
        .collect();
    ctx.symbols = build_symbol_table(fir);
    ctx.enums = build_enum_registry(fir);
    ctx.fns = build_fn_registry(fir);
    ctx.structs = build_struct_registry(fir);
    ctx.write_inferred = build_write_inferred_types(fir, &ctx.symbols);

    let mut stmts = Vec::new();

    for raw_stmt in &fir.raw_statements {
        match &raw_stmt.kind {
            RawStmtKind::Local(local) => {
                lower_local_binding(local, &mut ctx, &mut stmts)?;
            }
            RawStmtKind::Expr(expr_stmt) => {
                if !expr_stmt.has_semi && has_return {
                    // Final expression in return-type function → drive the "out" port
                    let value = lower_expr(&expr_stmt.expr, &mut ctx)?;
                    stmts.push(CHIRStmt::PortWrite {
                        port_name: "out".to_string(),
                        value,
                        span: expr_stmt.span,
                    });
                } else {
                    lower_expr_stmt(&expr_stmt.expr, raw_stmt.span, &mut ctx, &mut stmts)?;
                }
            }
            RawStmtKind::Item(_) => {}
        }
    }

    Ok(CHIRCombBody {
        submodules: ctx.submodules,
        stmts,
    })
}

// ── Sequential body ───────────────────────────────────────────────────────────

fn lower_seq_body(
    fir: &FrontendModuleIR,
    hardware_fns: &std::collections::HashSet<String>,
    registry: &ModuleRegistry,
) -> Result<CHIRSeqBody, CHIRLowerError> {
    let clock = fir.clocks.first()
        .map(|c| c.param_name.clone())
        .ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
            description: "sequential module has no clock parameter".to_string(),
            span: fir.span,
            suggested_rewrite: None,
        })?;

    let mut registers = Vec::new();
    let mut memories: Vec<CHIRMemoryDecl> = Vec::new();
    let mut mem_infos: std::collections::HashMap<String, MemInfo> =
        std::collections::HashMap::new();
    // RECEIVED memories — `Memory<…>` PARAMETERS — join `memories` first, so the
    // body lowering (staging nets, capture pipeline, write stages) treats them
    // exactly like a declared memory; only emission differs (bus ports, no
    // array). See `CHIRMemoryDecl::received`.
    for decl in received_memory_decls(fir)? {
        mem_infos.insert(
            decl.name.clone(),
            MemInfo {
                elem_ty: decl.elem_ty.clone(),
                read_ports: decl.read_ports,
                write_ports: decl.write_ports,
            },
        );
        memories.push(decl);
    }
    // (index into `memories`, its source-level preload) — see `RawMemInit`.
    let mut pending_mem_inits: Vec<(usize, RawMemInit)> = Vec::new();
    let mut loop_body_stmts: Option<&[RawStmt]> = None;
    // Pre-loop non-`mut` `let`s: combinational constants/wires available in the
    // loop body. Collected here, lowered once the context exists, and prepended
    // to the loop body. Stored as (name, type, init expr, span).
    let mut pre_loop_wires: Vec<(String, CHIRType, &ExprType, SourceSpan)> = Vec::new();
    // Seeded with the module's ports so pre-loop register inits that reference
    // ports (or later registers) can infer their width.
    let mut symbols = build_symbol_table(fir);
    let enums = build_enum_registry(fir);
    // Forward inference: a register/wire whose width the init can't determine
    // (`let mut out_n = Bits::x()`) takes the type of the output port it drives.
    let write_inferred = build_write_inferred_types(fir, &symbols);
    // Needed during the scan itself: a struct-valued `let mut` (a pipeline
    // latch, `let mut if_id = IFIDReg::bubble();`) flattens to one register per
    // field, which takes seeing through the ctor call before the context exists.
    let fns_reg = build_fn_registry(fir);
    let structs_reg = build_struct_registry(fir);
    let mut pre_struct_locals: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for stmt in &fir.raw_statements {
        match &stmt.kind {
            RawStmtKind::Local(local) if local.is_mut => {
                // A struct-valued latch flattens to one register per declared
                // field, named `<local>_<field>` — the same flattening every
                // struct WIRE binding gets, so a field read (`if_id.pc` →
                // `if_id_pc`) is one scheme for both. The register authority is
                // consulted for the WHOLE name: the source-level inference sees
                // `if_id`, not the fields.
                if let Some(init) = &local.init {
                    if let Some(lit) = resolve_struct_literal_in(init, &fns_reg, &structs_reg)? {
                        let sname = compact_ident(&lit.path_text);
                        if !fir.registers.iter().any(|r| r == &local.name) {
                            return Err(CHIRLowerError::UnsupportedConstruct {
                                description: format!(
                                    "struct-typed pre-loop `let mut {}` is not a register \
                                     (never live across a clock edge); bind it with a plain \
                                     `let` instead",
                                    local.name
                                ),
                                span: local.span,
                                suggested_rewrite: None,
                            });
                        }
                        for (fname, fty) in
                            struct_fields_in(&sname, &structs_reg, &enums, local.span)?
                        {
                            let finit = lit
                                .fields
                                .iter()
                                .find(|f| f.member == fname)
                                .and_then(|f| lower_init_to_lit(&f.expr, &enums));
                            let rname = format!("{}_{fname}", local.name);
                            symbols.insert(rname.clone(), fty.clone());
                            registers.push(CHIRRegDecl {
                                name: rname,
                                ty: fty,
                                init: finit,
                                span: local.span,
                            });
                        }
                        pre_struct_locals.insert(local.name.clone(), sname);
                        continue;
                    }
                }
                let ty = match (&local.ty, &local.init) {
                    (Some(t), _) => resolve_type(&t.ty_text, t.span)?,
                    (None, Some(init)) => match infer_type_from_expr(init, local.span, &symbols, &enums) {
                        Ok(t) => t,
                        Err(e) => write_inferred.get(&local.name).cloned().ok_or(e)?,
                    },
                    (None, None) => return Err(CHIRLowerError::AmbiguousWidth { span: local.span }),
                };
                symbols.insert(local.name.clone(), ty.clone());
                // Register or wire? CONSULT the FIR's register authority — the
                // shared source-level liveness inference plus the names the
                // FIR→FIR passes synthesized (see `FrontendModuleIR::registers`).
                // The old rule here — every pre-loop `let mut` is a register —
                // was a second, syntactic decider that merely happened to agree
                // with the shared inference (register_reconciliation.rs was the
                // measurement); making this arm consume the authority closes the
                // register half of the c2 obligation. A pre-loop `let mut` the
                // authority calls a wire lowers exactly like a pre-loop `let`
                // (a combinational wire, often a constant). A demoted local
                // WITHOUT an init has no wire value to take, so it stays a
                // register conservatively — no corpus instance exists.
                if fir.registers.iter().any(|r| r == &local.name) || local.init.is_none() {
                    let init = local.init.as_ref().and_then(|e| lower_init_to_lit(e, &enums));
                    registers.push(CHIRRegDecl {
                        name: local.name.clone(),
                        ty,
                        init,
                        span: local.span,
                    });
                } else if let Some(init) = &local.init {
                    pre_loop_wires.push((local.name.clone(), ty, init, local.span));
                }
            }
            RawStmtKind::Local(local) => {
                // A `Memory<..>` binding is a hardware submodule (an array plus
                // per-port buses), not a wire — it has no bit width to infer.
                if let Some((decl, raw_init)) =
                    parse_memory_decl(&local.name, local.init.as_ref(), local.span)?
                {
                    mem_infos.insert(
                        local.name.clone(),
                        MemInfo {
                            elem_ty: decl.elem_ty.clone(),
                            read_ports: decl.read_ports,
                            write_ports: decl.write_ports,
                        },
                    );
                    if let Some(raw) = raw_init {
                        // Lowered once the context exists, like the pre-loop wires.
                        pending_mem_inits.push((memories.len(), raw));
                    }
                    memories.push(decl);
                    continue;
                }
                // Pre-loop non-`mut` `let` → a combinational wire (often a
                // constant) visible throughout the loop body.
                if let Some(init) = &local.init {
                    let ty = match &local.ty {
                        Some(t) => resolve_type(&t.ty_text, t.span)?,
                        None => match infer_type_from_expr(init, local.span, &symbols, &enums) {
                            Ok(t) => t,
                            Err(e) => write_inferred.get(&local.name).cloned().ok_or(e)?,
                        },
                    };
                    symbols.insert(local.name.clone(), ty.clone());
                    pre_loop_wires.push((local.name.clone(), ty, init, local.span));
                }
            }
            RawStmtKind::Expr(expr_stmt) => {
                match &expr_stmt.expr {
                    ExprType::Loop(loop_expr) => {
                        loop_body_stmts = Some(&loop_expr.body);
                        break;
                    }
                    ExprType::While(_) => {
                        return Err(CHIRLowerError::UnsupportedConstruct {
                            description: "while loops are not supported; use `loop { ... clk.tick().await; }`".to_string(),
                            span: stmt.span,
                            suggested_rewrite: Some("replace `while cond { body }` with `loop { if !cond { break; } body clk.tick().await; }`".to_string()),
                        });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let loop_stmts = loop_body_stmts.ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
        description: "sequential module must have a top-level `loop` block".to_string(),
        span: fir.span,
        suggested_rewrite: Some("wrap the module body in `loop { ... }`".to_string()),
    })?;

    let mut ctx = LowerCtx::new(hardware_fns, registry);
    ctx.params = param_names(fir);
    ctx.clock_name = clock.clone();
    ctx.output_ports = fir.signature.params.iter()
        .filter_map(|p| {
            let compact = compact_type(&p.ty.ty_text);
            if compact.starts_with("Out<") { Some(p.name.clone()) } else { None }
        })
        .collect();
    ctx.symbols = symbols;
    ctx.enums = enums;
    ctx.fns = build_fn_registry(fir);
    ctx.structs = build_struct_registry(fir);
    ctx.write_inferred = build_write_inferred_types(fir, &ctx.symbols);
    ctx.struct_locals = pre_struct_locals;
    ctx.memories = mem_infos;

    // Preloads, now that expressions can be lowered. Each word is retyped and
    // width-cast to the element type, the same treatment a `let` binding's
    // initializer gets — a bare `0` in a 16-bit memory must be `16'd0`.
    for (idx, raw) in pending_mem_inits {
        let elem_ty = memories[idx].elem_ty.clone();
        let ew = type_width(&elem_ty);
        let lower_word = |e: &ExprType, ctx: &mut LowerCtx| -> Result<CHIRExpr, CHIRLowerError> {
            let v = retype_default_literals_in_values(lower_expr(e, ctx)?, ew.clone());
            // Stricter than `resize_to_target`: an index expression like `i * 3 + 7`
            // has no inferable width, and an UNRESIZED assignment into the array is
            // a Verilator width warning — fatal under `-Wall`. So anything not
            // already known to be element-width gets an explicit cast.
            Ok(if expr_width(&v, ctx) == Some(ew.clone()) {
                v
            } else {
                CHIRExpr::Resize { expr: Box::new(v), width: ew.clone() }
            })
        };
        // A preload is power-on contents: it must be a constant expression, not a
        // reading of the design's own state. The simulator would evaluate a
        // captured port once at construction and an `initial` block would sample
        // it at time 0 — two different things that happen to look alike, so the
        // shape is refused rather than emitted.
        let span = memories[idx].span;
        let mem_name = memories[idx].name.clone();
        let check_const = |e: &CHIRExpr, fill_var: Option<&str>, ctx: &LowerCtx| {
            let mut bad: Option<String> = None;
            walk_chir_expr(e, &mut |x| {
                if let CHIRExpr::Var(n) = x {
                    if Some(n.as_str()) != fill_var && ctx.symbols.contains_key(n.as_str()) {
                        bad.get_or_insert_with(|| n.clone());
                    }
                }
            });
            match bad {
                None => Ok(()),
                Some(n) => Err(CHIRLowerError::UnsupportedConstruct {
                    description: format!(
                        "memory `{mem_name}`: the preload reads `{n}`, which is a signal of this \
                         module. Initial contents must be constant — a signal has no value before \
                         the design starts running"
                    ),
                    span,
                    suggested_rewrite: None,
                }),
            }
        };

        memories[idx].init = Some(match raw {
            RawMemInit::Fill { var, body } => {
                // The fill index is an integer in scope only for the fill body.
                let shadowed = ctx.symbols.insert(
                    var.clone(),
                    CHIRType::UInt { width: Width::Concrete(32) },
                );
                let value = lower_word(body, &mut ctx);
                match shadowed {
                    Some(prev) => {
                        ctx.symbols.insert(var.clone(), prev);
                    }
                    None => {
                        ctx.symbols.remove(&var);
                    }
                }
                let value = value?;
                check_const(&value, Some(&var), &ctx)?;
                CHIRMemInit::Fill { var, value }
            }
            RawMemInit::Words(words) => {
                let lowered: Vec<CHIRExpr> = words
                    .iter()
                    .map(|w| lower_word(w, &mut ctx))
                    .collect::<Result<_, _>>()?;
                for w in &lowered {
                    check_const(w, None, &ctx)?;
                }
                CHIRMemInit::Words(lowered)
            }
        });
    }

    // Emit pre-loop wires first so they are declared before any use, then the
    // loop body itself.
    let mut loop_body = Vec::new();
    for (name, ty, init, span) in &pre_loop_wires {
        let value = lower_expr(init, &mut ctx)?;
        loop_body.push(CHIRStmt::Wire {
            name: name.clone(),
            ty: ty.clone(),
            value,
            span: *span,
        });
    }
    loop_body.extend(lower_stmts(loop_stmts, &mut ctx)?);

    let has_tick = loop_body.iter().any(|s| matches!(s, CHIRStmt::AwaitTick { .. }));
    if !has_tick {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: "sequential module loop body has no clk.tick().await".to_string(),
            span: fir.span,
            suggested_rewrite: Some("add `clk.tick().await;` inside the loop".to_string()),
        });
    }

    Ok(CHIRSeqBody {
        clock,
        registers,
        memories,
        submodules: ctx.submodules,
        loop_body,
    })
}

// ── Structural body (item 4: hierarchical clocked instantiation) ──────────────

/// Lower a `#[hardware(structural)]` parent: a pure hierarchy of clocked
/// submodule instances wired together by internal nets. No registers, no loop.
///
/// Two body forms are recognised, in source order:
///   * `let <net> = wire::<T, D>(<init>);` — declares an internal net wiring
///     children together; its width comes from `T`. Use sites reference the
///     driver as `<net>.0` and the observer as `<net>.1`; both resolve to the
///     one SV net named `<net>`.
///   * `child(<args>...)` — a submodule instantiation (a hardware-module call in
///     statement position). Args are positional against the child's declared
///     params: `Clock` params become clock port connections, every other port
///     (In/Out) becomes a named net connection.
fn lower_structural_body(
    fir: &FrontendModuleIR,
    registry: &ModuleRegistry,
) -> Result<CHIRStructuralBody, CHIRLowerError> {
    let mut nets: Vec<(String, CHIRType)> = Vec::new();
    let mut submodules: Vec<CHIRSubmoduleInst> = Vec::new();
    let mut inst_counters: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // signal → clock domain, for the call-site CDC check. Seeded with the
    // parent's own ports and clocks; internal nets are added as they are declared.
    let mut signal_domains: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for p in &fir.signature.params {
        if let Some(d) = domain_of_type_text(&p.ty.ty_text) {
            signal_domains.insert(p.name.clone(), d);
        }
    }

    for stmt in &fir.raw_statements {
        match &stmt.kind {
            RawStmtKind::Local(local) => {
                let Some(init) = &local.init else {
                    return Err(CHIRLowerError::UnsupportedConstruct {
                        description: "structural module: `let` without initializer".to_string(),
                        span: local.span,
                        suggested_rewrite: Some("declare internal nets with `let n = wire::<T, D>(init);`".to_string()),
                    });
                };
                let (ty, domain) = parse_wire_net(init, local.span)?;
                if let Some(d) = domain {
                    signal_domains.insert(local.name.clone(), d);
                }
                nets.push((local.name.clone(), ty));
            }
            RawStmtKind::Expr(expr_stmt) => {
                let ExprType::Call(call) = &expr_stmt.expr else {
                    return Err(CHIRLowerError::UnsupportedConstruct {
                        description: "structural module body may only contain net declarations and submodule instantiations".to_string(),
                        span: expr_stmt.span,
                        suggested_rewrite: None,
                    });
                };
                if !call.is_hardware_module {
                    return Err(CHIRLowerError::UnsupportedConstruct {
                        description: "structural module: call is not a #[hardware] submodule".to_string(),
                        span: call.span,
                        suggested_rewrite: None,
                    });
                }
                submodules.push(lower_structural_inst(call, registry, &mut inst_counters, &signal_domains)?);
            }
            RawStmtKind::Item(_) => {}
        }
    }

    Ok(CHIRStructuralBody { nets, submodules })
}

/// Parse the data type `T` and clock domain `D` of an internal net declared by
/// `wire::<T, D>(init)`. `D` is `None` only when the turbofish omits it.
fn parse_wire_net(init: &ExprType, span: SourceSpan) -> Result<(CHIRType, Option<String>), CHIRLowerError> {
    let ExprType::Call(call) = init else {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: "structural net must be initialized with `wire::<T, D>(init)`".to_string(),
            span,
            suggested_rewrite: None,
        });
    };
    let ExprType::Path(p) = call.func.as_ref() else {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: "structural net initializer must be a `wire::<T, D>(..)` call".to_string(),
            span,
            suggested_rewrite: None,
        });
    };
    // path_text is like `wire :: < Logic , ClkFast >` (possibly module-qualified).
    let text: String = p.path_text.chars().filter(|c| !c.is_whitespace()).collect();
    if !text.contains("wire::<") {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: "structural net must be declared with an explicit `wire::<T, D>(init)` type".to_string(),
            span,
            suggested_rewrite: Some("annotate the net type: `let n = wire::<Bits<8>, D>(init);`".to_string()),
        });
    }
    let after = &text[text.find("wire::<").unwrap() + "wire::<".len()..];
    let inner = after.strip_suffix('>').unwrap_or(after);
    // `T, D` split at the top level.
    let parts = split_top_level_commas(inner);
    let t = parts.first().copied().unwrap_or_default();
    let domain = parts.get(1).map(|d| d.trim().to_string());
    Ok((resolve_type(t, span)?, domain))
}

/// The clock domain a port/clock type text is in, if determinable.
/// `In<T,D>` / `Out<T,D>` / `RegOut<T,D>` → `D` (the 2nd generic arg; the
/// unit domain `()` for the one-arg shorthand `In<T>`). `Clock<D>` → `D`.
/// Any other type → `None`.
fn domain_of_type_text(ty_text: &str) -> Option<String> {
    let compact = compact_type(ty_text);
    let lt = compact.find('<')?;
    let gt = compact.rfind('>')?;
    if gt <= lt + 1 {
        return None;
    }
    let args = split_top_level_commas(&compact[lt + 1..gt]);
    if compact.starts_with("Clock<") {
        args.first().map(|s| s.trim().to_string())
    } else if compact.starts_with("In<") || compact.starts_with("Out<") || compact.starts_with("RegOut<") {
        Some(args.get(1).map(|s| s.trim().to_string()).unwrap_or_else(|| "()".to_string()))
    } else {
        None
    }
}

/// Lower one submodule instantiation in a structural body.
fn lower_structural_inst(
    call: &ExprCall,
    registry: &ModuleRegistry,
    inst_counters: &mut std::collections::HashMap<String, usize>,
    signal_domains: &std::collections::HashMap<String, String>,
) -> Result<CHIRSubmoduleInst, CHIRLowerError> {
    let module_name = match call.func.as_ref() {
        ExprType::Path(p) => p.path_text.trim().to_string(),
        _ => return Err(CHIRLowerError::UnsupportedConstruct {
            description: "structural instantiation with non-identifier callee".to_string(),
            span: call.span,
            suggested_rewrite: None,
        }),
    };

    let count = inst_counters.entry(module_name.clone()).or_insert(0);
    let inst_name = format!("{}_{}", module_name, count);
    *count += 1;

    let callee = registry.get(&module_name).ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
        description: format!("structural instantiation of unknown module `{module_name}`"),
        span: call.span,
        suggested_rewrite: None,
    })?;

    let params = &callee.signature.params;
    if call.args.len() != params.len() {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: format!(
                "structural instantiation of `{}` passes {} args but the module has {} ports \
                 (pass every clock and data port positionally)",
                module_name, call.args.len(), params.len()
            ),
            span: call.span,
            suggested_rewrite: None,
        });
    }

    let mut clocks: Vec<(String, String)> = Vec::new();
    let mut port_nets: Vec<(String, String)> = Vec::new();
    for (param, arg) in params.iter().zip(call.args.iter()) {
        let signal = structural_signal_name(arg, call.span)?;

        // Call-site CDC / domain-consistency check. The connected signal's clock
        // domain must equal the child port's declared domain. For compiled code
        // the phantom domain types enforce this already (wiring a `ClkFast` net
        // into a `ClkSlow` port is a nominal type error); the transpiler is
        // text-based and never type-checks, so it re-derives the same rule here —
        // mirroring how it re-runs `check_reachability`. A regular child's ports
        // are all its own clock domain, so a foreign net wired into one is
        // rejected here; a `#[hardware(synchronizer)]` child legitimately
        // declares a foreign-domain input, so its net domains still match and it
        // passes — the sanctioned crossing point.
        if let (Some(port_dom), Some(sig_dom)) =
            (domain_of_type_text(&param.ty.ty_text), signal_domains.get(&signal))
        {
            if &port_dom != sig_dom {
                return Err(CHIRLowerError::UnsupportedConstruct {
                    description: format!(
                        "clock-domain crossing: wiring `{signal}` (domain `{sig_dom}`) into `{module_name}` \
                         port `{}` (domain `{port_dom}`). A regular module may not cross clock domains — \
                         bring the signal across with a `#[hardware(synchronizer)]` child (e.g. `copper::sync_2ff`), \
                         then wire that synchronizer's output here",
                        param.name
                    ),
                    span: call.span,
                    suggested_rewrite: None,
                });
            }
        }

        let compact = compact_type(&param.ty.ty_text);
        if compact.starts_with("Clock<") {
            clocks.push((param.name.clone(), signal));
        } else {
            port_nets.push((param.name.clone(), signal));
        }
    }

    Ok(CHIRSubmoduleInst {
        inst_name,
        module_name,
        inputs: Vec::new(),
        output_wire: String::new(),
        output_ty: CHIRType::UInt { width: Width::Concrete(1) },
        clocks,
        port_nets,
        output_port: None,
        span: call.span,
    })
}

/// Resolve a structural instantiation argument to a signal name: a parent port
/// (`count_out`), an internal net's driver/observer (`flag.0` / `flag.1` → `flag`),
/// or a cloned clock (`wr_clk.clone()` → `wr_clk`).
fn structural_signal_name(arg: &ExprType, span: SourceSpan) -> Result<String, CHIRLowerError> {
    match arg {
        ExprType::Path(p) => Ok(p.path_text.trim().to_string()),
        ExprType::Field(f) => structural_signal_name(f.base.as_ref(), span),
        ExprType::MethodCall(mc) if mc.method == "clone" => {
            structural_signal_name(mc.receiver.as_ref(), span)
        }
        _ => Err(CHIRLowerError::UnsupportedConstruct {
            description: "structural port argument must be a port name, an internal net (`net.0`/`net.1`), or a cloned clock".to_string(),
            span,
            suggested_rewrite: None,
        }),
    }
}

// ── Lowering context ──────────────────────────────────────────────────────────

pub(crate) struct LowerCtx<'a> {
    // NOTE: threaded through the lowering pipeline but not yet consumed here.
    // See run-copper setup notes — candidate for a scoped "unthread hardware_fns"
    // cleanup or a real use (submodule detection).
    #[allow(dead_code)]
    hardware_fns: &'a std::collections::HashSet<String>,
    registry: &'a ModuleRegistry,
    submodules: Vec<CHIRSubmoduleInst>,
    inst_counters: std::collections::HashMap<String, usize>,
    clock_name: String,
    /// Names of `Out<T,D>` ports — used to validate `.write()` targets.
    output_ports: std::collections::HashSet<String>,
    /// In-scope names (ports, wires, registers) → type, for width inference.
    symbols: SymbolTable,
    /// Enum encodings visible to this module.
    enums: EnumRegistry,
    /// Match-arm pattern bindings in scope: binder name → the scrutinee element
    /// it names (e.g. `t` in `(Phase::Yellow, t, _)` → `timer`). Populated only
    /// while lowering that arm's guard and body.
    bindings: std::collections::HashMap<String, CHIRExpr>,
    /// File-scope free functions available for inlining (#7b), by name. Built
    /// from `FrontendModuleIR::file_fns`.
    fns: std::collections::HashMap<String, FrontendFnIR>,
    /// Locals (wires AND flattened registers) of struct type: name → struct
    /// name. What lets a whole-struct copy (`ex_mem = new_ex_mem;`) and a
    /// struct-typed leaf in a conditional tree expand per-field.
    struct_locals: std::collections::HashMap<String, String>,
    /// Free-fn names currently being inlined, to detect (and reject) recursion.
    inlining: std::collections::HashSet<String>,
    /// File-scope struct definitions by name, for struct lowering (milestone 2):
    /// a struct value binds one wire per field (`<base>_<field>`).
    structs: std::collections::HashMap<String, ItemStruct>,
    /// Forward-inferred local types: an un-annotated `let x = <context-width
    /// ctor>` (e.g. `Bits::zero()`) whose width the bottom-up pass can't derive
    /// takes the type of the output port it is later written to (`out.write(x)`).
    write_inferred: SymbolTable,
    /// `Memory<..>` instances declared before the loop, by binding name.
    memories: std::collections::HashMap<String, MemInfo>,
    /// Module parameters and file-scope constants in scope. These are NOT
    /// signals — they emit as SystemVerilog `parameter int` / `localparam int`,
    /// i.e. 32 bits — so a bare literal compared against one must be sized to 32
    /// rather than left at the 64-bit default (`ELS_P == 64'd1` is a Verilator
    /// WIDTHEXPAND error under `-Wall`).
    params: std::collections::HashSet<String>,
}

impl<'a> LowerCtx<'a> {
    fn new(
        hardware_fns: &'a std::collections::HashSet<String>,
        registry: &'a ModuleRegistry,
    ) -> Self {
        Self {
            hardware_fns,
            registry,
            submodules: Vec::new(),
            inst_counters: std::collections::HashMap::new(),
            clock_name: String::new(),
            output_ports: std::collections::HashSet::new(),
            symbols: SymbolTable::new(),
            enums: EnumRegistry::new(),
            bindings: std::collections::HashMap::new(),
            fns: std::collections::HashMap::new(),
            struct_locals: std::collections::HashMap::new(),
            inlining: std::collections::HashSet::new(),
            structs: std::collections::HashMap::new(),
            write_inferred: SymbolTable::new(),
            memories: std::collections::HashMap::new(),
            params: std::collections::HashSet::new(),
        }
    }

    fn next_inst_name(&mut self, module_name: &str) -> (String, String) {
        let count = self.inst_counters.entry(module_name.to_string()).or_insert(0);
        let inst_name = format!("{}_{}", module_name, count);
        let output_wire = format!("{}_out", inst_name);
        *count += 1;
        (inst_name, output_wire)
    }
}

// ── Statement lowering ────────────────────────────────────────────────────────

fn lower_stmts(stmts: &[RawStmt], ctx: &mut LowerCtx) -> Result<Vec<CHIRStmt>, CHIRLowerError> {
    let mut out = Vec::new();
    for stmt in stmts {
        lower_stmt(stmt, ctx, &mut out)?;
    }
    Ok(out)
}

// ── Local bindings (scalar + struct, milestone 2) ─────────────────────────────

/// Lower a `let` binding. A struct value (a struct literal, or a call that
/// inlines to one) binds one wire per field — `<base>_<field>` — matching the
/// name that field access (`base.field`) already lowers to. Everything else is a
/// single scalar wire.
fn lower_local_binding(
    local: &LocalStmt,
    ctx: &mut LowerCtx,
    out: &mut Vec<CHIRStmt>,
) -> Result<(), CHIRLowerError> {
    let Some(init) = &local.init else { return Ok(()) };

    if let Some(s) = resolve_struct_literal(init, ctx)? {
        return lower_struct_binding(&local.name, &s, ctx, out);
    }

    // `let (a, b, …) = <tuple tree>` — one binding per element (see
    // `lower_tuple_binding`). The parser carries the binder list in the name.
    if local.name.starts_with('(') {
        return lower_tuple_binding(local, init, ctx, out);
    }

    // `let x = { …; tail };` — a block-valued binding (the CPU's forwarding
    // unit). Handled ahead of the conditional paths so inner `let`s get scoped.
    if let ExprType::Block(b) = init {
        return lower_block_binding(local, b, ctx, out);
    }

    // A struct-valued `if`/`match` binding: default-then-override. Declare every
    // `<name>_<field>` wire with a zero default, then lower the conditional as a
    // STATEMENT whose leaves assign the fields — which keeps arm-local `let`s
    // (an expression projection would drop them, `extract_block_expr_value`
    // takes only the tail).
    if matches!(init, ExprType::If(_) | ExprType::Match(_)) {
        if let Some(sname) = conditional_struct_name(init, ctx) {
            let fields = struct_fields(&sname, ctx, local.span)?;
            for (fname, fty) in &fields {
                let wire = format!("{}_{fname}", local.name);
                ctx.symbols.insert(wire.clone(), fty.clone());
                out.push(CHIRStmt::Wire {
                    name: wire,
                    ty: fty.clone(),
                    value: CHIRExpr::Lit(CHIRLit { ty: fty.clone(), value: 0 }),
                    span: local.span,
                });
            }
            ctx.struct_locals.insert(local.name.clone(), sname.clone());
            let rewritten = {
                let ctx_ref: &LowerCtx = ctx;
                let mut mk = |leaf: &ExprType, span: SourceSpan| {
                    struct_leaf_assigns(&local.name, &sname, &fields, leaf, ctx_ref, span)
                };
                rewrite_value_leaves(init, local.span, &mut mk)?
            };
            let mut stmts = Vec::new();
            lower_expr_stmt(&rewritten, local.span, ctx, &mut stmts)?;
            out.extend(stmts);
            return Ok(());
        }
    }

    let ty = match &local.ty {
        Some(t) => resolve_type(&t.ty_text, t.span)?,
        // A memory read result carries the memory's element type, which the
        // generic width inference below cannot see through the port handle.
        None if mem_result_type(init, ctx, local.span)?.is_some() => {
            mem_result_type(init, ctx, local.span)?.expect("just checked")
        }
        // Bottom-up inference first; if the width is ambiguous (a context-width
        // ctor like `Bits::zero()`), fall back to the type of the output port this
        // local is later written to (forward inference).
        None => match infer_type_from_expr(init, local.span, &ctx.symbols, &ctx.enums) {
            Ok(t) => t,
            Err(e) => fn_return_type(init, ctx)
                .or_else(|| ctx.write_inferred.get(&local.name).cloned())
                .ok_or(e)?,
        },
    };
    // Retype context-width default literals (e.g. `Bits::zero()`) to the binding's
    // declared width — including a symbolic parameter width (`Bits<N>` → `N'd0`) —
    // and width-cast a value whose width differs from the declared type.
    let tw = type_width(&ty);
    let value = retype_default_literals_in_values(lower_expr(init, ctx)?, tw.clone());
    let value = resize_to_target(value, &tw, ctx);
    ctx.symbols.insert(local.name.clone(), ty.clone());
    out.push(CHIRStmt::Wire {
        name: local.name.clone(),
        ty,
        value,
        span: local.span,
    });
    Ok(())
}

/// Retype a port write's default-width literals (`o.write(Bits::zero())`) to
/// the port's declared width — the same treatment an `Assign`'s RHS gets. Only
/// AMBIGUOUS default literals are rewritten, so a value with its own width is
/// untouched.
fn retype_port_write_value(port: &str, value: CHIRExpr, ctx: &LowerCtx) -> CHIRExpr {
    match ctx.symbols.get(port) {
        Some(ty) => retype_default_literals_in_values(value, type_width(ty)),
        None => value,
    }
}

/// The declared return type of a call to a registered file-scope fn
/// (`branch_taken(…)` → `bool`), for typing a binding the expression walk
/// cannot: the walk sees a Call, not the inlined body.
fn fn_return_type(init: &ExprType, ctx: &LowerCtx) -> Option<CHIRType> {
    let ExprType::Call(call) = init else { return None };
    let fn_ir = call_path(call).and_then(|n| ctx.fns.get(&n))?;
    let ret = fn_ir.signature.return_ty.as_ref()?;
    let text = compact_ident(&ret.ty_text);
    resolve_type(&text, ret.span).ok().or_else(|| {
        ctx.enums
            .get(&text)
            .map(|e| CHIRType::UInt { width: Width::Concrete(e.width) })
    })
}

/// Lower `let (a, b, …) = <tuple tree>` as one binding per element, by the same
/// default-then-override statement transform as a struct-valued conditional: a
/// zero-defaulted wire per (non-wildcard) element, then the tree in statement
/// position with every tuple-literal leaf turned into per-element assignments.
/// A struct-typed element recursively gets per-FIELD wires (`<elem>_<field>`)
/// and is registered as a struct local, so later field reads and whole-struct
/// copies of it work unchanged.
fn lower_tuple_binding(
    local: &LocalStmt,
    init: &ExprType,
    ctx: &mut LowerCtx,
    out: &mut Vec<CHIRStmt>,
) -> Result<(), CHIRLowerError> {
    let names: Vec<String> = local
        .name
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .map(|n| n.trim().to_string())
        .collect();

    // A call to a tuple-returning helper (`let (s, c) = full_adder(…)`, cause
    // J-b) inlines first (#7b), its body locals prefixed with the first binder
    // so they cannot capture a caller name.
    let inlined_init;
    let init = if let ExprType::Call(call) = init {
        match call_path(call).and_then(|n| ctx.fns.get(&n).cloned()) {
            Some(fn_ir) => {
                let mut e = build_inlined_expr(&fn_ir, &call.args, call.span)?;
                if let ExprType::Block(b) = &mut e {
                    let prefix = names
                        .iter()
                        .find(|n| *n != "_")
                        .cloned()
                        .unwrap_or_else(|| "tuple".to_string());
                    rename_block_locals(b, &prefix);
                }
                inlined_init = e;
                &inlined_init
            }
            None => init,
        }
    } else {
        init
    };

    // Sample leaves for per-element typing: the first leaf that yields a
    // definite answer wins (a `Bits::zero()` element in one arm has no width of
    // its own; another arm's `pc + imm` does).
    let mut leaves: Vec<&ExprType> = Vec::new();
    collect_value_leaves(init, &mut leaves);
    if leaves.is_empty() {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: "tuple binding needs a tuple literal or an `if`/`match` over tuple \
                          literals"
                .to_string(),
            span: local.span,
            suggested_rewrite: None,
        });
    }

    // element index → Some(struct name) when that element is struct-valued
    let mut elem_structs: Vec<Option<String>> = vec![None; names.len()];
    for (idx, name) in names.iter().enumerate() {
        if name == "_" {
            continue;
        }
        // Struct-valued element?
        let mut sname = None;
        for leaf in &leaves {
            let elem = project_tuple_element(leaf, idx, local.span)?;
            if let Some(n) = conditional_struct_name(&elem, ctx) {
                sname = Some(n);
                break;
            }
        }
        if let Some(sname) = sname {
            let fields = struct_fields(&sname, ctx, local.span)?;
            for (fname, fty) in &fields {
                let wire = format!("{name}_{fname}");
                ctx.symbols.insert(wire.clone(), fty.clone());
                out.push(CHIRStmt::Wire {
                    name: wire,
                    ty: fty.clone(),
                    value: CHIRExpr::Lit(CHIRLit { ty: fty.clone(), value: 0 }),
                    span: local.span,
                });
            }
            ctx.struct_locals.insert(name.clone(), sname.clone());
            elem_structs[idx] = Some(sname);
            continue;
        }
        // Scalar element: infer its type from the first leaf that knows it,
        // with arm-local `let`s in scope.
        let mut tree_symbols = ctx.symbols.clone();
        collect_tree_local_types(init, &mut tree_symbols, &ctx.enums);
        let mut ty = None;
        for leaf in &leaves {
            let elem = project_tuple_element(leaf, idx, local.span)?;
            if let Ok(t) = infer_type_from_expr(&elem, local.span, &tree_symbols, &ctx.enums) {
                ty = Some(t);
                break;
            }
        }
        let ty = match ty.or_else(|| ctx.write_inferred.get(name).cloned()) {
            Some(t) => t,
            None => return Err(CHIRLowerError::AmbiguousWidth { span: local.span }),
        };
        ctx.symbols.insert(name.clone(), ty.clone());
        out.push(CHIRStmt::Wire {
            name: name.clone(),
            ty: ty.clone(),
            value: CHIRExpr::Lit(CHIRLit { ty, value: 0 }),
            span: local.span,
        });
    }

    let rewritten = {
        let ctx_ref: &LowerCtx = ctx;
        let names_ref = &names;
        let elem_structs_ref = &elem_structs;
        let mut mk = |leaf: &ExprType, span: SourceSpan| -> Result<Vec<RawStmt>, CHIRLowerError> {
            let mut stmts = Vec::new();
            for (idx, name) in names_ref.iter().enumerate() {
                if name == "_" {
                    continue;
                }
                let elem = project_tuple_element(leaf, idx, span)?;
                match &elem_structs_ref[idx] {
                    Some(sname) => {
                        let fields = struct_fields(sname, ctx_ref, span)?;
                        stmts.extend(struct_leaf_assigns(
                            name, sname, &fields, &elem, ctx_ref, span,
                        )?);
                    }
                    None => stmts.push(assign_stmt(name, elem, span)),
                }
            }
            Ok(stmts)
        };
        rewrite_value_leaves(init, local.span, &mut mk)?
    };
    let mut stmts = Vec::new();
    lower_expr_stmt(&rewritten, local.span, ctx, &mut stmts)?;
    out.extend(stmts);
    Ok(())
}

/// Prefix every top-level `let` in a block with `prefix_`, substituting the
/// references — the scoping step for a block used as a value: two blocks may
/// both declare `hit`, and without the rename the second silently reads (or
/// redrives) the first's wire.
fn rename_block_locals(b: &mut copper_core::frontend_ir::ExprBlock, prefix: &str) {
    let mut subst: std::collections::HashMap<String, ExprType> = std::collections::HashMap::new();
    for stmt in &mut b.stmts {
        if let RawStmtKind::Local(l) = &mut stmt.kind {
            let renamed = format!("{prefix}_{}", l.name);
            subst.insert(
                l.name.clone(),
                ExprType::Path(copper_core::frontend_ir::ExprPath {
                    path_text: renamed.clone(),
                    span: l.span,
                }),
            );
            l.name = renamed;
        }
    }
    if !subst.is_empty() {
        subst_in_stmts(&mut b.stmts, &subst);
    }
}

/// Lower `let x = { …; tail };` — a block-valued binding. Inner `let`s become
/// wires prefixed with the binding's name (`x_<inner>` — the CPU's two
/// forwarding blocks both declare `from_ex_mem`, and without the prefix the
/// second declaration would silently shadow the first), and the tail is bound
/// by the same default-then-override transform as a conditional initializer,
/// so a conditional tail keeps its structure and the inner `let`s lower once.
fn lower_block_binding(
    local: &LocalStmt,
    block: &copper_core::frontend_ir::ExprBlock,
    ctx: &mut LowerCtx,
    out: &mut Vec<CHIRStmt>,
) -> Result<(), CHIRLowerError> {
    let mut b = block.clone();
    rename_block_locals(&mut b, &local.name);

    let tail = block_tail_expr(&b.stmts).cloned().ok_or_else(|| {
        CHIRLowerError::UnsupportedConstruct {
            description: format!(
                "block binding `{}` has no tail value expression",
                local.name
            ),
            span: local.span,
            suggested_rewrite: None,
        }
    })?;
    let init = ExprType::Block(b);

    // Struct-valued tail → per-field wires; otherwise one scalar wire.
    if let Some(sname) = conditional_struct_name(&tail, ctx) {
        let fields = struct_fields(&sname, ctx, local.span)?;
        for (fname, fty) in &fields {
            let wire = format!("{}_{fname}", local.name);
            ctx.symbols.insert(wire.clone(), fty.clone());
            out.push(CHIRStmt::Wire {
                name: wire,
                ty: fty.clone(),
                value: CHIRExpr::Lit(CHIRLit { ty: fty.clone(), value: 0 }),
                span: local.span,
            });
        }
        ctx.struct_locals.insert(local.name.clone(), sname.clone());
        let rewritten = {
            let ctx_ref: &LowerCtx = ctx;
            let mut mk = |leaf: &ExprType, span: SourceSpan| {
                struct_leaf_assigns(&local.name, &sname, &fields, leaf, ctx_ref, span)
            };
            rewrite_value_leaves(&init, local.span, &mut mk)?
        };
        let mut stmts = Vec::new();
        lower_expr_stmt(&rewritten, local.span, ctx, &mut stmts)?;
        out.extend(stmts);
        return Ok(());
    }

    let ty = match &local.ty {
        Some(t) => resolve_type(&t.ty_text, t.span)?,
        None => {
            let mut tree_symbols = ctx.symbols.clone();
            collect_tree_local_types(&init, &mut tree_symbols, &ctx.enums);
            let mut leaves = Vec::new();
            collect_value_leaves(&tail, &mut leaves);
            leaves
                .iter()
                .find_map(|leaf| {
                    infer_type_from_expr(leaf, local.span, &tree_symbols, &ctx.enums).ok()
                })
                .or_else(|| ctx.write_inferred.get(&local.name).cloned())
                .ok_or(CHIRLowerError::AmbiguousWidth { span: local.span })?
        }
    };
    ctx.symbols.insert(local.name.clone(), ty.clone());
    out.push(CHIRStmt::Wire {
        name: local.name.clone(),
        ty: ty.clone(),
        value: CHIRExpr::Lit(CHIRLit { ty, value: 0 }),
        span: local.span,
    });
    let rewritten = {
        let mut mk = |leaf: &ExprType, span: SourceSpan| {
            Ok(vec![assign_stmt(&local.name, leaf.clone(), span)])
        };
        rewrite_value_leaves(&init, local.span, &mut mk)?
    };
    let mut stmts = Vec::new();
    lower_expr_stmt(&rewritten, local.span, ctx, &mut stmts)?;
    out.extend(stmts);
    Ok(())
}

/// Fold the types of every `let` declared anywhere inside a value tree into
/// `symbols`, so a leaf that names an arm-local binding (`tgt` in the CPU's
/// JAL arm) can still be typed. Best-effort: a local whose init cannot be
/// typed is skipped.
fn collect_tree_local_types(expr: &ExprType, symbols: &mut SymbolTable, enums: &EnumRegistry) {
    fn scan_stmts(stmts: &[RawStmt], symbols: &mut SymbolTable, enums: &EnumRegistry) {
        for stmt in stmts {
            match &stmt.kind {
                RawStmtKind::Local(l) => {
                    let ty = match (&l.ty, &l.init) {
                        (Some(t), _) => resolve_type(&t.ty_text, t.span).ok(),
                        (None, Some(init)) => {
                            collect_tree_local_types(init, symbols, enums);
                            infer_type_from_expr(init, l.span, symbols, enums).ok()
                        }
                        (None, None) => None,
                    };
                    if let Some(ty) = ty {
                        symbols.insert(l.name.clone(), ty);
                    }
                }
                RawStmtKind::Expr(es) => collect_tree_local_types(&es.expr, symbols, enums),
                RawStmtKind::Item(_) => {}
            }
        }
    }
    match expr {
        ExprType::If(f) => {
            scan_stmts(&f.then_block, symbols, enums);
            if let Some(else_br) = &f.else_branch {
                collect_tree_local_types(else_br, symbols, enums);
            }
        }
        ExprType::Match(m) => {
            for arm in &m.arms {
                collect_tree_local_types(&arm.body, symbols, enums);
            }
        }
        ExprType::Block(b) => scan_stmts(&b.stmts, symbols, enums),
        _ => {}
    }
}

/// Collect the value LEAVES of a conditional tree (the non-`if`/`match`/block
/// expressions), in source order.
fn collect_value_leaves<'e>(expr: &'e ExprType, out: &mut Vec<&'e ExprType>) {
    match expr {
        ExprType::If(f) => {
            if let Some(tail) = block_tail_expr(&f.then_block) {
                collect_value_leaves(tail, out);
            }
            if let Some(else_br) = &f.else_branch {
                collect_value_leaves(else_br, out);
            }
        }
        ExprType::Match(m) => {
            for arm in &m.arms {
                collect_value_leaves(&arm.body, out);
            }
        }
        ExprType::Block(b) => {
            if let Some(tail) = block_tail_expr(&b.stmts) {
                collect_value_leaves(tail, out);
            }
        }
        leaf => out.push(leaf),
    }
}

/// If `init` denotes a struct value — a struct literal, or a call to a
/// file-scope fn that inlines to one (`alu_exec_reg(..)` → `AluOutput { .. }`) —
/// return that literal. Inlines one level to see through a call.
fn resolve_struct_literal(
    init: &ExprType,
    ctx: &LowerCtx,
) -> Result<Option<ExprStruct>, CHIRLowerError> {
    resolve_struct_literal_in(init, &ctx.fns, &ctx.structs)
}

/// Registry-based form of [`resolve_struct_literal`], usable before a
/// `LowerCtx` exists (the pre-loop register scan).
fn resolve_struct_literal_in(
    init: &ExprType,
    fns: &std::collections::HashMap<String, FrontendFnIR>,
    structs: &std::collections::HashMap<String, ItemStruct>,
) -> Result<Option<ExprStruct>, CHIRLowerError> {
    let inlined;
    // An associated ctor (`IFIDReg::bubble()`) usually builds `Self { … }`;
    // the type it means is the call path's prefix.
    let mut self_ty: Option<String> = None;
    let candidate: &ExprType = match init {
        ExprType::Call(call) => {
            let path = call_path(call);
            match path.as_deref().and_then(|n| fns.get(n).cloned()) {
                Some(fn_ir) => {
                    self_ty = path
                        .as_deref()
                        .and_then(|n| n.rsplit_once("::"))
                        .map(|(ty, _)| ty.to_string());
                    inlined = build_inlined_expr(&fn_ir, &call.args, call.span)?;
                    &inlined
                }
                None => init,
            }
        }
        _ => init,
    };

    match candidate {
        ExprType::Struct(st) => {
            let mut name = compact_ident(&st.path_text);
            if name == "Self" {
                match self_ty {
                    Some(ty) => name = ty,
                    None => return Ok(None),
                }
            }
            Ok(structs.contains_key(&name).then(|| {
                let mut lit = st.clone();
                lit.path_text = name;
                lit
            }))
        }
        _ => Ok(None),
    }
}

/// Bind a struct literal as one wire per field: `<base>_<field> = <field value>`.
fn lower_struct_binding(
    base: &str,
    s: &ExprStruct,
    ctx: &mut LowerCtx,
    out: &mut Vec<CHIRStmt>,
) -> Result<(), CHIRLowerError> {
    if s.rest.is_some() {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: "struct functional update (`..rest`) is not supported in hardware".to_string(),
            span: s.span,
            suggested_rewrite: None,
        });
    }
    let struct_name = compact_ident(&s.path_text);
    ctx.struct_locals.insert(base.to_string(), struct_name.clone());
    for field in &s.fields {
        let wire_name = format!("{base}_{}", field.member);
        let ty = resolve_field_type(&struct_name, &field.member, &field.expr, field.span, ctx)?;
        let value = lower_expr(&field.expr, ctx)?;
        ctx.symbols.insert(wire_name.clone(), ty.clone());
        out.push(CHIRStmt::Wire {
            name: wire_name,
            ty,
            value,
            span: field.span,
        });
    }
    Ok(())
}

/// The hardware type of struct field `field`: prefer the declared field type
/// (resolving `Bits`/primitives directly, or an enum name to its encoding
/// width), falling back to inferring from the field's value expression.
fn resolve_field_type(
    struct_name: &str,
    field: &str,
    value: &ExprType,
    span: SourceSpan,
    ctx: &LowerCtx,
) -> Result<CHIRType, CHIRLowerError> {
    if let Some(def) = ctx.structs.get(struct_name) {
        if let Some(f) = def.fields.iter().find(|f| f.name == field) {
            let text: String = f.ty.ty_text.chars().filter(|c| !c.is_whitespace()).collect();
            if let Ok(t) = resolve_type(&text, f.ty.span) {
                return Ok(t);
            }
            if let Some(enum_def) = ctx.enums.get(&text) {
                return Ok(CHIRType::UInt { width: Width::Concrete(enum_def.width) });
            }
        }
    }
    infer_type_from_expr(value, span, &ctx.symbols, &ctx.enums)
}

/// The one struct name every LEAF of a conditional tree resolves to — a struct
/// literal, a ctor call that inlines to one, or a local already known to be of
/// that struct type. `None` if any leaf is something else or the names differ.
fn conditional_struct_name(expr: &ExprType, ctx: &LowerCtx) -> Option<String> {
    match expr {
        ExprType::If(f) => {
            let then = conditional_struct_name(block_tail_expr(&f.then_block)?, ctx)?;
            let els = conditional_struct_name(f.else_branch.as_deref()?, ctx)?;
            (then == els).then_some(then)
        }
        ExprType::Match(m) => {
            let mut name: Option<String> = None;
            for arm in &m.arms {
                let n = conditional_struct_name(&arm.body, ctx)?;
                match &name {
                    None => name = Some(n),
                    Some(prev) if *prev == n => {}
                    _ => return None,
                }
            }
            name
        }
        ExprType::Block(b) => conditional_struct_name(block_tail_expr(&b.stmts)?, ctx),
        ExprType::Path(path) => {
            let ident = compact_ident(&path.path_text);
            ctx.struct_locals.get(&ident).cloned()
        }
        other => resolve_struct_literal(other, ctx)
            .ok()
            .flatten()
            .map(|s| compact_ident(&s.path_text)),
    }
}

fn compact_ident(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The last non-semicolon expression statement of a block — its value.
fn block_tail_expr(stmts: &[RawStmt]) -> Option<&ExprType> {
    stmts.iter().rev().find_map(|s| match &s.kind {
        RawStmtKind::Expr(es) if !es.has_semi => Some(&es.expr),
        _ => None,
    })
}

/// The declared, ordered field list of struct `name`: `(field, resolved type)`.
fn struct_fields(
    name: &str,
    ctx: &LowerCtx,
    span: SourceSpan,
) -> Result<Vec<(String, CHIRType)>, CHIRLowerError> {
    struct_fields_in(name, &ctx.structs, &ctx.enums, span)
}

fn struct_fields_in(
    name: &str,
    structs: &std::collections::HashMap<String, ItemStruct>,
    enums: &EnumRegistry,
    span: SourceSpan,
) -> Result<Vec<(String, CHIRType)>, CHIRLowerError> {
    let def = structs.get(name).ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
        description: format!("unknown struct `{name}`"),
        span,
        suggested_rewrite: None,
    })?;
    def.fields
        .iter()
        .map(|f| {
            let text = compact_ident(&f.ty.ty_text);
            let ty = match resolve_type(&text, f.ty.span) {
                Ok(t) => t,
                Err(e) => match enums.get(&text) {
                    Some(enum_def) => CHIRType::UInt { width: Width::Concrete(enum_def.width) },
                    None => return Err(e),
                },
            };
            Ok((f.name.clone(), ty))
        })
        .collect()
}

/// Rewrite a struct/tuple-VALUED conditional tree into STATEMENT position:
/// every value leaf becomes the assignment statements `mk(leaf)` produces,
/// while `if`/`match`/block structure — including arm-local `let`s, which an
/// expression projection would drop — survives intact. The result lowers
/// through the ordinary statement path (`lower_expr_stmt`), so together with
/// default-initialized wire declarations ahead of it this is the
/// default-then-override lowering of a conditional aggregate value.
fn rewrite_value_leaves(
    expr: &ExprType,
    span: SourceSpan,
    mk: &mut dyn FnMut(&ExprType, SourceSpan) -> Result<Vec<RawStmt>, CHIRLowerError>,
) -> Result<ExprType, CHIRLowerError> {
    match expr {
        ExprType::If(f) => {
            let mut new_if = f.clone();
            rewrite_block_tail(&mut new_if.then_block, f.span, mk)?;
            let else_br = f.else_branch.as_deref().ok_or_else(|| {
                CHIRLowerError::UnsupportedConstruct {
                    description: "aggregate-valued `if` needs an `else` branch".to_string(),
                    span: f.span,
                    suggested_rewrite: None,
                }
            })?;
            new_if.else_branch = Some(Box::new(rewrite_value_leaves(else_br, f.span, mk)?));
            Ok(ExprType::If(new_if))
        }
        ExprType::Match(m) => {
            let mut new_m = m.clone();
            for arm in &mut new_m.arms {
                arm.body = Box::new(rewrite_value_leaves(&arm.body, m.span, mk)?);
            }
            Ok(ExprType::Match(new_m))
        }
        ExprType::Block(b) => {
            let mut nb = b.clone();
            rewrite_block_tail(&mut nb.stmts, b.span, mk)?;
            Ok(ExprType::Block(nb))
        }
        leaf => {
            let stmts = mk(leaf, span)?;
            Ok(ExprType::Block(copper_core::frontend_ir::ExprBlock { stmts, span }))
        }
    }
}

/// Replace a block's tail value expression with its leaf rewrite, in place.
fn rewrite_block_tail(
    stmts: &mut Vec<RawStmt>,
    span: SourceSpan,
    mk: &mut dyn FnMut(&ExprType, SourceSpan) -> Result<Vec<RawStmt>, CHIRLowerError>,
) -> Result<(), CHIRLowerError> {
    let tail_idx = stmts
        .iter()
        .rposition(|s| matches!(&s.kind, RawStmtKind::Expr(es) if !es.has_semi))
        .ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
            description: "aggregate-valued branch has no tail value expression".to_string(),
            span,
            suggested_rewrite: None,
        })?;
    if let RawStmtKind::Expr(es) = &mut stmts[tail_idx].kind {
        es.expr = rewrite_value_leaves(&es.expr.clone(), es.span, mk)?;
        es.has_semi = true;
    }
    Ok(())
}

/// A synthesized `target = value;` statement, for the leaf rewrites above.
fn assign_stmt(target: &str, value: ExprType, span: SourceSpan) -> RawStmt {
    RawStmt {
        order: 0,
        kind: RawStmtKind::Expr(copper_core::frontend_ir::ExprStmt {
            expr: ExprType::Assign(copper_core::frontend_ir::ExprAssign {
                left: Box::new(ExprType::Path(copper_core::frontend_ir::ExprPath {
                    path_text: target.to_string(),
                    span,
                })),
                right: Box::new(value),
                span,
            }),
            has_semi: true,
            span,
        }),
        text: String::new(),
        span,
    }
}

/// The assignments that store one struct-valued LEAF into `base`'s per-field
/// wires/registers: a literal (or ctor) assigns each field's expression; a
/// struct-typed local assigns field accesses, which lower to its own
/// `<local>_<field>` nets.
fn struct_leaf_assigns(
    base: &str,
    struct_name: &str,
    fields: &[(String, CHIRType)],
    leaf: &ExprType,
    ctx: &LowerCtx,
    span: SourceSpan,
) -> Result<Vec<RawStmt>, CHIRLowerError> {
    if let Some(lit) = resolve_struct_literal(leaf, ctx)? {
        return fields
            .iter()
            .map(|(fname, _)| {
                let value = lit
                    .fields
                    .iter()
                    .find(|f| f.member == *fname)
                    .map(|f| (*f.expr).clone())
                    .ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
                        description: format!(
                            "struct literal `{}` is missing field `{fname}`",
                            lit.path_text
                        ),
                        span,
                        suggested_rewrite: None,
                    })?;
                Ok(assign_stmt(&format!("{base}_{fname}"), value, span))
            })
            .collect();
    }
    if let ExprType::Path(pth) = leaf {
        let ident = compact_ident(&pth.path_text);
        if ctx.struct_locals.get(&ident).map(String::as_str) == Some(struct_name) {
            return Ok(fields
                .iter()
                .map(|(fname, _)| {
                    let value = ExprType::Field(copper_core::frontend_ir::ExprField {
                        base: Box::new(leaf.clone()),
                        member: fname.clone(),
                        span,
                    });
                    assign_stmt(&format!("{base}_{fname}"), value, span)
                })
                .collect());
        }
    }
    Err(CHIRLowerError::UnsupportedConstruct {
        description: format!(
            "a `{struct_name}` value here must be a struct literal, a constructor call, \
             or a `{struct_name}`-typed local"
        ),
        span,
        suggested_rewrite: None,
    })
}

fn lower_stmt(
    stmt: &RawStmt,
    ctx: &mut LowerCtx,
    out: &mut Vec<CHIRStmt>,
) -> Result<(), CHIRLowerError> {
    match &stmt.kind {
        RawStmtKind::Local(local) => lower_local_binding(local, ctx, out)?,

        RawStmtKind::Expr(expr_stmt) => {
            lower_expr_stmt(&expr_stmt.expr, stmt.span, ctx, out)?;
        }

        RawStmtKind::Item(_) => {}
    }
    Ok(())
}

fn lower_expr_stmt(
    expr: &ExprType,
    span: SourceSpan,
    ctx: &mut LowerCtx,
    out: &mut Vec<CHIRStmt>,
) -> Result<(), CHIRLowerError> {
    match expr {
        // A `const { … }` block (e.g. `const { assert!(N == clog2(M)) }`) is a
        // compile-time check with no hardware meaning — Rust has already verified
        // it. Elide it.
        ExprType::Const(_) => {}

        // `for <var> in <start>..<end> { <body> }` → a SystemVerilog `for` loop
        // (Verilator unrolls it at elaboration, so `end` may be a parameter). The
        // loop variable is in scope for the body.
        ExprType::ForLoop(f) => {
            // A `clk.tick().await` inside a `for` is a counted delay: it needs a
            // cycle-counter register + a self-loop state (control extraction),
            // which is not built. Reject it rather than silently drop the delay
            // (the loop body would otherwise be treated as combinational).
            if stmts_contain_tick(&f.body) {
                return Err(CHIRLowerError::UnsupportedConstruct {
                    description: "clk.tick().await inside a for-loop (a counted delay) is not yet \
                                  supported — it needs control extraction (a cycle counter + \
                                  self-loop state), not unrolling".to_string(),
                    span,
                    suggested_rewrite: None,
                });
            }
            let var = f.pat_text.chars().filter(|c| !c.is_whitespace()).collect::<String>();
            if !is_ident(&var) {
                return Err(CHIRLowerError::UnsupportedConstruct {
                    description: format!("unsupported for-loop pattern `{}`; use a simple loop variable", f.pat_text),
                    span,
                    suggested_rewrite: None,
                });
            }
            let (start, end) = match &*f.iter {
                ExprType::Range(r) if !r.inclusive => {
                    let start = match &r.start {
                        Some(s) => lower_expr(s, ctx)?,
                        None => CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(32) }, value: 0 }),
                    };
                    let end = match &r.end {
                        Some(e) => lower_expr(e, ctx)?,
                        None => return Err(CHIRLowerError::UnsupportedConstruct {
                            description: "for-loop range needs an upper bound".to_string(),
                            span, suggested_rewrite: None,
                        }),
                    };
                    (start, end)
                }
                _ => return Err(CHIRLowerError::UnsupportedConstruct {
                    description: "only exclusive range `start..end` for-loops are supported".to_string(),
                    span,
                    suggested_rewrite: Some("rewrite the iterator as `a..b`".to_string()),
                }),
            };
            // The loop variable is an integer index, in scope for the body.
            ctx.symbols.insert(var.clone(), CHIRType::UInt { width: Width::Concrete(32) });
            let body = lower_stmts(&f.body, ctx)?;
            out.push(CHIRStmt::ForLoop { var, start, end, body, span });
        }

        ExprType::Await(await_expr) => {
            if is_tick_await(&await_expr.base) {
                out.push(CHIRStmt::AwaitTick {
                    clock: ctx.clock_name.clone(),
                    span,
                });
            } else {
                return Err(CHIRLowerError::UnsupportedConstruct {
                    description: "await on non-clock value".to_string(),
                    span,
                    suggested_rewrite: Some("use clk.tick().await to wait for a clock edge".to_string()),
                });
            }
        }

        // mem.write_port::<J>().write(addr, value) → MemWrite
        ExprType::MethodCall(mc) if mc.method == "write" && mc.args.len() == 2 => {
            let Some((mem, port)) = parse_mem_port(&mc.receiver, "write_port", ctx, span)? else {
                return Err(CHIRLowerError::UnsupportedConstruct {
                    description: "two-argument `.write()` on something that is not a memory \
                                  write port".to_string(),
                    span,
                    suggested_rewrite: None,
                });
            };
            let addr = lower_expr(&mc.args[0], ctx)?;
            let value = lower_expr(&mc.args[1], ctx)?;
            out.push(CHIRStmt::MemWrite { mem, port, addr, value, span });
        }

        // mem.read_port::<I>().read(addr) → MemRead. A port read (`in.read()`)
        // takes no argument, so the arity separates the two.
        ExprType::MethodCall(mc) if mc.method == "read" && mc.args.len() == 1 => {
            let Some((mem, port)) = parse_mem_port(&mc.receiver, "read_port", ctx, span)? else {
                return Err(CHIRLowerError::UnsupportedConstruct {
                    description: "one-argument `.read()` on something that is not a memory \
                                  read port".to_string(),
                    span,
                    suggested_rewrite: None,
                });
            };
            let addr = lower_expr(&mc.args[0], ctx)?;
            out.push(CHIRStmt::MemRead { mem, port, addr, span });
        }

        // port.write(value) → PortWrite
        ExprType::MethodCall(mc) if mc.method == "write" && mc.args.len() == 1 => {
            let port_name = match mc.receiver.as_ref() {
                ExprType::Lit(lit) => lit.text.trim().to_string(),
                ExprType::Path(p) => p.path_text.trim().to_string(),
                _ => return Err(CHIRLowerError::UnsupportedConstruct {
                    description: "port.write() receiver must be a simple port name".to_string(),
                    span,
                    suggested_rewrite: None,
                }),
            };
            let value = retype_port_write_value(&port_name, lower_expr(&mc.args[0], ctx)?, ctx);
            out.push(CHIRStmt::PortWrite { port_name, value, span });
        }

        ExprType::Assign(assign) => {
            // Tuple destructuring: `(a, b) = rhs` becomes one assignment per
            // element, each taking the corresponding projection of `rhs`.
            //
            // A Rust tuple assignment is *simultaneous*: the whole right-hand
            // side is evaluated before either target is updated. Emitting the
            // assignments directly would break that — Phase C forwards a
            // register's new value to later assignments in the same segment, so
            // `(phase, timer) = match (phase, …)` would compute `timer` from the
            // *new* phase. So every projection is first evaluated into a wire
            // (all of which see the pre-assignment values), and only then are the
            // registers assigned from those wires.
            if let ExprType::Tuple(lhs) = assign.left.as_ref() {
                let mut pending = Vec::new();
                for (idx, target_expr) in lhs.elements.iter().enumerate() {
                    let target = extract_assign_target(target_expr, span)?;
                    let projected = project_tuple_element(&assign.right, idx, span)?;
                    let value = lower_expr(&projected, ctx)?;
                    let ty = ctx.symbols.get(&target).cloned().ok_or_else(|| {
                        CHIRLowerError::UnsupportedConstruct {
                            description: format!(
                                "cannot determine the type of tuple assignment target '{target}'"
                            ),
                            span,
                            suggested_rewrite: None,
                        }
                    })?;
                    let value = retype_default_literals_in_values(value, type_width(&ty));
                    let tmp = format!("{target}_next_val");
                    ctx.symbols.insert(tmp.clone(), ty.clone());
                    out.push(CHIRStmt::Wire { name: tmp.clone(), ty, value, span });
                    pending.push((target, tmp));
                }
                for (target, tmp) in pending {
                    out.push(CHIRStmt::Assign { target, value: CHIRExpr::Var(tmp), span });
                }
                return Ok(());
            }
            // Whole-struct assignment: `ex_mem = new_ex_mem;` (or a struct
            // literal / ctor / conditional over them) expands to one assignment
            // per field. RHS field reads see pre-assignment values only when
            // they name OTHER locals' wires — a literal whose fields read the
            // target's own fields would see forwarded values; no such shape
            // exists in the corpus, and the plain-Var copy (the CPU latches) is
            // hazard-free by construction.
            if let ExprType::Path(pth) = assign.left.as_ref() {
                let target = compact_ident(&pth.path_text);
                if let Some(sname) = ctx.struct_locals.get(&target).cloned() {
                    let fields = struct_fields(&sname, ctx, span)?;
                    let rewritten = {
                        let ctx_ref: &LowerCtx = ctx;
                        let mut mk = |leaf: &ExprType, span: SourceSpan| {
                            struct_leaf_assigns(&target, &sname, &fields, leaf, ctx_ref, span)
                        };
                        rewrite_value_leaves(&assign.right, span, &mut mk)?
                    };
                    lower_expr_stmt(&rewritten, span, ctx, out)?;
                    return Ok(());
                }
            }
            // LHS bit-assign: `base[index] = value` drives a single bit of an
            // already-declared signal.
            if let ExprType::Index(idx) = assign.left.as_ref() {
                let base = extract_assign_target(&idx.base, span)?;
                let index = lower_expr(&idx.index, ctx)?;
                let value = lower_expr(&assign.right, ctx)?;
                out.push(CHIRStmt::IndexAssign { base, index, value, span });
                return Ok(());
            }
            let target = extract_assign_target(&assign.left, span)?;
            let mut value = lower_expr(&assign.right, ctx)?;
            // Propagate the target's width into untyped literals on the RHS. A
            // symbolic (parameter) width is fine — the literal becomes `N'd…`.
            // A value whose width differs from the target is width-cast.
            if let Some(tw) = ctx.symbols.get(&target).map(type_width) {
                value = retype_default_literals_in_values(value, tw.clone());
                value = resize_to_target(value, &tw, ctx);
            }
            out.push(CHIRStmt::Assign { target, value, span });
        }

        ExprType::Block(block) => {
            for stmt in &block.stmts {
                lower_stmt(stmt, ctx, out)?;
            }
        }

        ExprType::If(if_expr) => {
            let condition = lower_expr(&if_expr.condition, ctx)?;
            reject_tick_in_branch(&if_expr.then_block, span)?;
            let then_body = lower_stmts(&if_expr.then_block, ctx)?;
            let else_body = if_expr.else_branch.as_ref()
                .map(|br| lower_else_branch(br, span, ctx))
                .transpose()?;
            out.push(CHIRStmt::If { condition, then_body, else_body, span });
        }

        ExprType::Match(match_expr) => {
            let scrutinee = lower_expr(&match_expr.scrutinee, ctx)?;
            let mut arms = Vec::new();
            for arm in &match_expr.arms {
                let patterns = parse_or_patterns(&arm.pattern_text, span, &ctx.enums)?;
                let patterns = size_patterns_to_scrutinee(patterns, &scrutinee, span, ctx)?;
                let guard = arm.guard.as_ref().map(|g| lower_expr(g, ctx)).transpose()?;
                let body_stmts = match arm.body.as_ref() {
                    ExprType::Block(block) => lower_stmts(&block.stmts, ctx)?,
                    ExprType::If(_) | ExprType::Match(_) | ExprType::Assign(_) => {
                        let mut v = Vec::new();
                        lower_expr_stmt(&arm.body, span, ctx, &mut v)?;
                        v
                    }
                    ExprType::MethodCall(mc) if mc.method == "write" && mc.args.len() == 1 => {
                        let port_name = match mc.receiver.as_ref() {
                            ExprType::Lit(lit) => lit.text.trim().to_string(),
                            ExprType::Path(p) => p.path_text.trim().to_string(),
                            _ => return Err(CHIRLowerError::UnsupportedConstruct {
                                description: "port.write() receiver must be a simple port name".to_string(),
                                span,
                                suggested_rewrite: None,
                            }),
                        };
                        let value = retype_port_write_value(
                            &port_name,
                            lower_expr(&mc.args[0], ctx)?,
                            ctx,
                        );
                        vec![CHIRStmt::PortWrite { port_name, value, span }]
                    }
                    other => {
                        // Expression body — evaluate and discard (may have side effects via submodule calls)
                        let _ = lower_expr(other, ctx)?;
                        vec![]
                    }
                };
                arms.push(CHIRMatchArm { patterns, guard, body: body_stmts, span });
            }
            out.push(CHIRStmt::Match { scrutinee, arms, span });
        }

        // A `while` that CONTAINS a tick never reaches here — `desugar_tick_waits`
        // rewrites it into the repeating-wait `loop { if !cond { break; } … }`
        // shape before lowering. So this is always the combinational case: a loop
        // with no clock boundary, which has to be fully unrolled to be hardware
        // and therefore needs a trip count known at compile time. `while` only
        // implies that; `for` states it.
        ExprType::While(_) => {
            return Err(CHIRLowerError::UnsupportedConstruct {
                description: "a `while` loop with no `clk.tick().await` is combinational, so it must unroll — which needs a trip count known at compile time. Write it as a `for` over a constant range"
                    .to_string(),
                span,
                suggested_rewrite: Some(
                    "`for i in 0..N { ... }` with a constant `N` (a literal, a const item, or a                      const generic)"
                        .to_string(),
                ),
            });
        }

        // A `loop` nested inside the module's own top-level loop that control
        // extraction declined to flatten. Reaching CHIR at all means the flattener
        // passed on the whole module, so this is the module's *reported* blocker and
        // the message has to be followable — see `nested_loop_error`, which sorts
        // the three distinct reasons a nested loop lands here. Neither may be
        // silently dropped, which is what the generic expression fall-through used
        // to do to it.
        ExprType::Loop(l) => {
            return Err(nested_loop_error(l, span));
        }

        // Only meaningful inside a nested loop, which is rejected above — so
        // reaching here means one escaped its loop, and dropping it silently
        // would change the design's control flow.
        ExprType::Break(_) | ExprType::Continue(_) => {
            return Err(CHIRLowerError::UnsupportedConstruct {
                description: format!(
                    "`{}` is not supported in hardware; the module's top-level `loop` runs \
                     forever",
                    expr.kind_name()
                ),
                span,
                suggested_rewrite: None,
            });
        }

        ExprType::Lit(_) => {
            // Plain literal as statement — no effect
        }

        other => {
            let _ = lower_expr(other, ctx)?;
        }
    }

    Ok(())
}

fn lower_else_branch(
    branch: &ExprType,
    span: SourceSpan,
    ctx: &mut LowerCtx,
) -> Result<Vec<CHIRStmt>, CHIRLowerError> {
    match branch {
        ExprType::Block(block) => lower_stmts(&block.stmts, ctx),
        ExprType::If(if_expr) => {
            let mut out = Vec::new();
            lower_expr_stmt(&ExprType::If(if_expr.clone()), span, ctx, &mut out)?;
            Ok(out)
        }
        ExprType::Assign(a) => {
            let target = extract_assign_target(&a.left, span)?;
            let value = lower_expr(&a.right, ctx)?;
            Ok(vec![CHIRStmt::Assign { target, value, span }])
        }
        ExprType::MethodCall(mc) if mc.method == "write" && mc.args.len() == 1 => {
            let port_name = match mc.receiver.as_ref() {
                ExprType::Lit(lit) => lit.text.trim().to_string(),
                ExprType::Path(p) => p.path_text.trim().to_string(),
                _ => return Err(CHIRLowerError::UnsupportedConstruct {
                    description: "port.write() receiver must be a simple port name".to_string(),
                    span,
                    suggested_rewrite: None,
                }),
            };
            let value = retype_port_write_value(&port_name, lower_expr(&mc.args[0], ctx)?, ctx);
            Ok(vec![CHIRStmt::PortWrite { port_name, value, span }])
        }
        other => {
            let _ = lower_expr(other, ctx)?;
            Ok(vec![])
        }
    }
}

// ── Expression lowering ───────────────────────────────────────────────────────

pub(crate) fn lower_expr(expr: &ExprType, ctx: &mut LowerCtx) -> Result<CHIRExpr, CHIRLowerError> {
    match expr {
        ExprType::Lit(lit) => lower_name_or_lit(&lit.text, lit.span, ctx),
        ExprType::Path(path) => lower_name_or_lit(&path.path_text, path.span, ctx),

        ExprType::Binary(bin) => {
            let left = lower_expr(&bin.left, ctx)?;
            let right = lower_expr(&bin.right, ctx)?;
            let op = lower_binop(&bin.op, bin.span)?;
            // An untyped integer literal takes the width of the other operand,
            // so `timer < 1` compares at the register's width rather than the
            // default literal width (which would widen the whole expression).
            let (left, right) = balance_binop_literals(left, right, ctx);
            Ok(signed_binop(op, left, right))
        }

        ExprType::Unary(un) => {
            let inner = lower_expr(&un.expr, ctx)?;
            let op = lower_unop(&un.op, un.span)?;
            // Rust has no `~`: `!` IS bitwise Not on integers and `Bits<N>`
            // (`std::ops::Not`), and logical not only on `bool`. Emitting `!`
            // verbatim made `!mask` collapse to one bit (SystemVerilog LOGICAL
            // negation) — the `bit_not_bits` ledger entry, which reached
            // rv32i_cpu_pipelined's JALR alignment mask and zeroed every jump
            // target. Width decides: multi-bit → `~`; 1-bit/bool (or unknown
            // width) → `!`, which keeps `bit_not_bool` exact and is identical
            // for one bit anyway.
            let operand_is_boolean = matches!(&inner,
                CHIRExpr::BinOp { op, .. } if matches!(op,
                    CHIRBinOp::Eq | CHIRBinOp::Neq
                    | CHIRBinOp::Lt | CHIRBinOp::Lte | CHIRBinOp::Gt | CHIRBinOp::Gte
                    | CHIRBinOp::LogicalAnd | CHIRBinOp::LogicalOr));
            let op = if matches!(op, CHIRUnOp::LogicalNot)
                && !operand_is_boolean
                && width_of_chir_expr(&inner, ctx).is_some_and(|w| w > 1)
            {
                CHIRUnOp::BitNot
            } else {
                op
            };
            // Negation of a signed value is bit-identical (two's complement);
            // keep the wrapper on the RESULT so a downstream compare or shift
            // still sees signedness.
            if let CHIRExpr::SignCast { signed: true, expr } = inner {
                return Ok(CHIRExpr::SignCast {
                    signed: true,
                    expr: Box::new(CHIRExpr::UnOp { op, expr }),
                });
            }
            Ok(CHIRExpr::UnOp { op, expr: Box::new(inner) })
        }

        ExprType::If(if_expr) => {
            let cond = lower_expr(&if_expr.condition, ctx)?;
            let then_val = extract_block_expr_value(&if_expr.then_block, if_expr.span, ctx)?;
            let else_val = if_expr.else_branch.as_ref()
                .map(|br| lower_expr(br, ctx))
                .transpose()?
                .ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
                    description: "if-as-expression requires an else branch".to_string(),
                    span: if_expr.span,
                    suggested_rewrite: None,
                })?;
            // A mux's arms are one value at two spellings, so a default-width
            // literal arm takes the other arm's width exactly as a binop
            // operand does (`lit_width_in_ternary`'s `Bits::zero()` else-arm:
            // the `from_lit` arm carries a declared 32, the bare zero must not
            // stay 64 and WIDTHTRUNC into the port).
            let (then_val, else_val) = balance_binop_literals(then_val, else_val, ctx);
            Ok(CHIRExpr::Mux {
                cond: Box::new(cond),
                then_val: Box::new(then_val),
                else_val: Box::new(else_val),
            })
        }

        ExprType::Match(match_expr) => {
            let scrutinee = lower_expr(&match_expr.scrutinee, ctx)?;
            let mut arms = Vec::new();
            let mut default = None;
            // Arms with guards, binders, or partial wildcards cannot be a `case`
            // over a single concatenated selector — those lower to a condition
            // chain instead (see `lower_match_as_chain`).
            if !match_expr_is_case_compatible(match_expr, ctx)? {
                return lower_match_as_chain(match_expr, ctx);
            }
            for arm in &match_expr.arms {
                let patterns = parse_or_patterns(&arm.pattern_text, match_expr.span, &ctx.enums)?;
                let patterns =
                    size_patterns_to_scrutinee(patterns, &scrutinee, match_expr.span, ctx)?;
                let guard = arm.guard.as_ref().map(|g| lower_expr(g, ctx)).transpose()?;
                let value = lower_expr(&arm.body, ctx)?;
                // Wildcard with no guard → default arm
                if patterns.len() == 1 && matches!(patterns[0], CHIRPattern::Wildcard) && guard.is_none() {
                    default = Some(Box::new(value));
                } else {
                    // For or-patterns in case expression, emit one arm per pattern
                    for pattern in patterns {
                        arms.push(CHIRCaseArm { pattern, guard: guard.clone(), value: value.clone() });
                    }
                }
            }
            Ok(CHIRExpr::Case {
                scrutinee: Box::new(scrutinee),
                arms,
                default,
            })
        }

        ExprType::MethodCall(mc) => lower_method_call(mc, ctx),

        ExprType::Call(call) => {
            if call.is_hardware_module {
                lower_hardware_call(call, ctx)
            } else if call_path(call).as_deref().is_some_and(|n| ctx.fns.contains_key(n)) {
                // Call to a file-scope free function (#7b): inline it.
                lower_inlined_fn_call(call, ctx)
            } else if let Some(inner) = identity_pack_call(call) {
                // `Bits::from_array` / `from_slice` move no bits — lower the
                // argument in place of the call.
                let inner = inner.clone();
                lower_expr(&inner, ctx)
            } else if let Some(ctor) = call_path(call).as_deref().and_then(classify_value_ctor) {
                match ctor {
                    // `Bits::from_u32(x)` — the value is the argument,
                    // reinterpreted as a `width`-bit value.
                    ValueCtor::FromInt { width } => lower_bits_constructor(call, width, ctx),
                    // `Bits::from_lit::<1>()` / `Bits::zero()` — a fixed value.
                    // A type-position turbofish pins the width; otherwise the
                    // default lets the surrounding assignment or operand decide.
                    ValueCtor::Const { value, width } => Ok(CHIRExpr::Lit(CHIRLit {
                        ty: CHIRType::UInt {
                            width: Width::Concrete(width.unwrap_or(DEFAULT_LIT_WIDTH)),
                        },
                        value,
                    })),
                }
            } else {
                Err(CHIRLowerError::UnsupportedConstruct {
                    description: "non-hardware function calls cannot appear in hardware expressions; add #[hardware]".to_string(),
                    span: call.span,
                    suggested_rewrite: Some("annotate the function with #[hardware]".to_string()),
                })
            }
        }

        ExprType::Cast(cast) => {
            // Width changes are handled at VLIR emission; what a cast DOES carry
            // is signedness. `as i*` wraps the operand in `SignCast { signed:
            // true }` so a comparison or right shift downstream is emitted
            // signed (`$signed`, `>>>`) — before this, the cast was stripped
            // outright and `(a as i32) < (b as i32)` compiled to an UNSIGNED
            // compare while `as i32 >> 20` compiled to a LOGICAL shift, both
            // lint-clean and wrong (the signedness claim-ledger entries).
            // `as u*` of a signed expression re-interprets back (`$unsigned`);
            // `as u*` of a plain expression stays a strip, as before.
            let inner = lower_expr(&cast.expr, ctx)?;
            let target = compact_type(&cast.target_ty.ty_text);
            let to_signed = matches!(
                target.as_str(),
                "i8" | "i16" | "i32" | "i64" | "i128" | "isize"
            );
            Ok(match (to_signed, inner) {
                (true, CHIRExpr::SignCast { expr, .. }) => {
                    CHIRExpr::SignCast { signed: true, expr }
                }
                (true, plain) => {
                    CHIRExpr::SignCast { signed: true, expr: Box::new(plain) }
                }
                // `as u*` of a SIGNED expression re-interprets it back — the
                // explicit `$unsigned` keeps the emitted type honest.
                (false, CHIRExpr::SignCast { expr, .. }) => {
                    CHIRExpr::SignCast { signed: false, expr }
                }
                // `as u*`/`as usize` of a plain expression is the historical
                // strip: signedness-free, width handled at VLIR emission.
                (false, plain) => plain,
            })
        }

        ExprType::Reference(r) => {
            lower_expr(&r.expr, ctx)
        }

        ExprType::Field(f) => {
            let base = lower_expr(&f.base, ctx)?;
            match base {
                CHIRExpr::Var(name) => Ok(CHIRExpr::Var(format!("{}_{}", name, f.member))),
                _ => Err(CHIRLowerError::UnsupportedConstruct {
                    description: "field access on complex expression not supported in hardware".to_string(),
                    span: f.span,
                    suggested_rewrite: None,
                }),
            }
        }

        ExprType::Index(idx) => lower_index(idx, ctx),

        // A tuple value (e.g. a `match (a, b)` scrutinee) is the bit
        // concatenation of its elements, first element most-significant —
        // matching how tuple *patterns* are encoded in Phase D.
        ExprType::Tuple(t) => {
            let parts = t
                .elements
                .iter()
                .map(|e| lower_expr(e, ctx))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CHIRExpr::Concat(parts))
        }

        // `[Logic::Zero; N]` — the packed vector its declared type describes
        // (see the matching arm in `infer_type_from_expr`).
        ExprType::Repeat(rep) => lower_repeat(rep, ctx),

        // A block used as an expression (e.g. an `if`/`else` branch) evaluates to
        // its tail expression.
        ExprType::Block(b) => extract_block_expr_value(&b.stmts, b.span, ctx),

        other => Err(CHIRLowerError::UnsupportedConstruct {
            description: format!("{} is not supported in hardware", other.kind_name()),
            span: other.span(),
            suggested_rewrite: None,
        }),
    }
}

/// Lower a `Bits`-style value constructor (`Bits::from_u32(x)`): the result is
/// its single argument reinterpreted as a `width`-bit value. A bare literal
/// argument adopts the constructor width so downstream widths stay exact.
fn lower_bits_constructor(
    call: &ExprCall,
    width: usize,
    ctx: &mut LowerCtx,
) -> Result<CHIRExpr, CHIRLowerError> {
    let arg = call.args.first().ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
        description: "value constructor requires one argument".to_string(),
        span: call.span,
        suggested_rewrite: None,
    })?;
    let lowered = lower_expr(arg, ctx)?;
    // The argument is a `width`-bit value (Rust infers its literals as the
    // constructor's source type, e.g. `u32`), so retype its literals to match.
    Ok(retype_literals(lowered, width))
}

/// Set every bare literal in an arithmetic/bitwise expression tree to `width`
/// bits. Used to give a constructor argument (`Bits::from_u32(1 << 31 | …)`) the
/// constructor's width so it does not overflow into the default literal width.
fn retype_literals(expr: CHIRExpr, width: usize) -> CHIRExpr {
    match expr {
        CHIRExpr::Lit(lit) => CHIRExpr::Lit(CHIRLit {
            ty: CHIRType::UInt { width: Width::Concrete(width) },
            value: lit.value,
        }),
        CHIRExpr::BinOp { left, op, right } => CHIRExpr::BinOp {
            left: Box::new(retype_literals(*left, width)),
            op,
            right: Box::new(retype_literals(*right, width)),
        },
        CHIRExpr::UnOp { op, expr } => CHIRExpr::UnOp {
            op,
            expr: Box::new(retype_literals(*expr, width)),
        },
        // Named signals and structural ops already carry their own widths.
        other => other,
    }
}

// ── Match-arm pattern elements (B2: bindings / guards / partial wildcards) ────

/// One element of a match arm's pattern, positioned against the scrutinee.
/// A tuple pattern yields one per tuple position; a scalar pattern yields one.
#[derive(Debug, Clone)]
enum PatElem {
    /// A concrete value — contributes an equality test on that position.
    Lit(CHIRLit),
    /// `_` — matches anything, contributes no condition.
    Wildcard,
    /// A binder (`t`) — contributes no condition, but names that position so the
    /// arm's guard and body can refer to it.
    Bind(String),
    /// A named constant (`localparam` / parameter) — contributes an equality
    /// test against the name; SystemVerilog evaluates it.
    Const(String),
}

/// Parse one pattern alternative into positional elements.
fn parse_pattern_elems(
    text: &str,
    span: SourceSpan,
    enums: &EnumRegistry,
    params: &std::collections::HashSet<String>,
) -> Result<Vec<PatElem>, CHIRLowerError> {
    let s = text.trim();
    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len() - 1];
        return split_top_level_commas(inner)
            .iter()
            .map(|p| parse_pattern_elem(p.trim(), span, enums, params))
            .collect();
    }
    Ok(vec![parse_pattern_elem(s, span, enums, params)?])
}

fn parse_pattern_elem(
    s: &str,
    span: SourceSpan,
    enums: &EnumRegistry,
    params: &std::collections::HashSet<String>,
) -> Result<PatElem, CHIRLowerError> {
    if s == "_" {
        return Ok(PatElem::Wildcard);
    }
    // Reuse the scalar pattern parser for literals / enum paths / `Logic::*`.
    match parse_pattern(s, span, enums)? {
        CHIRPattern::Lit(lit) => return Ok(PatElem::Lit(lit)),
        // A const/parameter name before enum-variant resolution: it is a value,
        // not a variant.
        CHIRPattern::EnumVariant { name, inner: None } if params.contains(&name) => {
            return Ok(PatElem::Const(name));
        }
        CHIRPattern::EnumVariant { name, .. } => {
            // A bare variant name (no enum prefix): resolve it if exactly one
            // enum declares it.
            let mut found = None;
            for def in enums.values() {
                if let Some(v) = def.variants.get(&name) {
                    if found.is_some() {
                        found = None;
                        break;
                    }
                    found = Some(CHIRLit {
                        ty: CHIRType::UInt { width: Width::Concrete(def.width) },
                        value: *v,
                    });
                }
            }
            if let Some(lit) = found {
                return Ok(PatElem::Lit(lit));
            }
        }
        _ => {}
    }
    // A lowercase identifier in pattern position is a binder.
    if is_ident(s) && s.chars().next().map(|c| c.is_lowercase()).unwrap_or(false) {
        return Ok(PatElem::Bind(s.to_string()));
    }
    Ok(PatElem::Wildcard)
}

/// Split a pattern on top-level `|` alternatives.
fn split_top_level_pipes(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '|' if depth == 0 => {
                out.push(s[start..i].to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].to_string());
    out
}

/// The scrutinee's positional element expressions (a tuple matches positionally).
fn scrutinee_elements(scrutinee: &ExprType) -> Vec<&ExprType> {
    match scrutinee {
        ExprType::Tuple(t) => t.elements.iter().collect(),
        other => vec![other],
    }
}

/// True when every arm can be a `case` label: no guards, and each alternative is
/// either all-literal (a concrete selector value) or a whole-pattern wildcard
/// (the `default`). Binders and partial wildcards disqualify it.
fn match_expr_is_case_compatible(
    m: &copper_core::frontend_ir::ExprMatch,
    ctx: &LowerCtx,
) -> Result<bool, CHIRLowerError> {
    for arm in &m.arms {
        if arm.guard.is_some() {
            return Ok(false);
        }
        for alt in split_top_level_pipes(&arm.pattern_text) {
            let elems = parse_pattern_elems(&alt, m.span, &ctx.enums, &ctx.params)?;
            let all_lit = elems
                .iter()
                .all(|e| matches!(e, PatElem::Lit(_) | PatElem::Const(_)));
            let whole_wildcard = elems.len() == 1 && matches!(elems[0], PatElem::Wildcard);
            if !all_lit && !whole_wildcard {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Lower a match to a priority chain of muxes: each arm contributes a condition
/// built from its *literal* positions (wildcards constrain nothing), plus its
/// guard; binders name scrutinee elements for use in the guard and body.
///
/// This is the general form — it handles guards, pattern bindings, and partial
/// wildcards, which a concatenated-selector `case` cannot express.
fn lower_match_as_chain(
    m: &copper_core::frontend_ir::ExprMatch,
    ctx: &mut LowerCtx,
) -> Result<CHIRExpr, CHIRLowerError> {
    let span = m.span;
    let scrut_elems = scrutinee_elements(&m.scrutinee);
    let mut result: Option<CHIRExpr> = None;

    // Build back-to-front so earlier arms take priority.
    for arm in m.arms.iter().rev() {
        let alts = split_top_level_pipes(&arm.pattern_text);
        let mut alt_conds: Vec<Option<CHIRExpr>> = Vec::new();
        let mut bindings: std::collections::HashMap<String, CHIRExpr> =
            std::collections::HashMap::new();
        let mut has_binder = false;

        for alt in &alts {
            let elems = parse_pattern_elems(alt, span, &ctx.enums, &ctx.params)?;
            // A lone `_` matches the whole scrutinee regardless of its arity
            // (`match (a, b) { _ => .. }`), so it is unconditional. Only when the
            // pattern is a tuple must its arity match the scrutinee's.
            let whole_wildcard = elems.len() == 1 && matches!(elems[0], PatElem::Wildcard);
            if !whole_wildcard && elems.len() != scrut_elems.len() {
                return Err(CHIRLowerError::UnsupportedConstruct {
                    description: format!(
                        "match arm pattern has {} element(s) but the scrutinee has {}",
                        elems.len(),
                        scrut_elems.len()
                    ),
                    span,
                    suggested_rewrite: None,
                });
            }
            let mut alt_cond: Option<CHIRExpr> = None;
            for (i, e) in elems.iter().enumerate() {
                if whole_wildcard {
                    break; // unconditional; no per-element conditions
                }
                match e {
                    PatElem::Lit(lit) => {
                        let sc = lower_expr(scrut_elems[i], ctx)?;
                        alt_cond = and_cond(
                            alt_cond,
                            CHIRExpr::BinOp {
                                left: Box::new(sc),
                                op: CHIRBinOp::Eq,
                                right: Box::new(CHIRExpr::Lit(lit.clone())),
                            },
                        );
                    }
                    PatElem::Const(name) => {
                        let sc = lower_expr(scrut_elems[i], ctx)?;
                        alt_cond = and_cond(
                            alt_cond,
                            CHIRExpr::BinOp {
                                left: Box::new(sc),
                                op: CHIRBinOp::Eq,
                                right: Box::new(CHIRExpr::Var(name.clone())),
                            },
                        );
                    }
                    PatElem::Wildcard => {}
                    PatElem::Bind(name) => {
                        has_binder = true;
                        let sc = lower_expr(scrut_elems[i], ctx)?;
                        bindings.insert(name.clone(), sc);
                    }
                }
            }
            alt_conds.push(alt_cond);
        }

        if alts.len() > 1 && has_binder {
            return Err(CHIRLowerError::UnsupportedConstruct {
                description: "or-patterns combined with a pattern binding are not supported"
                    .to_string(),
                span,
                suggested_rewrite: Some("split the alternatives into separate arms".to_string()),
            });
        }

        // An unconditional alternative makes the whole arm unconditional;
        // otherwise OR the alternatives together.
        let arm_cond = if alt_conds.iter().any(|c| c.is_none()) {
            None
        } else {
            alt_conds.into_iter().flatten().reduce(|a, b| CHIRExpr::BinOp {
                left: Box::new(a),
                op: CHIRBinOp::LogicalOr,
                right: Box::new(b),
            })
        };

        // Lower the guard and body with this arm's bindings in scope.
        let saved = std::mem::replace(&mut ctx.bindings, bindings);
        let guard = arm.guard.as_ref().map(|g| lower_expr(g, ctx)).transpose()?;
        let value = lower_expr(&arm.body, ctx)?;
        ctx.bindings = saved;

        let full_cond = match (arm_cond, guard) {
            (None, None) => None,
            (Some(c), None) => Some(c),
            (None, Some(g)) => Some(g),
            (Some(c), Some(g)) => Some(CHIRExpr::BinOp {
                left: Box::new(c),
                op: CHIRBinOp::LogicalAnd,
                right: Box::new(g),
            }),
        };

        result = Some(match (result.take(), full_cond) {
            // The last arm (processed first) is the fallback. Rust has already
            // proven the match exhaustive, so if no earlier arm matches this one
            // must — its pattern condition is implied and can be dropped. That
            // is what lets an enum-exhaustive match with no `_` arm work.
            (None, _) => {
                if arm.guard.is_some() {
                    return Err(CHIRLowerError::UnsupportedConstruct {
                        description: "the final match arm is guarded, so no branch is \
                                      guaranteed to produce a value"
                            .to_string(),
                        span,
                        suggested_rewrite: Some("add an unguarded `_ => …` arm".to_string()),
                    });
                }
                value
            }
            // An unconditional arm replaces the fallback (later arms are dead).
            (Some(_), None) => value,
            (Some(prev), Some(cond)) => CHIRExpr::Mux {
                cond: Box::new(cond),
                then_val: Box::new(value),
                else_val: Box::new(prev),
            },
        });
    }

    result.ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
        description: "match has no arms".to_string(),
        span,
        suggested_rewrite: None,
    })
}

/// The width an untyped integer literal falls back to when nothing constrains it.
/// `[elem; len]` as a packed vector.
///
/// Repeating a **zero** gives a zero whatever the length is, so it is emitted the
/// way `Bits::zero()` is — a context-width literal the surrounding declaration
/// sizes. That is what lets `[Logic::Zero; N]` work for a symbolic `N`, which the
/// concatenation form below cannot express.
///
/// Any other repeated element needs the length spelled out and becomes an
/// explicit concatenation.
fn lower_repeat(rep: &ExprRepeat, ctx: &mut LowerCtx) -> Result<CHIRExpr, CHIRLowerError> {
    let elem = lower_expr(&rep.expr, ctx)?;

    if matches!(&elem, CHIRExpr::Lit(l) if l.value == 0) {
        return Ok(CHIRExpr::Lit(CHIRLit {
            ty: CHIRType::UInt { width: Width::Concrete(DEFAULT_LIT_WIDTH) },
            value: 0,
        }));
    }

    match repeat_len(&rep.len) {
        Some(Width::Concrete(n)) => Ok(CHIRExpr::Concat(vec![elem; n])),
        _ => Err(CHIRLowerError::UnsupportedConstruct {
            description: "a repeated array with a symbolic length must repeat a zero \
                          element (`[Logic::Zero; N]`); give the length a concrete value \
                          to repeat anything else"
                .to_string(),
            span: rep.span,
            suggested_rewrite: None,
        }),
    }
}

const DEFAULT_LIT_WIDTH: usize = 64;

/// Give a bare (default-width) integer literal operand the width of the other
/// side of a binary operation. Without this, `timer < 1` on an 8-bit register
/// compares at 64 bits and Verilator reports a width-expansion warning.
fn balance_binop_literals(left: CHIRExpr, right: CHIRExpr, ctx: &LowerCtx) -> (CHIRExpr, CHIRExpr) {
    let is_default_lit = |e: &CHIRExpr| {
        matches!(e, CHIRExpr::Lit(l)
            if l.ty == CHIRType::UInt { width: Width::Concrete(DEFAULT_LIT_WIDTH) })
    };
    let other_width = |e: &CHIRExpr| {
        width_of_chir_expr(e, ctx).filter(|w| *w != DEFAULT_LIT_WIDTH)
    };

    if is_default_lit(&right) && !is_default_lit(&left) {
        if let Some(w) = other_width(&left) {
            return (left, retype_literals(right, w));
        }
    }
    if is_default_lit(&left) && !is_default_lit(&right) {
        if let Some(w) = other_width(&right) {
            return (retype_literals(left, w), right);
        }
    }
    (left, right)
}

/// Retype untyped (default-width) literals sitting in *value* positions to
/// `width`, propagating an assignment target's width into its right-hand side.
///
/// Conditions and match scrutinees are deliberately left alone: in
/// `phase == 2'd0 ? 0 : …` the `2'd0` is comparing a 2-bit register and must keep
/// its own width, while the `0` result belongs to the assignment target.
/// The declared width of a type as a `Width` (concrete or symbolic parameter).
fn type_width(ty: &CHIRType) -> Width {
    match ty {
        CHIRType::UInt { width } | CHIRType::SInt { width } => width.clone(),
        CHIRType::Bool => Width::Concrete(1),
        // Element width — see `width_of_type`.
        CHIRType::Array { elem, .. } => type_width(elem),
    }
}

/// The `Width` of a lowered expression when it can be read off directly (a signal
/// reference or a literal); `None` for anything whose width needs inference.
fn expr_width(e: &CHIRExpr, ctx: &LowerCtx) -> Option<Width> {
    match e {
        CHIRExpr::Var(name) => ctx.symbols.get(name).map(type_width),
        CHIRExpr::Lit(l) => Some(type_width(&l.ty)),
        CHIRExpr::Slice { high, low, .. } => Some(Width::Concrete(high - low + 1)),
        CHIRExpr::DynBit { .. } => Some(Width::Concrete(1)),
        CHIRExpr::Resize { width, .. } => Some(width.clone()),
        _ => None,
    }
}

/// Wrap `value` in a width-cast to `target` when its width is known and differs,
/// so an assignment that mixes concrete and parameter widths stays lint-clean
/// (`res = N_LOG'(i)`). A no-op when the widths match or the value width is
/// unknown (that path already width-matches or is inferred elsewhere).
fn resize_to_target(value: CHIRExpr, target: &Width, ctx: &LowerCtx) -> CHIRExpr {
    match expr_width(&value, ctx) {
        Some(w) if &w != target => CHIRExpr::Resize {
            expr: Box::new(value),
            width: target.clone(),
        },
        _ => value,
    }
}

fn retype_default_literals_in_values(e: CHIRExpr, width: Width) -> CHIRExpr {
    let recurse = |x: Box<CHIRExpr>| Box::new(retype_default_literals_in_values(*x, width.clone()));
    match e {
        CHIRExpr::Lit(l)
            if l.ty == CHIRType::UInt { width: Width::Concrete(DEFAULT_LIT_WIDTH) } =>
        {
            CHIRExpr::Lit(CHIRLit {
                ty: CHIRType::UInt { width: width.clone() },
                value: l.value,
            })
        }
        CHIRExpr::Mux { cond, then_val, else_val } => CHIRExpr::Mux {
            cond, // a condition, not a value
            then_val: recurse(then_val),
            else_val: recurse(else_val),
        },
        CHIRExpr::Case { scrutinee, arms, default } => CHIRExpr::Case {
            scrutinee, // a selector, not a value
            arms: arms
                .into_iter()
                .map(|a| CHIRCaseArm {
                    pattern: a.pattern,
                    guard: a.guard,
                    value: retype_default_literals_in_values(a.value, width.clone()),
                })
                .collect(),
            default: default.map(recurse),
        },
        CHIRExpr::BinOp { left, op, right } => CHIRExpr::BinOp {
            left: recurse(left),
            op,
            right: recurse(right),
        },
        other => other,
    }
}

/// Best-effort width of an already-lowered expression, resolved against the
/// in-scope signal types. Measuring the *lowered* form matters because pattern
/// bindings have been substituted by then (`t` → `timer`).
fn width_of_chir_expr(e: &CHIRExpr, ctx: &LowerCtx) -> Option<usize> {
    match e {
        CHIRExpr::Var(name) => ctx
            .symbols
            .get(name)
            .and_then(width_of_type_concrete)
            // Not a signal: a `parameter int` / `localparam int` is 32 bits.
            .or_else(|| ctx.params.contains(name).then_some(32)),
        CHIRExpr::Lit(l) => width_of_type_concrete(&l.ty),
        CHIRExpr::BinOp { left, right, .. } => {
            width_of_chir_expr(left, ctx).or_else(|| width_of_chir_expr(right, ctx))
        }
        CHIRExpr::UnOp { expr, .. } => width_of_chir_expr(expr, ctx),
        CHIRExpr::Mux { then_val, else_val, .. } => {
            width_of_chir_expr(then_val, ctx).or_else(|| width_of_chir_expr(else_val, ctx))
        }
        CHIRExpr::Slice { high, low, .. } => Some(high - low + 1),
        // A signedness reinterpretation keeps its operand's width.
        CHIRExpr::SignCast { expr, .. } => width_of_chir_expr(expr, ctx),
        // A memory read result: the element width lives in the memory's
        // declaration, which this expression-only walk does not carry.
        CHIRExpr::MemData { mem, .. } => ctx.memories.get(mem).map(|m| width_of_type(&m.elem_ty)),
        CHIRExpr::MemValid { .. } => Some(1),
        CHIRExpr::DynBit { .. } => Some(1),
        CHIRExpr::Resize { width, .. } => match width {
            Width::Concrete(n) => Some(*n),
            Width::Param(_) => None,
        },
        CHIRExpr::Concat(parts) => {
            parts.iter().map(|p| width_of_chir_expr(p, ctx)).sum::<Option<usize>>()
        }
        CHIRExpr::Case { arms, default, .. } => arms
            .first()
            .and_then(|a| width_of_chir_expr(&a.value, ctx))
            .or_else(|| default.as_ref().and_then(|d| width_of_chir_expr(d, ctx))),
    }
}

/// Combine two optional conditions with `&&`.
fn and_cond(acc: Option<CHIRExpr>, next: CHIRExpr) -> Option<CHIRExpr> {
    Some(match acc {
        None => next,
        Some(prev) => CHIRExpr::BinOp {
            left: Box::new(prev),
            op: CHIRBinOp::LogicalAnd,
            right: Box::new(next),
        },
    })
}

/// Project element `idx` out of a tuple-valued expression, for lowering a tuple
/// destructuring assignment into one assignment per element.
///
/// A tuple literal projects directly; `match`/`if` are pushed through so each
/// element gets its own conditional expression (e.g.
/// `(a, b) = match s { p => (x, y) }` → `a = match s { p => x }`,
/// `b = match s { p => y }`).
fn project_tuple_element(
    expr: &ExprType,
    idx: usize,
    span: SourceSpan,
) -> Result<ExprType, CHIRLowerError> {
    match expr {
        ExprType::Tuple(t) => t.elements.get(idx).cloned().ok_or_else(|| {
            CHIRLowerError::UnsupportedConstruct {
                description: format!(
                    "tuple has {} elements; cannot project element {}",
                    t.elements.len(),
                    idx
                ),
                span,
                suggested_rewrite: None,
            }
        }),
        ExprType::Match(m) => {
            let mut projected = m.clone();
            for arm in &mut projected.arms {
                let inner = project_tuple_element(&arm.body, idx, span)?;
                arm.body = Box::new(inner);
            }
            Ok(ExprType::Match(projected))
        }
        ExprType::If(f) => {
            let mut projected = f.clone();
            if let Some(else_br) = &f.else_branch {
                let inner = project_tuple_element(else_br, idx, span)?;
                projected.else_branch = Some(Box::new(inner));
            }
            Ok(ExprType::If(projected))
        }
        ExprType::Block(b) => {
            // Project the block's tail expression.
            let tail = b
                .stmts
                .iter()
                .rev()
                .find_map(|s| match &s.kind {
                    RawStmtKind::Expr(es) if !es.has_semi => Some(&es.expr),
                    _ => None,
                })
                .ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
                    description: "block used as a tuple value has no tail expression".to_string(),
                    span,
                    suggested_rewrite: None,
                })?;
            project_tuple_element(tail, idx, span)
        }
        other => Err(CHIRLowerError::UnsupportedConstruct {
            description: format!(
                "cannot destructure element {idx} from this expression: {:?}",
                std::mem::discriminant(other)
            ),
            span,
            suggested_rewrite: Some(
                "assign each element separately, or return a tuple literal / match on one".to_string(),
            ),
        }),
    }
}

/// Lower a bit-index `base[i]` to a single-bit slice. The index must be a
/// compile-time constant (variable indices require loop unrolling, which is a
/// separate lowering step).
fn lower_index(idx: &ExprIndex, ctx: &mut LowerCtx) -> Result<CHIRExpr, CHIRLowerError> {
    let base = lower_expr(&idx.base, ctx)?;
    // A compile-time constant index is a static one-bit `Slice`; anything else
    // (e.g. a loop variable) is a dynamic single-bit select `base[index]`.
    match eval_const_usize(&idx.index) {
        Some(bit) => Ok(CHIRExpr::Slice { expr: Box::new(base), high: bit, low: bit }),
        None => {
            let index = lower_expr(&idx.index, ctx)?;
            Ok(CHIRExpr::DynBit { base: Box::new(base), index: Box::new(index) })
        }
    }
}

/// Evaluate an expression to a constant `usize`, for use as a bit index.
/// Currently only integer literals; extended to loop-variable substitution when
/// `for`-loop unrolling lands.
fn eval_const_usize(expr: &ExprType) -> Option<usize> {
    match expr {
        ExprType::Lit(lit) => {
            let compact: String = lit.text.chars().filter(|c| !c.is_whitespace()).collect();
            parse_int_literal(&compact).map(|(v, _)| v as usize)
        }
        _ => None,
    }
}

/// Lower an identifier-or-literal, resolving match-arm pattern bindings first so
/// a binder (`t`) becomes the scrutinee element it names rather than a dangling
/// signal reference.
fn lower_name_or_lit(
    text: &str,
    span: SourceSpan,
    ctx: &LowerCtx,
) -> Result<CHIRExpr, CHIRLowerError> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(bound) = ctx.bindings.get(&compact) {
        return Ok(bound.clone());
    }
    lower_lit_expr(text, span, &ctx.enums)
}

fn lower_lit_expr(
    text: &str,
    span: SourceSpan,
    enums: &EnumRegistry,
) -> Result<CHIRExpr, CHIRLowerError> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();

    // Keyword literals — checked before the identifier case (`true`/`false` are
    // valid identifiers and would otherwise be treated as variable references).
    match compact.as_str() {
        "true"  => return Ok(CHIRExpr::Lit(CHIRLit { ty: CHIRType::Bool, value: 1 })),
        "false" => return Ok(CHIRExpr::Lit(CHIRLit { ty: CHIRType::Bool, value: 0 })),
        // 4-state single-bit constants → 1-bit literals.
        "Logic::One"  => return Ok(CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(1) }, value: 1 })),
        "Logic::Zero" => return Ok(CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(1) }, value: 0 })),
        _ => {}
    }

    if is_ident(&compact) {
        return Ok(CHIRExpr::Var(compact));
    }

    if let Some((value, _)) = parse_int_literal(&compact) {
        let ty = infer_type_from_suffix(&compact).unwrap_or(CHIRType::UInt { width: Width::Concrete(64) });
        return Ok(CHIRExpr::Lit(CHIRLit { ty, value }));
    }

    // An enum variant path (`State::IDLE`) → its encoded value.
    if let Some((ty, value)) = resolve_enum_path(&compact, enums) {
        return Ok(CHIRExpr::Lit(CHIRLit { ty, value }));
    }

    Err(CHIRLowerError::UnsupportedConstruct {
        description: format!("cannot lower literal: {}", text),
        span,
        suggested_rewrite: None,
    })
}

/// Assemble a binary operation with **signedness propagation** (see
/// `CHIRExpr::SignCast`). Two's complement makes `+ - * & | ^ <<` bit-identical,
/// so the wrapper moves to the RESULT there; it stays on the operands where
/// signedness is observable — comparisons (SystemVerilog compares signed iff
/// every operand is signed; a plain operand, e.g. a literal, is wrapped along,
/// matching Rust's same-type requirement) and `%`; a right shift keeps a signed
/// LEFT operand, which the emitter renders as `>>>`. `==`/`!=` and the logical
/// ops see bits only.
fn signed_binop(op: CHIRBinOp, left: CHIRExpr, right: CHIRExpr) -> CHIRExpr {
    fn peel(e: CHIRExpr) -> (bool, CHIRExpr) {
        match e {
            CHIRExpr::SignCast { signed: true, expr } => (true, *expr),
            other => (false, other),
        }
    }
    fn wrap(e: CHIRExpr) -> Box<CHIRExpr> {
        Box::new(CHIRExpr::SignCast { signed: true, expr: Box::new(e) })
    }
    let (ls, li) = peel(left);
    let (rs, ri) = peel(right);
    if !ls && !rs {
        return CHIRExpr::BinOp { left: Box::new(li), op, right: Box::new(ri) };
    }
    match op {
        CHIRBinOp::Lt | CHIRBinOp::Lte | CHIRBinOp::Gt | CHIRBinOp::Gte => CHIRExpr::BinOp {
            left: wrap(li),
            op,
            right: wrap(ri),
        },
        CHIRBinOp::Rem => CHIRExpr::SignCast {
            signed: true,
            expr: Box::new(CHIRExpr::BinOp { left: wrap(li), op, right: wrap(ri) }),
        },
        CHIRBinOp::Shr => CHIRExpr::SignCast {
            signed: true,
            expr: Box::new(CHIRExpr::BinOp { left: wrap(li), op, right: Box::new(ri) }),
        },
        CHIRBinOp::Eq | CHIRBinOp::Neq | CHIRBinOp::LogicalAnd | CHIRBinOp::LogicalOr => {
            CHIRExpr::BinOp { left: Box::new(li), op, right: Box::new(ri) }
        }
        _ => CHIRExpr::SignCast {
            signed: true,
            expr: Box::new(CHIRExpr::BinOp { left: Box::new(li), op, right: Box::new(ri) }),
        },
    }
}

fn lower_binop(op: &str, span: SourceSpan) -> Result<CHIRBinOp, CHIRLowerError> {
    match op {
        "+"  => Ok(CHIRBinOp::Add { wrapping: false }),
        "-"  => Ok(CHIRBinOp::Sub { wrapping: false }),
        "*"  => Ok(CHIRBinOp::Mul { wrapping: false }),
        "%"  => Ok(CHIRBinOp::Rem),
        "&"  => Ok(CHIRBinOp::BitAnd),
        "|"  => Ok(CHIRBinOp::BitOr),
        "^"  => Ok(CHIRBinOp::BitXor),
        "<<" => Ok(CHIRBinOp::Shl),
        ">>" => Ok(CHIRBinOp::Shr),
        "==" => Ok(CHIRBinOp::Eq),
        "!=" => Ok(CHIRBinOp::Neq),
        "<"  => Ok(CHIRBinOp::Lt),
        "<=" => Ok(CHIRBinOp::Lte),
        ">"  => Ok(CHIRBinOp::Gt),
        ">=" => Ok(CHIRBinOp::Gte),
        "&&" => Ok(CHIRBinOp::LogicalAnd),
        "||" => Ok(CHIRBinOp::LogicalOr),
        _ => Err(CHIRLowerError::UnsupportedConstruct {
            description: format!("binary operator '{}' not supported in hardware", op),
            span,
            suggested_rewrite: None,
        }),
    }
}

fn lower_unop(op: &str, span: SourceSpan) -> Result<CHIRUnOp, CHIRLowerError> {
    match op {
        "!"  => Ok(CHIRUnOp::LogicalNot),
        "~"  => Ok(CHIRUnOp::BitNot),
        "-"  => Ok(CHIRUnOp::Neg),
        _ => Err(CHIRLowerError::UnsupportedConstruct {
            description: format!("unary operator '{}' not supported in hardware", op),
            span,
            suggested_rewrite: None,
        }),
    }
}

fn lower_method_call(
    mc: &copper_core::frontend_ir::ExprMethodCall,
    ctx: &mut LowerCtx,
) -> Result<CHIRExpr, CHIRLowerError> {
    // `a.arithmetic_shift_right(n)` IS the signed shift: `$signed(a) >>> n`,
    // the same lowering `(a.as_u32() as i32) >> n` gets via the cast path.
    if mc.method == "arithmetic_shift_right" && mc.args.len() == 1 {
        let recv = lower_expr(&mc.receiver, ctx)?;
        let amt = lower_expr(&mc.args[0], ctx)?;
        return Ok(signed_binop(
            CHIRBinOp::Shr,
            CHIRExpr::SignCast { signed: true, expr: Box::new(recv) },
            amt,
        ));
    }

    // `mem.read_port::<I>().data()` / `.is_ready()` — the read port's output
    // and its valid flag. Checked before the passthrough list below, which would
    // otherwise swallow nothing here but keeps the memory forms adjacent.
    if mc.args.is_empty() && (mc.method == "data" || mc.method == "is_ready") {
        let span = mc.span;
        if let Some((mem, port)) = parse_mem_port(&mc.receiver, "read_port", ctx, span)? {
            return Ok(if mc.method == "data" {
                CHIRExpr::MemData { mem, port }
            } else {
                CHIRExpr::MemValid { mem, port }
            });
        }
        // A *write* port's `is_ready()` is unconditionally true in the simulator
        // (a pipelined write port always accepts), so it lowers to a constant.
        if mc.method == "is_ready" {
            if parse_mem_port(&mc.receiver, "write_port", ctx, span)?.is_some() {
                return Ok(CHIRExpr::Lit(CHIRLit {
                    ty: CHIRType::Bool,
                    value: 1,
                }));
            }
        }
    }

    match mc.method.as_str() {
        // Value passthroughs (simulation-only conversions on already-hardware
        // values): `port.read()`, `logic.as_bool()`. Lower to the receiver.
        "read" | "as_bool" | "as_u8" | "as_u16" | "as_u32" | "as_u64" | "as_u128"
        | "as_usize" | "as_bits"
            if mc.args.is_empty() =>
        {
            lower_expr(&mc.receiver, ctx)
        }

        "wrapping_add" if mc.args.len() == 1 => {
            let left = lower_expr(&mc.receiver, ctx)?;
            let right = lower_expr(&mc.args[0], ctx)?;
            Ok(CHIRExpr::BinOp {
                left: Box::new(left),
                op: CHIRBinOp::Add { wrapping: true },
                right: Box::new(right),
            })
        }
        "wrapping_sub" if mc.args.len() == 1 => {
            let left = lower_expr(&mc.receiver, ctx)?;
            let right = lower_expr(&mc.args[0], ctx)?;
            Ok(CHIRExpr::BinOp {
                left: Box::new(left),
                op: CHIRBinOp::Sub { wrapping: true },
                right: Box::new(right),
            })
        }
        "wrapping_mul" if mc.args.len() == 1 => {
            let left = lower_expr(&mc.receiver, ctx)?;
            let right = lower_expr(&mc.args[0], ctx)?;
            Ok(CHIRExpr::BinOp {
                left: Box::new(left),
                op: CHIRBinOp::Mul { wrapping: true },
                right: Box::new(right),
            })
        }
        "saturating_add" | "saturating_sub" | "checked_add" | "checked_sub" => {
            Err(CHIRLowerError::UnsupportedConstruct {
                description: format!(
                    "`{}` is not supported in hardware; use wrapping arithmetic or plain `+`/`-`",
                    mc.method
                ),
                span: mc.span,
                suggested_rewrite: Some(format!(
                    "replace with `.wrapping_{}()`",
                    mc.method.trim_start_matches("saturating_").trim_start_matches("checked_")
                )),
            })
        }
        "lock" | "unwrap" | "clone" => lower_expr(&mc.receiver, ctx),
        _ => Err(CHIRLowerError::UnsupportedConstruct {
            description: format!("method `{}` is not supported in hardware expressions", mc.method),
            span: mc.span,
            suggested_rewrite: None,
        }),
    }
}

fn lower_hardware_call(
    call: &copper_core::frontend_ir::ExprCall,
    ctx: &mut LowerCtx,
) -> Result<CHIRExpr, CHIRLowerError> {
    let module_name = match call.func.as_ref() {
        ExprType::Lit(lit) => lit.text.trim().to_string(),
        ExprType::Path(p) => p.path_text.trim().to_string(),
        _ => return Err(CHIRLowerError::UnsupportedConstruct {
            description: "hardware module call with non-identifier callee".to_string(),
            span: call.span,
            suggested_rewrite: None,
        }),
    };

    let (inst_name, output_wire) = ctx.next_inst_name(&module_name);

    let (inputs, output_ty) = if let Some(callee) = ctx.registry.get(&module_name) {
        // Skip clock params when mapping positional args → port names
        let data_params: Vec<_> = callee.signature.params.iter()
            .filter(|p| {
                let compact = compact_type(&p.ty.ty_text);
                !compact.starts_with("Clock<")
            })
            .collect();

        let mut inputs = Vec::new();
        for (i, arg) in call.args.iter().enumerate() {
            let port_name = data_params.get(i)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| format!("arg{}", i));
            let expr = lower_expr(arg, ctx)?;
            inputs.push((port_name, expr));
        }

        let output_ty = callee.signature.return_ty.as_ref()
            .map(|rt| resolve_type(&rt.ty_text, rt.span))
            .transpose()?
            .unwrap_or(CHIRType::UInt { width: Width::Concrete(1) });

        (inputs, output_ty)
    } else {
        // Callee not in registry — use positional port names as fallback
        let mut inputs = Vec::new();
        for (i, arg) in call.args.iter().enumerate() {
            let expr = lower_expr(arg, ctx)?;
            inputs.push((format!("arg{}", i), expr));
        }
        (inputs, CHIRType::UInt { width: Width::Concrete(8) })
    };

    ctx.submodules.push(CHIRSubmoduleInst {
        inst_name,
        module_name,
        inputs,
        output_wire: output_wire.clone(),
        output_ty,
        // Legacy expression model: clock is filtered above, output uses the
        // conventional `.out`, no direct net connections. The structural
        // (statement/port) form populates these instead — see `lower_structural_body`.
        clocks: Vec::new(),
        port_nets: Vec::new(),
        output_port: None,
        span: call.span,
    });

    Ok(CHIRExpr::Var(output_wire))
}

// ── Pattern parsing ───────────────────────────────────────────────────────────

/// Size suffix-less integer pattern literals to the scrutinee's width.
///
/// `parse_pattern` gives a bare `55` the 64-bit default, and the emitted case
/// label then compares `op == 64'd55` — WIDTHEXPAND against a narrower
/// scrutinee (the `match_on_usize` / `match_on_literals` ledger entries; the
/// width comes from the LITERAL, not the scrutinee). A literal whose value does
/// not fit the scrutinee's width keeps the default rather than silently
/// truncating into a different match.
///
/// Also the home of const-name patterns: a name that is really a file-scope
/// const (a `localparam`) or a const-generic parameter parses as an enum-variant
/// pattern; it is rewritten here to [`CHIRPattern::Const`], which carries the
/// NAME to the emitted case label (SystemVerilog evaluates the localparam —
/// consts deliberately keep their source expression, see `file_consts`). This
/// was a refusal until 2026-08-27 (the `match_on_const_pattern` ledger entry).
fn size_patterns_to_scrutinee(
    patterns: Vec<CHIRPattern>,
    scrutinee: &CHIRExpr,
    span: SourceSpan,
    ctx: &LowerCtx,
) -> Result<Vec<CHIRPattern>, CHIRLowerError> {
    let _ = span;
    let patterns: Vec<CHIRPattern> = patterns
        .into_iter()
        .map(|p| match p {
            CHIRPattern::EnumVariant { name, inner: None } if ctx.params.contains(&name) => {
                CHIRPattern::Const { name }
            }
            other => other,
        })
        .collect();
    let Some(w) = width_of_chir_expr(scrutinee, ctx) else { return Ok(patterns) };
    Ok(patterns
        .into_iter()
        .map(|p| match p {
            CHIRPattern::Lit(CHIRLit {
                ty: CHIRType::UInt { width: Width::Concrete(dw) },
                value,
            }) if dw == DEFAULT_LIT_WIDTH
                && (w >= 128 || value < (1u128 << w)) =>
            {
                CHIRPattern::Lit(CHIRLit {
                    ty: CHIRType::UInt { width: Width::Concrete(w) },
                    value,
                })
            }
            other => other,
        })
        .collect())
}

/// Parse a `pattern_text` string into a single `CHIRPattern`.
pub fn parse_pattern(
    text: &str,
    span: SourceSpan,
    enums: &EnumRegistry,
) -> Result<CHIRPattern, CHIRLowerError> {
    let s = text.trim();

    if s == "_" {
        return Ok(CHIRPattern::Wildcard);
    }

    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len() - 1];
        let parts = split_top_level_commas(inner);
        let sub: Result<Vec<_>, _> =
            parts.iter().map(|p| parse_pattern(p.trim(), span, enums)).collect();
        return Ok(CHIRPattern::Tuple(sub?));
    }

    if let Some((value, _)) = parse_int_literal(s) {
        let ty = infer_type_from_suffix(s).unwrap_or(CHIRType::UInt { width: Width::Concrete(64) });
        return Ok(CHIRPattern::Lit(CHIRLit { ty, value }));
    }

    match s {
        "true"  => return Ok(CHIRPattern::Lit(CHIRLit { ty: CHIRType::Bool, value: 1 })),
        "false" => return Ok(CHIRPattern::Lit(CHIRLit { ty: CHIRType::Bool, value: 0 })),
        _ => {}
    }

    // Path patterns: `Logic::One` / `Logic::Zero`, and enum variants, all match
    // against a concrete encoded value.
    let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    match compact.as_str() {
        "Logic::One" => {
            return Ok(CHIRPattern::Lit(CHIRLit {
                ty: CHIRType::UInt { width: Width::Concrete(1) },
                value: 1,
            }))
        }
        "Logic::Zero" => {
            return Ok(CHIRPattern::Lit(CHIRLit {
                ty: CHIRType::UInt { width: Width::Concrete(1) },
                value: 0,
            }))
        }
        _ => {}
    }
    if let Some((ty, value)) = resolve_enum_path(&compact, enums) {
        return Ok(CHIRPattern::Lit(CHIRLit { ty, value }));
    }

    if is_ident(s) {
        if s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            return Ok(CHIRPattern::EnumVariant { name: s.to_string(), inner: None });
        }
        return Ok(CHIRPattern::Wildcard);
    }

    Err(CHIRLowerError::UnsupportedConstruct {
        description: format!("unsupported pattern: {}", text),
        span,
        suggested_rewrite: None,
    })
}

/// Parse an or-pattern string (`"1 | 2 | 3"`) into all alternatives.
///
/// `1 | 2` → `[Lit(1), Lit(2)]`
/// `_`     → `[Wildcard]`
pub fn parse_or_patterns(text: &str, span: SourceSpan, enums: &EnumRegistry) -> Result<Vec<CHIRPattern>, CHIRLowerError> {
    let mut patterns = Vec::new();
    let mut remaining = text.trim();

    loop {
        match find_top_level_pipe(remaining) {
            Some(idx) => {
                patterns.push(parse_pattern(&remaining[..idx], span, enums)?);
                remaining = remaining[idx + 1..].trim_start();
            }
            None => {
                patterns.push(parse_pattern(remaining, span, enums)?);
                break;
            }
        }
    }

    Ok(patterns)
}

// ── Post-lowering validation ──────────────────────────────────────────────────

/// Validate scope: all `CHIRExpr::Var` references must resolve to a declared name.
/// Also checks that `emit!` is only used when an output port exists.
fn validate_module(
    module: &CHIRModule,
    fir: &FrontendModuleIR,
) -> Result<(), CHIRLowerError> {
    validate_module_scope(module).map_err(|e| explain_skipped_const(e, fir))
}

/// A reference to a file-scope const that this pass could *not* turn into a
/// `localparam` surfaces as an ordinary "undefined variable". That is the right
/// error, but on its own it points at the use site and says nothing about the
/// const, so the reader has no way to tell it apart from a genuine typo. Attach
/// the reason the const was skipped.
fn explain_skipped_const(err: CHIRLowerError, fir: &FrontendModuleIR) -> CHIRLowerError {
    let CHIRLowerError::UnsupportedConstruct { description, span, suggested_rewrite } = err else {
        return err;
    };
    let name = description
        .strip_prefix("undefined variable '")
        .and_then(|rest| rest.split('\'').next())
        .map(str::to_string);
    let note = name.and_then(|n| crate::file_consts::rejection_note(fir, &n));
    match note {
        Some(note) => CHIRLowerError::UnsupportedConstruct {
            description: format!("{description} — {note}"),
            span,
            suggested_rewrite,
        },
        None => CHIRLowerError::UnsupportedConstruct { description, span, suggested_rewrite },
    }
}

fn validate_module_scope(module: &CHIRModule) -> Result<(), CHIRLowerError> {
    let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();

    for port in &module.ports {
        known.insert(port.name.clone());
    }
    // Module parameters (const generics) are usable in expressions (e.g. a loop
    // bound `N`); they resolve to SystemVerilog parameters at emission.
    for p in &module.params {
        known.insert(p.name.clone());
    }
    // File-scope `const` items resolve the same way, as `localparam`s.
    for lp in &module.localparams {
        known.insert(lp.name.clone());
    }

    match &module.body {
        CHIRBody::Combinational(body) => {
            // Submodule output wires are available everywhere in a comb body
            for sub in &body.submodules {
                for (_, expr) in &sub.inputs {
                    validate_expr(expr, &known, module.span)?;
                }
                known.insert(sub.output_wire.clone());
            }
            validate_stmts(&body.stmts, &mut known, module.span)?;
        }
        CHIRBody::Sequential(body) => {
            for reg in &body.registers {
                known.insert(reg.name.clone());
            }
            for sub in &body.submodules {
                for (_, expr) in &sub.inputs {
                    validate_expr(expr, &known, module.span)?;
                }
                known.insert(sub.output_wire.clone());
            }
            validate_stmts(&body.loop_body, &mut known, module.span)?;
        }
        CHIRBody::Structural(body) => {
            // Internal nets and parent ports are the resolvable names; each
            // submodule's clock/port connections must reference one of them.
            for (net, _) in &body.nets {
                known.insert(net.clone());
            }
            for sub in &body.submodules {
                for (_, sig) in sub.clocks.iter().chain(sub.port_nets.iter()) {
                    if !known.contains(sig) {
                        return Err(CHIRLowerError::UnsupportedConstruct {
                            description: format!(
                                "structural instance `{}` connects to unknown signal `{}` \
                                 (declare it as a parent port or `let {} = wire::<..>(..)`)",
                                sub.inst_name, sig, sig
                            ),
                            span: sub.span,
                            suggested_rewrite: None,
                        });
                    }
                }
            }
        }
    }

    Ok(())
}

fn validate_stmts(
    stmts: &[CHIRStmt],
    known: &mut std::collections::HashSet<String>,
    _span: SourceSpan,
) -> Result<(), CHIRLowerError> {
    for stmt in stmts {
        match stmt {
            CHIRStmt::Wire { name, value, span: s, .. } => {
                validate_expr(value, known, *s)?;
                known.insert(name.clone());
            }
            CHIRStmt::Assign { value, span: s, .. } => {
                validate_expr(value, known, *s)?;
            }
            CHIRStmt::PortWrite { value, span: s, .. } => {
                validate_expr(value, known, *s)?;
            }
            CHIRStmt::AwaitTick { .. } => {}
            CHIRStmt::If { condition, then_body, else_body, span: s } => {
                validate_expr(condition, known, *s)?;
                let mut then_known = known.clone();
                validate_stmts(then_body, &mut then_known, *s)?;
                if let Some(eb) = else_body {
                    let mut else_known = known.clone();
                    validate_stmts(eb, &mut else_known, *s)?;
                }
            }
            CHIRStmt::Match { scrutinee, arms, span: s } => {
                validate_expr(scrutinee, known, *s)?;
                for arm in arms {
                    let mut arm_known = known.clone();
                    validate_stmts(&arm.body, &mut arm_known, *s)?;
                }
            }
            CHIRStmt::ForLoop { var, start, end, body, span: s } => {
                validate_expr(start, known, *s)?;
                validate_expr(end, known, *s)?;
                // The loop variable is in scope for the body. Bounds like a module
                // parameter `N` are not `known` locals, so they are exempt from the
                // use-before-def check (they resolve to SV parameters at emission).
                let mut body_known = known.clone();
                body_known.insert(var.clone());
                validate_stmts(body, &mut body_known, *s)?;
            }
            CHIRStmt::MemRead { addr, span: s, .. } => {
                validate_expr(addr, known, *s)?;
            }
            CHIRStmt::MemWrite { addr, value, span: s, .. } => {
                validate_expr(addr, known, *s)?;
                validate_expr(value, known, *s)?;
            }
            CHIRStmt::IndexAssign { base, index, value, span: s } => {
                // `base` must be an already-declared signal; the bit-assign drives
                // one of its bits.
                if !known.contains(base.as_str()) {
                    return Err(CHIRLowerError::UnsupportedConstruct {
                        description: format!("bit-assign target '{base}' is not declared"),
                        span: *s,
                        suggested_rewrite: None,
                    });
                }
                validate_expr(index, known, *s)?;
                validate_expr(value, known, *s)?;
            }
        }
    }
    Ok(())
}

fn validate_expr(
    expr: &CHIRExpr,
    known: &std::collections::HashSet<String>,
    span: SourceSpan,
) -> Result<(), CHIRLowerError> {
    match expr {
        CHIRExpr::Var(name) => {
            if !known.contains(name.as_str()) {
                return Err(CHIRLowerError::UnsupportedConstruct {
                    description: format!("undefined variable '{}' referenced in hardware expression", name),
                    span,
                    suggested_rewrite: None,
                });
            }
        }
        CHIRExpr::Lit(_) => {}
        CHIRExpr::SignCast { expr, .. } => validate_expr(expr, known, span)?,
        CHIRExpr::BinOp { left, right, .. } => {
            validate_expr(left, known, span)?;
            validate_expr(right, known, span)?;
        }
        CHIRExpr::UnOp { expr, .. } => {
            validate_expr(expr, known, span)?;
        }
        CHIRExpr::Mux { cond, then_val, else_val } => {
            validate_expr(cond, known, span)?;
            validate_expr(then_val, known, span)?;
            validate_expr(else_val, known, span)?;
        }
        CHIRExpr::Case { scrutinee, arms, default } => {
            validate_expr(scrutinee, known, span)?;
            for arm in arms {
                validate_expr(&arm.value, known, span)?;
                if let Some(g) = &arm.guard {
                    validate_expr(g, known, span)?;
                }
            }
            if let Some(def) = default {
                validate_expr(def, known, span)?;
            }
        }
        CHIRExpr::Concat(exprs) => {
            for e in exprs {
                validate_expr(e, known, span)?;
            }
        }
        CHIRExpr::Slice { expr, .. } => {
            validate_expr(expr, known, span)?;
        }
        CHIRExpr::DynBit { base, index } => {
            validate_expr(base, known, span)?;
            validate_expr(index, known, span)?;
        }
        CHIRExpr::Resize { expr, .. } => validate_expr(expr, known, span)?,
        // A memory read result names a memory, not a local — nothing to resolve
        // against `known` (the declaration was checked when it was recognized).
        CHIRExpr::MemData { .. } | CHIRExpr::MemValid { .. } => {}
    }
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn is_tick_await(base: &ExprType) -> bool {
    match base {
        ExprType::MethodCall(mc) => mc.method == "tick" && mc.args.is_empty(),
        _ => false,
    }
}

fn extract_assign_target(expr: &ExprType, span: SourceSpan) -> Result<String, CHIRLowerError> {
    let name = match expr {
        ExprType::Lit(lit) => lit.text.trim().to_string(),
        ExprType::Path(p) => p.path_text.trim().to_string(),
        _ => {
            return Err(CHIRLowerError::UnsupportedConstruct {
                description: "complex assignment targets not supported".to_string(),
                span,
                suggested_rewrite: None,
            })
        }
    };
    if is_ident(&name) {
        Ok(name)
    } else {
        Err(CHIRLowerError::UnsupportedConstruct {
            description: format!("invalid assignment target: {}", name),
            span,
            suggested_rewrite: None,
        })
    }
}

fn extract_block_expr_value(
    block: &[RawStmt],
    span: SourceSpan,
    ctx: &mut LowerCtx,
) -> Result<CHIRExpr, CHIRLowerError> {
    for stmt in block.iter().rev() {
        if let RawStmtKind::Expr(es) = &stmt.kind {
            if !es.has_semi {
                return lower_expr(&es.expr, ctx);
            }
        }
    }
    Err(CHIRLowerError::UnsupportedConstruct {
        description: "if-as-expression branch has no value expression".to_string(),
        span,
        suggested_rewrite: None,
    })
}

/// Why a nested `loop` could not be flattened, as a diagnostic.
///
/// Three distinct reasons land here and they need three messages — the fix for
/// each is different, and until 2026-08-24 all of them printed the ordering rule.
/// That was unfollowable for two of the three: "the tick has to be the LAST
/// statement" is not advice you can act on when the body has no tick in statement
/// position at all.
///
///  1. **no tick anywhere** — an unbounded combinational loop;
///  2. **a tick in statement position, but not last** — the refused ordering
///     (`loop { tick; <test> }`), which is outside the language by decision;
///  3. **ticks only inside branches** — no single "code between two ticks"
///     segment, so there is no state for the flattener to build.
///
/// The span walks INWARD first: when the body's last statement is another
/// tick-bearing loop (cause K's shape, which the flattener does accept), the outer
/// loop is well-formed and the fault is somewhere inside, so reporting the outer
/// one names a construct the author has no reason to doubt.
fn nested_loop_error(l: &ExprLoop, span: SourceSpan) -> CHIRLowerError {
    // Descend to the innermost loop that is actually at fault.
    if let Some(last) = l.body.last() {
        if let RawStmtKind::Expr(es) = &last.kind {
            if let ExprType::Loop(inner) = &es.expr {
                if stmts_contain_tick(&inner.body) {
                    return nested_loop_error(inner, last.span);
                }
            }
        }
    }

    let has_statement_tick = l.body.iter().any(is_tick_stmt);
    let (description, suggested_rewrite) = if !stmts_contain_tick(&l.body) {
        (
            "a nested `loop` with no `clk.tick().await` would never terminate in hardware"
                .to_string(),
            Some("give the loop a `clk.tick().await`, or write it as a `for` over a constant \
                  range if it is meant to unroll"
                .to_string()),
        )
    } else if has_statement_tick {
        (
            "a repeating wait must be written as `loop { <test>; clk.tick().await; }` \
             — the tick has to be the LAST statement of the loop body. Testing AFTER \
             the tick reads an input in the window where a simulator samples the value \
             the just-past edge produced and a flip-flop samples the value present \
             before its own edge; the two are a cycle apart under any testbench that \
             changes inputs between edges. Copper does not choose between them — the \
             ordering is outside the language, the same way a pre-tick alignment hazard \
             is"
                .to_string(),
            Some("move the test before the tick: `loop { if ready { break; } \
                  clk.tick().await; }`"
                .to_string()),
        )
    } else {
        (
            "this nested `loop`'s body has no clock boundary of its own: every \
             `clk.tick().await` in it sits inside an `if` or `match` arm, so the body is \
             not one segment of code between two ticks and there is no state to build \
             from it. A nested loop's body must END at its boundary — either \
             `clk.tick().await` as the last statement, or another tick-bearing loop as \
             the last statement"
                .to_string(),
            Some("lift the tick out of the branches so it is the body's last statement: \
                  `loop { if ready { break; } <work>; clk.tick().await; }`"
                .to_string()),
        )
    };

    CHIRLowerError::UnsupportedConstruct { description, span, suggested_rewrite }
}

/// Is this statement a bare `clk.tick().await`, in statement position?
fn is_tick_stmt(stmt: &RawStmt) -> bool {
    match &stmt.kind {
        RawStmtKind::Expr(es) => match &es.expr {
            ExprType::Await(a) => is_tick_await(&a.base),
            _ => false,
        },
        _ => false,
    }
}

/// True if any statement issues a `clk.tick().await` at any nesting depth. A tick
/// is only valid at the top level of a module's `loop` body (that is what the
/// phase FSM extracts); a tick inside a `for`/`while`/`if`/`match` needs control
/// extraction (counters / self-loop states), which is not yet built — so such a
/// tick must be *rejected*, never silently dropped.
fn stmts_contain_tick(stmts: &[RawStmt]) -> bool {
    stmts.iter().any(stmt_contains_tick)
}

fn stmt_contains_tick(stmt: &RawStmt) -> bool {
    match &stmt.kind {
        RawStmtKind::Expr(es) => expr_contains_tick(&es.expr),
        RawStmtKind::Local(l) => l.init.as_ref().is_some_and(expr_contains_tick),
        RawStmtKind::Item(_) => false,
    }
}

fn expr_contains_tick(e: &ExprType) -> bool {
    match e {
        ExprType::Await(a) => is_tick_await(&a.base),
        ExprType::ForLoop(f) => stmts_contain_tick(&f.body),
        ExprType::Loop(l) => stmts_contain_tick(&l.body),
        ExprType::While(w) => stmts_contain_tick(&w.body),
        ExprType::Block(b) => stmts_contain_tick(&b.stmts),
        ExprType::If(f) => {
            stmts_contain_tick(&f.then_block)
                || f.else_branch.as_deref().is_some_and(expr_contains_tick)
        }
        ExprType::Match(m) => m.arms.iter().any(|a| expr_contains_tick(&a.body)),
        _ => false,
    }
}

fn reject_tick_in_branch(stmts: &[RawStmt], span: SourceSpan) -> Result<(), CHIRLowerError> {
    for stmt in stmts {
        if let RawStmtKind::Expr(es) = &stmt.kind {
            if let ExprType::Await(a) = &es.expr {
                if is_tick_await(&a.base) {
                    return Err(CHIRLowerError::TickInsideBranch { span });
                }
            }
        }
    }
    Ok(())
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false)
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

fn parse_int_literal(s: &str) -> Option<(u128, Option<usize>)> {
    let (num_part, width) = strip_int_suffix(s);
    let value = if num_part.starts_with("0x") || num_part.starts_with("0X") {
        u128::from_str_radix(&num_part[2..].replace('_', ""), 16).ok()?
    } else if num_part.starts_with("0b") || num_part.starts_with("0B") {
        u128::from_str_radix(&num_part[2..].replace('_', ""), 2).ok()?
    } else {
        num_part.replace('_', "").parse::<u128>().ok()?
    };
    Some((value, width))
}

fn strip_int_suffix(s: &str) -> (&str, Option<usize>) {
    let suffixes: &[(&str, usize)] = &[
        ("u128", 128), ("u64", 64), ("u32", 32), ("u16", 16), ("u8", 8),
        ("i128", 128), ("i64", 64), ("i32", 32), ("i16", 16), ("i8", 8),
        // See `infer_type_from_suffix`: `usize`/`isize` are 32-bit throughout.
        ("usize", 32), ("isize", 32),
    ];
    for (suffix, width) in suffixes {
        if let Some(stripped) = s.strip_suffix(suffix) {
            if !stripped.is_empty() {
                return (stripped, Some(*width));
            }
        }
    }
    (s, None)
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        parts.push(&s[start..]);
    }
    parts
}

fn find_top_level_pipe(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

// ── Memory recognition ────────────────────────────────────────────────────────
//
// A `Memory<T, R, W, D, READ_LAT, WRITE_LAT>` bound before the loop is a
// hardware submodule (an array plus per-port address/data buses), not a local
// wire. Recognition happens here so the rest of the pipeline sees explicit
// `MemRead`/`MemWrite` statements and `MemData`/`MemValid` expressions rather
// than opaque method calls.
//
// Only the `READ_LAT == WRITE_LAT == 1` form lowers today. Everything outside
// that is a *clean* error naming the construct, never a silent mis-lowering:
// the sim's deeper pipelines are real behaviour that the emitted array does not
// reproduce.

/// What the pre-loop scan learned about one `Memory<..>` binding, kept in the
/// lowering context so a `mem.read_port::<I>()` chain resolves and range-checks.
#[derive(Debug, Clone)]
pub(crate) struct MemInfo {
    pub(crate) elem_ty: CHIRType,
    pub(crate) read_ports: usize,
    pub(crate) write_ports: usize,
}

/// The index of the matching `>` for the `<` at `open` in `s` (byte offsets).
fn matching_angle(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices().skip(open) {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Recognize a `Memory<..>` constructor bound to a pre-loop `let`.
///
/// `Ok(None)` means "not a memory at all" — the caller falls through to normal
/// wire lowering. `Err` means "a memory, but not one this pipeline can emit".
/// A memory's initial contents as *source expressions*, held until the lowering
/// context exists (they can reference module parameters and constants, exactly
/// like the pre-loop wires that are lowered the same way).
enum RawMemInit<'a> {
    /// `from_fn(clk, N, |var| body)`.
    Fill { var: String, body: &'a ExprType },
    /// `from_contents(clk, vec![a, b, c])`.
    Words(Vec<&'a ExprType>),
}

type ParsedMemory<'a> = (CHIRMemoryDecl, Option<RawMemInit<'a>>);

fn parse_memory_decl<'a>(
    name: &str,
    init: Option<&'a ExprType>,
    span: SourceSpan,
) -> Result<Option<ParsedMemory<'a>>, CHIRLowerError> {
    let Some(init) = init else { return Ok(None) };

    // `Memory::<..>::new(..).read_first()` — the mode this lowering emits.
    // `.write_first()` is a different RAM, so it is refused rather than ignored.
    if let ExprType::MethodCall(mc) = init {
        return match mc.method.as_str() {
            "read_first" | "write_first" if mc.args.is_empty() => {
                let mut parsed = parse_memory_decl(name, Some(&mc.receiver), span)?;
                if let Some((decl, _)) = parsed.as_mut() {
                    decl.write_mode = if mc.method == "write_first" {
                        WriteMode::WriteFirst
                    } else {
                        WriteMode::ReadFirst
                    };
                }
                Ok(parsed)
            }
            _ => Ok(None),
        };
    }

    let ExprType::Call(call) = init else { return Ok(None) };
    let Some(path) = call_path(call) else { return Ok(None) };

    // Split the path *head* (everything before the turbofish) into segments, so a
    // module qualifier and the constructor name are both visible:
    //   `copper_core::Memory::<..>::new` → head `copper_core::Memory::` → [copper_core, Memory]
    //   `Memory::new`                    → head `Memory::new`            → [Memory, new]
    let head_end = path.find('<').unwrap_or(path.len());
    let segs: Vec<&str> =
        path[..head_end].split("::").filter(|s| !s.is_empty()).collect();
    let has_generics = head_end < path.len();
    let is_memory_ctor = if has_generics {
        segs.last() == Some(&"Memory")
    } else {
        segs.len() >= 2 && segs[segs.len() - 2] == "Memory"
    };
    if !is_memory_ctor {
        return Ok(None);
    }
    if !has_generics {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: format!(
                "memory `{name}` is declared without an explicit type; the transpiler needs the \
                 element type and port counts from the turbofish"
            ),
            span,
            suggested_rewrite: Some(
                "annotate the declaration: `Memory::<Bits<16>, 1, 1, D, 1, 1>::new(clk, N)`"
                    .to_string(),
            ),
        });
    }

    let open = head_end;
    let close = matching_angle(&path, open).ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
        description: format!("unbalanced generic arguments on memory `{name}`"),
        span,
        suggested_rewrite: None,
    })?;
    let ctor = path[close + 1..].trim_start_matches(':').to_string();
    if !matches!(ctor.as_str(), "new" | "from_fn" | "from_contents") {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: format!(
                "memory constructor `{ctor}` is not supported by the transpiler yet; \
                 `new`, `from_fn` and `from_contents` lower"
            ),
            span,
            suggested_rewrite: None,
        });
    }

    // `Memory::<Bits<16>,1,1,MainClk,1,1>::new` → the generic argument list.
    let args = split_top_level_commas(&path[open + 1..close]);
    if args.len() < 4 {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: format!(
                "memory `{name}` needs at least `Memory::<T, R, W, D>`; found {} generic argument(s)",
                args.len()
            ),
            span,
            suggested_rewrite: None,
        });
    }

    let elem_ty = resolve_type(args[0], span)?;
    let read_ports = parse_const_usize(args[1], "read port count", name, span)?;
    let write_ports = parse_const_usize(args[2], "write port count", name, span)?;
    // READ_LAT / WRITE_LAT both default to 1 when the turbofish omits them.
    let read_lat = match args.get(3 + 1) {
        Some(a) => parse_const_usize(a, "READ_LAT", name, span)?,
        None => 1,
    };
    let write_lat = match args.get(3 + 2) {
        Some(a) => parse_const_usize(a, "WRITE_LAT", name, span)?,
        None => 1,
    };
    if read_lat == 0 || write_lat == 0 {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: format!(
                "memory `{name}` has READ_LAT = {read_lat}, WRITE_LAT = {write_lat}; both must \
                 be at least 1 (a synchronous port cannot answer in zero cycles)"
            ),
            span,
            suggested_rewrite: None,
        });
    }

    // The depth, and the preload if there is one. `new`/`from_fn` state the size
    // directly; `from_contents` states it by how many words it supplies.
    let (depth, raw_init) = match ctor.as_str() {
        "new" => (literal_depth(call.args.get(1), name, "new", span)?, None),
        "from_fn" => {
            let depth = literal_depth(call.args.get(1), name, "from_fn", span)?;
            let f = call.args.get(2).ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
                description: format!("memory `{name}`: `from_fn` takes a clock, a size and a fill function"),
                span,
                suggested_rewrite: None,
            })?;
            let ExprType::Closure(c) = f else {
                return Err(CHIRLowerError::UnsupportedConstruct {
                    description: format!(
                        "memory `{name}`: the `from_fn` fill must be a closure written at the \
                         call site. The transpiler emits the fill it describes rather than \
                         running it, so a named function or a captured value has nothing to emit"
                    ),
                    span,
                    suggested_rewrite: Some("inline the fill: `from_fn(clk, N, |i| …)`".to_string()),
                });
            };
            let [var] = c.params.as_slice() else {
                return Err(CHIRLowerError::UnsupportedConstruct {
                    description: format!(
                        "memory `{name}`: the `from_fn` fill takes exactly one parameter (the index)"
                    ),
                    span,
                    suggested_rewrite: None,
                });
            };
            if !is_ident(var) {
                return Err(CHIRLowerError::UnsupportedConstruct {
                    description: format!(
                        "memory `{name}`: unsupported `from_fn` parameter pattern `{var}`; use a \
                         plain index name"
                    ),
                    span,
                    suggested_rewrite: None,
                });
            }
            (depth, Some(RawMemInit::Fill { var: var.clone(), body: &c.body }))
        }
        _ => {
            // `from_contents(clk, <words>)`. The words must be visible in the
            // source — `vec![…]` (desugared to an array) or `vec![x; n]`. A Vec
            // built at run time (the CPU's program image) is not something the
            // transpiler can see, and is refused rather than half-emitted.
            let words = call.args.get(1).ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
                description: format!("memory `{name}`: `from_contents` takes a clock and the contents"),
                span,
                suggested_rewrite: None,
            })?;
            match words {
                ExprType::Array(a) => {
                    if a.elements.is_empty() {
                        return Err(CHIRLowerError::UnsupportedConstruct {
                            description: format!("memory `{name}` is created with no contents"),
                            span,
                            suggested_rewrite: None,
                        });
                    }
                    (
                        a.elements.len(),
                        Some(RawMemInit::Words(a.elements.iter().collect())),
                    )
                }
                // `vec![x; n]` — every word the same expression, which is a fill
                // whose value happens not to mention the index.
                ExprType::Repeat(r) => {
                    let depth = literal_depth(Some(&r.len), name, "from_contents", span)?;
                    (depth, Some(RawMemInit::Fill { var: "i".to_string(), body: &r.expr }))
                }
                _ => {
                    return Err(CHIRLowerError::UnsupportedConstruct {
                        description: format!(
                            "memory `{name}`: `from_contents` needs its contents written at the \
                             call site (`vec![…]`). A `Vec` computed at run time has no emitted \
                             form — the transpiler does not execute Rust"
                        ),
                        span,
                        suggested_rewrite: Some(
                            "write the words inline, or describe them with `from_fn(clk, N, |i| …)`"
                                .to_string(),
                        ),
                    })
                }
            }
        }
    };

    Ok(Some((
        CHIRMemoryDecl {
            received: false,
            name: name.to_string(),
            elem_ty,
            depth,
            read_ports,
            write_ports,
            read_lat,
            write_lat,
            init: None, // filled in by the caller, once expressions can be lowered
            // The default; a `.read_first()` / `.write_first()` wrapper overrides
            // it on the way back out of the recursion above.
            write_mode: WriteMode::ReadFirst,
            span,
        },
        raw_init,
    )))
}

/// A memory size argument, which must be an integer literal to size the array.
fn literal_depth(
    arg: Option<&ExprType>,
    mem: &str,
    ctor: &str,
    span: SourceSpan,
) -> Result<usize, CHIRLowerError> {
    let depth = match arg {
        Some(ExprType::Lit(l)) => l
            .text
            .trim()
            .trim_end_matches("usize")
            .trim_end_matches('_')
            .replace('_', "")
            .parse::<usize>()
            .ok(),
        _ => None,
    }
    .ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
        description: format!(
            "memory `{mem}`: the size argument to `{ctor}` must be an integer literal"
        ),
        span,
        suggested_rewrite: None,
    })?;
    if depth == 0 {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: format!("memory `{mem}` has size 0"),
            span,
            suggested_rewrite: None,
        });
    }
    Ok(depth)
}

fn parse_const_usize(
    text: &str,
    what: &str,
    mem: &str,
    span: SourceSpan,
) -> Result<usize, CHIRLowerError> {
    text.trim().parse::<usize>().map_err(|_| CHIRLowerError::UnsupportedConstruct {
        description: format!(
            "memory `{mem}`: {what} `{}` is not a concrete number; generic memories are not \
             transpilable (the CLI only handles concrete modules)",
            text.trim()
        ),
        span,
        suggested_rewrite: None,
    })
}

/// The type of `mem.read_port::<I>().data()` / `.is_ready()` — the element type
/// and a single bit respectively. `None` when `init` is not a memory read result.
fn mem_result_type(
    init: &ExprType,
    ctx: &LowerCtx,
    span: SourceSpan,
) -> Result<Option<CHIRType>, CHIRLowerError> {
    let ExprType::MethodCall(mc) = init else { return Ok(None) };
    if !mc.args.is_empty() {
        return Ok(None);
    }
    let Some((mem, _)) = parse_mem_port(&mc.receiver, "read_port", ctx, span)? else {
        return Ok(None);
    };
    Ok(match mc.method.as_str() {
        "data" => ctx.memories.get(&mem).map(|m| m.elem_ty.clone()),
        "is_ready" => Some(CHIRType::Bool),
        _ => None,
    })
}

/// Pre-order walk over every sub-expression.
fn walk_chir_expr(expr: &CHIRExpr, f: &mut impl FnMut(&CHIRExpr)) {
    f(expr);
    match expr {
        CHIRExpr::BinOp { left, right, .. } => {
            walk_chir_expr(left, f);
            walk_chir_expr(right, f);
        }
        CHIRExpr::UnOp { expr, .. }
        | CHIRExpr::Resize { expr, .. }
        | CHIRExpr::SignCast { expr, .. } => walk_chir_expr(expr, f),
        CHIRExpr::Mux { cond, then_val, else_val } => {
            walk_chir_expr(cond, f);
            walk_chir_expr(then_val, f);
            walk_chir_expr(else_val, f);
        }
        CHIRExpr::Case { scrutinee, arms, default } => {
            walk_chir_expr(scrutinee, f);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_chir_expr(g, f);
                }
                walk_chir_expr(&arm.value, f);
            }
            if let Some(d) = default {
                walk_chir_expr(d, f);
            }
        }
        CHIRExpr::Concat(parts) => {
            for p in parts {
                walk_chir_expr(p, f);
            }
        }
        CHIRExpr::Slice { expr, .. } => walk_chir_expr(expr, f),
        CHIRExpr::DynBit { base, index } => {
            walk_chir_expr(base, f);
            walk_chir_expr(index, f);
        }
        CHIRExpr::Var(_) | CHIRExpr::Lit(_) | CHIRExpr::MemData { .. } | CHIRExpr::MemValid { .. } => {}
    }
}

/// If `recv` is `<mem>.<kind>::<I>()` for a known memory, return `(mem, I)`.
/// `kind` is `read_port` or `write_port`.
fn parse_mem_port(
    recv: &ExprType,
    kind: &str,
    ctx: &LowerCtx,
    span: SourceSpan,
) -> Result<Option<(String, usize)>, CHIRLowerError> {
    let ExprType::MethodCall(mc) = recv else { return Ok(None) };
    if mc.method != kind || !mc.args.is_empty() {
        return Ok(None);
    }
    let ExprType::Path(p) = mc.receiver.as_ref() else { return Ok(None) };
    let mem = p.path_text.trim().to_string();
    let Some(info) = ctx.memories.get(&mem) else { return Ok(None) };

    let idx_text = mc.turbofish.first().ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
        description: format!("`{mem}.{kind}()` needs an explicit port index, e.g. `{kind}::<0>()`"),
        span,
        suggested_rewrite: None,
    })?;
    let port = parse_const_usize(idx_text, "port index", &mem, span)?;
    let count = if kind == "read_port" { info.read_ports } else { info.write_ports };
    if port >= count {
        return Err(CHIRLowerError::UnsupportedConstruct {
            description: format!(
                "memory `{mem}` has {count} {kind}(s); index {port} is out of range"
            ),
            span,
            suggested_rewrite: None,
        });
    }
    Ok(Some((mem, port)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_core::frontend_ir::SourceSpan;

    fn span() -> SourceSpan { SourceSpan::default() }
    fn no_hw() -> std::collections::HashSet<String> { Default::default() }
    fn empty_registry() -> ModuleRegistry { Default::default() }
    fn hw(names: &[&str]) -> std::collections::HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn make_fir(src: &str) -> FrontendModuleIR {
        use syn::ItemFn;
        use crate::parser::capture_frontend_ir;
        let design_fn: ItemFn = syn::parse_str(src).unwrap();
        capture_frontend_ir(&design_fn, &no_hw()).unwrap()
    }

    fn make_fir_hw(src: &str, hardware_fns: &std::collections::HashSet<String>) -> FrontendModuleIR {
        use syn::ItemFn;
        use crate::parser::capture_frontend_ir;
        let design_fn: ItemFn = syn::parse_str(src).unwrap();
        capture_frontend_ir(&design_fn, hardware_fns).unwrap()
    }

    // ── Type resolution ──────────────────────────────────────────────────────

    #[test]
    fn test_resolve_primitive_uint_types() {
        assert_eq!(resolve_type("u8",   span()).unwrap(), CHIRType::UInt { width: Width::Concrete(8) });
        assert_eq!(resolve_type("u16",  span()).unwrap(), CHIRType::UInt { width: Width::Concrete(16) });
        assert_eq!(resolve_type("u32",  span()).unwrap(), CHIRType::UInt { width: Width::Concrete(32) });
        assert_eq!(resolve_type("u64",  span()).unwrap(), CHIRType::UInt { width: Width::Concrete(64) });
        assert_eq!(resolve_type("u128", span()).unwrap(), CHIRType::UInt { width: Width::Concrete(128) });
    }

    #[test]
    fn test_resolve_primitive_sint_types() {
        assert_eq!(resolve_type("i8",   span()).unwrap(), CHIRType::SInt { width: Width::Concrete(8) });
        assert_eq!(resolve_type("i16",  span()).unwrap(), CHIRType::SInt { width: Width::Concrete(16) });
        assert_eq!(resolve_type("i32",  span()).unwrap(), CHIRType::SInt { width: Width::Concrete(32) });
        assert_eq!(resolve_type("i64",  span()).unwrap(), CHIRType::SInt { width: Width::Concrete(64) });
        assert_eq!(resolve_type("i128", span()).unwrap(), CHIRType::SInt { width: Width::Concrete(128) });
    }

    #[test]
    fn test_resolve_bool() {
        assert_eq!(resolve_type("bool", span()).unwrap(), CHIRType::Bool);
    }

    #[test]
    fn test_resolve_bit_and_logic() {
        assert_eq!(resolve_type("Bit",   span()).unwrap(), CHIRType::UInt { width: Width::Concrete(1) });
        assert_eq!(resolve_type("Logic", span()).unwrap(), CHIRType::UInt { width: Width::Concrete(1) });
    }

    #[test]
    fn test_resolve_bits_n() {
        assert_eq!(resolve_type("Bits<8>",  span()).unwrap(), CHIRType::UInt { width: Width::Concrete(8) });
        assert_eq!(resolve_type("Bits<16>", span()).unwrap(), CHIRType::UInt { width: Width::Concrete(16) });
        assert_eq!(resolve_type("Bits<1>",  span()).unwrap(), CHIRType::UInt { width: Width::Concrete(1) });
    }

    #[test]
    fn test_resolve_arc_mutex_strips_wrapper() {
        assert_eq!(resolve_type("Arc<Mutex<u8>>",  span()).unwrap(), CHIRType::UInt { width: Width::Concrete(8) });
        assert_eq!(resolve_type("Arc<Mutex<u32>>", span()).unwrap(), CHIRType::UInt { width: Width::Concrete(32) });
        assert_eq!(resolve_type("Arc<Mutex<bool>>", span()).unwrap(), CHIRType::Bool);
    }

    #[test]
    fn test_resolve_arc_mutex_with_whitespace() {
        assert_eq!(
            resolve_type("Arc< Mutex< u8 > >", span()).unwrap(),
            CHIRType::UInt { width: Width::Concrete(8) }
        );
    }

    #[test]
    fn test_resolve_unknown_type_returns_error() {
        assert!(matches!(
            resolve_type("SomeUnknownType", span()),
            Err(CHIRLowerError::UnresolvableType { .. })
        ));
    }

    #[test]
    fn test_resolve_bits_identifier_width_is_symbolic_param() {
        // `Bits<N>` with an identifier arg is a const-generic parameter → a
        // symbolic width (M2), not an error. (Whether the param is actually
        // declared is validated elsewhere; an undeclared one fails at emission.)
        assert_eq!(
            resolve_type("Bits<N>", span()).unwrap(),
            CHIRType::UInt { width: Width::Param("N".to_string()) }
        );
    }

    #[test]
    fn test_resolve_bits_invalid_width_returns_error() {
        // Neither a number nor a plain identifier → unresolvable.
        assert!(matches!(
            resolve_type("Bits<1.5>", span()),
            Err(CHIRLowerError::UnresolvableType { .. })
        ));
    }

    #[test]
    fn test_resolve_clock_type_returns_error() {
        assert!(matches!(
            resolve_type("Clock<MainClk>", span()),
            Err(CHIRLowerError::UnresolvableType { .. })
        ));
    }

    // ── Type inference ───────────────────────────────────────────────────────

    #[test]
    fn test_infer_type_from_suffix_uint() {
        assert_eq!(infer_type_from_suffix("0u8"),   Some(CHIRType::UInt { width: Width::Concrete(8) }));
        assert_eq!(infer_type_from_suffix("42u32"),  Some(CHIRType::UInt { width: Width::Concrete(32) }));
        assert_eq!(infer_type_from_suffix("10u64"),  Some(CHIRType::UInt { width: Width::Concrete(64) }));
    }

    #[test]
    fn test_infer_type_from_suffix_sint() {
        assert_eq!(infer_type_from_suffix("0i8"),   Some(CHIRType::SInt { width: Width::Concrete(8) }));
        assert_eq!(infer_type_from_suffix("42i16"),  Some(CHIRType::SInt { width: Width::Concrete(16) }));
    }

    #[test]
    fn test_infer_type_from_suffix_none_for_plain_literal() {
        assert_eq!(infer_type_from_suffix("42"),  None);
        assert_eq!(infer_type_from_suffix("foo"), None);
    }

    #[test]
    fn test_infer_type_from_expr_typed_literal() {
        use copper_core::frontend_ir::ExprLit;
        let expr = ExprType::Lit(ExprLit { text: "0u8".to_string(), span: span() });
        assert_eq!(infer_type_from_expr(&expr, span(), &SymbolTable::new(), &EnumRegistry::new()).unwrap(), CHIRType::UInt { width: Width::Concrete(8) });
    }

    #[test]
    fn test_infer_type_from_expr_bool_literal() {
        use copper_core::frontend_ir::ExprLit;
        let expr = ExprType::Lit(ExprLit { text: "true".to_string(), span: span() });
        assert_eq!(infer_type_from_expr(&expr, span(), &SymbolTable::new(), &EnumRegistry::new()).unwrap(), CHIRType::Bool);
    }

    #[test]
    fn test_infer_type_from_expr_ambiguous_returns_error() {
        use copper_core::frontend_ir::ExprLit;
        let expr = ExprType::Lit(ExprLit { text: "42".to_string(), span: span() });
        assert!(matches!(
            infer_type_from_expr(&expr, span(), &SymbolTable::new(), &EnumRegistry::new()),
            Err(CHIRLowerError::AmbiguousWidth { .. })
        ));
    }

    #[test]
    fn test_infer_type_from_cast_expr() {
        // `x as u16` → infer u16
        let fir = make_fir("fn f(x: u8) -> u16 { x as u16 }");
        // The cast should resolve the wire type as u16 during comb body lowering
        let module = lower_to_chir(&fir, &no_hw(), &empty_registry()).unwrap();
        // If we reach here without AmbiguousWidth, inference worked
        assert!(matches!(module.body, CHIRBody::Combinational(_)));
    }

    /// Helper: find the inferred type of wire `name` in a combinational module.
    fn comb_wire_type(src: &str, name: &str) -> CHIRType {
        let fir = make_fir(src);
        let module = lower_to_chir(&fir, &no_hw(), &empty_registry()).unwrap();
        let body = match &module.body {
            CHIRBody::Combinational(b) => b,
            _ => panic!("expected combinational body"),
        };
        body.stmts
            .iter()
            .find_map(|s| match s {
                CHIRStmt::Wire { name: n, ty, .. } if n == name => Some(ty.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("wire '{name}' not found"))
    }

    #[test]
    fn infers_logic_read_and_bitand_as_one_bit() {
        // `.read()` on a `Logic` port + bitwise `&` → 1-bit. Previously ambiguous.
        let ty = comb_wire_type(
            "fn f(a: In<Logic, ()>, b: In<Logic, ()>, o: Out<Logic, ()>) \
             { let w = a.read() & b.read(); o.write(w); }",
            "w",
        );
        assert_eq!(ty, CHIRType::UInt { width: Width::Concrete(1) });
    }

    #[test]
    fn infers_read_of_bits_port_width() {
        // `.read()` on a `Bits<16>` port carries its width through a bitwise op.
        let ty = comb_wire_type(
            "fn f(a: In<Bits<16>, ()>, b: In<Bits<16>, ()>, o: Out<Bits<16>, ()>) \
             { let w = a.read() | b.read(); o.write(w); }",
            "w",
        );
        assert_eq!(ty, CHIRType::UInt { width: Width::Concrete(16) });
    }

    #[test]
    fn infers_comparison_as_bool() {
        // A comparison is 1-bit boolean regardless of operand width.
        let ty = comb_wire_type(
            "fn f(a: In<u8, ()>, b: In<u8, ()>, o: Out<bool, ()>) \
             { let c = a.read() == b.read(); o.write(c); }",
            "c",
        );
        assert_eq!(ty, CHIRType::Bool);
    }

    #[test]
    fn as_bool_is_stripped_in_condition() {
        // `x.read().as_bool()` as an `if` condition (the lfsr form) must lower
        // without error — as_bool is a simulation-only conversion on an
        // already-1-bit value.
        let fir = make_fir_hw(
            "async fn m(clk: Clock<C>, en: In<Logic, C>, d: In<Bits<8>, C>, o: Out<Bits<8>, C>) {
                let mut r = Bits::from_u8(0);
                loop {
                    o.write(r);
                    clk.tick().await;
                    if en.read().as_bool() { r = d.read(); }
                }
            }",
            &no_hw(),
        );
        let module = lower_to_chir(&fir, &no_hw(), &empty_registry());
        assert!(module.is_ok(), "as_bool should lower: {:?}", module.err());
    }

    #[test]
    fn lowers_bit_index_to_single_bit_slice() {
        // `d.read()[3]` → 1-bit slice `Slice { high: 3, low: 3 }`.
        let fir = make_fir("fn f(d: In<Bits<8>, ()>, o: Out<Logic, ()>) { let w = d.read()[3]; o.write(w); }");
        let module = lower_to_chir(&fir, &no_hw(), &empty_registry()).unwrap();
        let body = match &module.body {
            CHIRBody::Combinational(b) => b,
            _ => panic!("expected combinational body"),
        };
        let (ty, value) = body.stmts.iter().find_map(|s| match s {
            CHIRStmt::Wire { name, ty, value, .. } if name == "w" => Some((ty.clone(), value.clone())),
            _ => None,
        }).expect("wire w");
        assert_eq!(ty, CHIRType::UInt { width: Width::Concrete(1) });
        assert!(matches!(value, CHIRExpr::Slice { high: 3, low: 3, .. }), "got {value:?}");
    }

    #[test]
    fn non_constant_bit_index_lowers_to_dynbit() {
        // A variable bit index is now a dynamic single-bit select `d[i]`
        // (CHIRExpr::DynBit), not an error. A constant index still uses Slice.
        let fir = make_fir("fn f(d: In<Bits<8>, ()>, i: In<u8, ()>, o: Out<Logic, ()>) { let w = d.read()[i.read() as usize]; o.write(w); }");
        let chir = lower_to_chir(&fir, &no_hw(), &empty_registry())
            .expect("dynamic bit index should lower to DynBit");
        let CHIRBody::Combinational(b) = chir.body else { panic!("expected comb body") };
        let has_dynbit = b.stmts.iter().any(|s| matches!(
            s, CHIRStmt::Wire { value: CHIRExpr::DynBit { .. }, .. }));
        assert!(has_dynbit, "expected a DynBit in the lowered body");
    }

    #[test]
    fn enum_width_sizing() {
        assert_eq!(bits_for(0), 1);
        assert_eq!(bits_for(1), 1);
        assert_eq!(bits_for(2), 2);
        assert_eq!(bits_for(3), 2);
        assert_eq!(bits_for(4), 3);
        assert_eq!(bits_for(6), 3); // pattern_detector's 7-variant State
        assert_eq!(bits_for(255), 8);
    }

    #[test]
    fn enum_registry_encodes_variants() {
        use copper_core::frontend_ir::{EnumVariant, ItemEnum};
        // Explicit discriminants are honored; unannotated variants continue
        // sequentially from the previous one (Rust's own rule).
        let mk = |name: &str, d: Option<&str>| EnumVariant {
            name: name.to_string(),
            discriminant: d.map(|s| s.to_string()),
            span: span(),
        };
        let mut fir = make_fir("fn f(o: Out<Logic, ()>) { o.write(Logic::Zero); }");
        fir.enums = vec![ItemEnum {
            name: "State".to_string(),
            variants: vec![mk("A", Some("0")), mk("B", None), mk("C", Some("5"))],
            attrs: vec![],
            span: span(),
        }];

        let reg = build_enum_registry(&fir);
        let def = reg.get("State").expect("State enum");
        assert_eq!(def.variants["A"], 0);
        assert_eq!(def.variants["B"], 1); // continues from A
        assert_eq!(def.variants["C"], 5);
        assert_eq!(def.width, 3); // must hold 5

        // Path resolution yields the enum's type and the variant value.
        let (ty, value) = resolve_enum_path("State::C", &reg).expect("resolve");
        assert_eq!(ty, CHIRType::UInt { width: Width::Concrete(3) });
        assert_eq!(value, 5);
        assert!(resolve_enum_path("State::MISSING", &reg).is_none());
    }

    #[test]
    fn constructor_width_from_name_and_turbofish() {
        assert_eq!(constructor_width("Bits::from_u32"), Some(32));
        assert_eq!(constructor_width("Bits::from_u8"), Some(8));
        assert_eq!(constructor_width("Bits::from_u128"), Some(128));
        // Turbofish width is explicit and wins.
        assert_eq!(constructor_width("Bits::<16>::from_u8"), Some(16));
        assert_eq!(constructor_width("Bits::zero"), None);
    }

    #[test]
    fn infers_bits_constructor_width() {
        // `Bits::from_u16(..)` with no annotation → 16-bit wire (previously the
        // parser mis-extracted `Bits::from_u16` as a type and failed).
        let ty = comb_wire_type(
            "fn f(o: Out<Bits<16>, ()>) { let w = Bits::from_u16(5); o.write(w); }",
            "w",
        );
        assert_eq!(ty, CHIRType::UInt { width: Width::Concrete(16) });
    }

    #[test]
    fn lowers_bits_constructor_to_widthed_literal() {
        // The constructor lowers to its argument, retyped to the constructor
        // width — so `Bits::from_u8(5)` becomes an 8-bit literal `5`.
        let fir = make_fir("fn f(o: Out<Bits<8>, ()>) { let w = Bits::from_u8(5); o.write(w); }");
        let module = lower_to_chir(&fir, &no_hw(), &empty_registry()).unwrap();
        let body = match &module.body {
            CHIRBody::Combinational(b) => b,
            _ => panic!("expected combinational body"),
        };
        let value = body.stmts.iter().find_map(|s| match s {
            CHIRStmt::Wire { name, value, .. } if name == "w" => Some(value.clone()),
            _ => None,
        }).expect("wire w");
        assert!(matches!(
            value,
            CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(8) }, value: 5 })
        ));
    }

    // ── Register init lowering ────────────────────────────────────────────────

    #[test]
    fn test_lower_init_to_lit_typed_integer() {
        use copper_core::frontend_ir::ExprLit;
        let expr = ExprType::Lit(ExprLit { text: "0u8".to_string(), span: span() });
        let lit = lower_init_to_lit(&expr, &EnumRegistry::new()).unwrap();
        assert_eq!(lit.value, 0);
        assert_eq!(lit.ty, CHIRType::UInt { width: Width::Concrete(8) });
    }

    #[test]
    fn test_lower_init_to_lit_bool_true() {
        use copper_core::frontend_ir::ExprLit;
        let expr = ExprType::Lit(ExprLit { text: "true".to_string(), span: span() });
        let lit = lower_init_to_lit(&expr, &EnumRegistry::new()).unwrap();
        assert_eq!(lit.ty, CHIRType::Bool);
        assert_eq!(lit.value, 1);
    }

    #[test]
    fn test_lower_init_to_lit_complex_expr_returns_none() {
        use copper_core::frontend_ir::{ExprBinary, ExprLit};
        let expr = ExprType::Binary(ExprBinary {
            left: Box::new(ExprType::Lit(ExprLit { text: "a".to_string(), span: span() })),
            op: "+".to_string(),
            right: Box::new(ExprType::Lit(ExprLit { text: "b".to_string(), span: span() })),
            span: span(),
        });
        assert!(lower_init_to_lit(&expr, &EnumRegistry::new()).is_none());
    }

    #[test]
    fn test_seq_register_has_init_value() {
        let fir = make_fir(
            "async fn counter(clk: Clock<MainClk>) {
                let mut count: u8 = 5u8;
                loop { count = count.wrapping_add(1u8); clk.tick().await; }
            }"
        );
        let body = lower_seq_body(&fir, &no_hw(), &empty_registry()).unwrap();
        let reg = &body.registers[0];
        assert!(reg.init.is_some());
        assert_eq!(reg.init.as_ref().unwrap().value, 5);
    }

    #[test]
    fn test_seq_register_infers_type_from_init() {
        let fir = make_fir(
            "async fn counter(clk: Clock<MainClk>) {
                let mut count = 0u16;
                loop { count = count.wrapping_add(1u16); clk.tick().await; }
            }"
        );
        let body = lower_seq_body(&fir, &no_hw(), &empty_registry()).unwrap();
        assert_eq!(body.registers[0].ty, CHIRType::UInt { width: Width::Concrete(16) });
    }

    // ── Port extraction ──────────────────────────────────────────────────────

    #[test]
    fn test_ports_clock_detected_as_clock_kind() {
        let fir = make_fir("async fn counter(clk: Clock<MainClk>, data: In<u8, MainClk>) {}");
        let ports = lower_ports(&fir).unwrap();
        assert!(matches!(ports[0].kind, CHIRPortKind::Clock { .. }));
        assert_eq!(ports[0].name, "clk");
    }

    #[test]
    fn test_ports_clock_domain_extracted() {
        let fir = make_fir("async fn m(clk: Clock<MyDomain>, x: u8) {}");
        let ports = lower_ports(&fir).unwrap();
        match &ports[0].kind {
            CHIRPortKind::Clock { domain } => assert_eq!(domain, "MyDomain"),
            _ => panic!("expected clock port"),
        }
    }

    #[test]
    fn test_ports_in_wrapper_becomes_input() {
        let fir = make_fir("async fn m(clk: Clock<C>, data: In<u8, C>) {}");
        let ports = lower_ports(&fir).unwrap();
        assert!(matches!(ports[1].direction, CHIRPortDir::Input));
        match &ports[1].kind {
            CHIRPortKind::Data { ty } => assert_eq!(*ty, CHIRType::UInt { width: Width::Concrete(8) }),
            _ => panic!("expected data port"),
        }
    }

    #[test]
    fn test_ports_out_wrapper_becomes_output() {
        let fir = make_fir("async fn m(clk: Clock<C>, result: Out<u16, C>) {}");
        let ports = lower_ports(&fir).unwrap();
        let out = ports.iter().find(|p| p.name == "result").unwrap();
        assert!(matches!(out.direction, CHIRPortDir::Output));
        assert!(matches!(out.kind, CHIRPortKind::Data { ty: CHIRType::UInt { width: Width::Concrete(16) } }));
    }

    #[test]
    fn test_ports_return_type_becomes_output_port() {
        let fir = make_fir("fn m(x: u8) -> u16 { x as u16 }");
        let ports = lower_ports(&fir).unwrap();
        let out = ports.iter().find(|p| p.name == "out").unwrap();
        assert!(matches!(out.direction, CHIRPortDir::Output));
        assert!(matches!(out.kind, CHIRPortKind::Data { ty: CHIRType::UInt { width: Width::Concrete(16) } }));
    }

    #[test]
    fn test_ports_no_out_params_no_output_port() {
        let fir = make_fir("async fn m(clk: Clock<C>, x: In<u8, C>) {}");
        let ports = lower_ports(&fir).unwrap();
        assert!(ports.iter().all(|p| !matches!(p.direction, CHIRPortDir::Output)));
    }

    #[test]
    fn test_ports_in_out_ordering() {
        let fir = make_fir("async fn m(clk: Clock<C>, a: In<u8, C>, b: Out<u16, C>) {}");
        let ports = lower_ports(&fir).unwrap();
        assert!(matches!(ports[0].direction, CHIRPortDir::Input)); // clk
        assert!(matches!(ports[1].direction, CHIRPortDir::Input)); // a
        assert!(matches!(ports[2].direction, CHIRPortDir::Output)); // b
    }

    #[test]
    fn test_ports_in_bits_generic_parsed() {
        let fir = make_fir("async fn m(clk: Clock<C>, data: In<Bits<8>, C>) {}");
        let ports = lower_ports(&fir).unwrap();
        match &ports[1].kind {
            CHIRPortKind::Data { ty } => assert_eq!(*ty, CHIRType::UInt { width: Width::Concrete(8) }),
            _ => panic!("expected data port"),
        }
    }

    // ── Pattern parsing ──────────────────────────────────────────────────────

    #[test]
    fn test_parse_pattern_wildcard() {
        assert!(matches!(parse_pattern("_", span(), &EnumRegistry::new()).unwrap(), CHIRPattern::Wildcard));
    }

    #[test]
    fn test_parse_pattern_integer_literal() {
        match parse_pattern("0", span(), &EnumRegistry::new()).unwrap() {
            CHIRPattern::Lit(lit) => assert_eq!(lit.value, 0),
            _ => panic!("expected lit"),
        }
    }

    #[test]
    fn test_parse_pattern_integer_with_suffix() {
        match parse_pattern("42u8", span(), &EnumRegistry::new()).unwrap() {
            CHIRPattern::Lit(lit) => {
                assert_eq!(lit.value, 42);
                assert_eq!(lit.ty, CHIRType::UInt { width: Width::Concrete(8) });
            }
            _ => panic!("expected lit"),
        }
    }

    #[test]
    fn test_parse_pattern_bool_true() {
        assert!(matches!(
            parse_pattern("true", span(), &EnumRegistry::new()).unwrap(),
            CHIRPattern::Lit(CHIRLit { ty: CHIRType::Bool, value: 1 })
        ));
    }

    #[test]
    fn test_parse_pattern_bool_false() {
        assert!(matches!(
            parse_pattern("false", span(), &EnumRegistry::new()).unwrap(),
            CHIRPattern::Lit(CHIRLit { ty: CHIRType::Bool, value: 0 })
        ));
    }

    #[test]
    fn test_parse_pattern_tuple_two_elements() {
        match parse_pattern("(0 , 1)", span(), &EnumRegistry::new()).unwrap() {
            CHIRPattern::Tuple(parts) => {
                assert_eq!(parts.len(), 2);
                assert!(matches!(parts[0], CHIRPattern::Lit(_)));
                assert!(matches!(parts[1], CHIRPattern::Lit(_)));
            }
            _ => panic!("expected tuple"),
        }
    }

    #[test]
    fn test_parse_pattern_tuple_wildcard_elements() {
        match parse_pattern("(_, _)", span(), &EnumRegistry::new()).unwrap() {
            CHIRPattern::Tuple(parts) => {
                assert_eq!(parts.len(), 2);
                assert!(matches!(parts[0], CHIRPattern::Wildcard));
                assert!(matches!(parts[1], CHIRPattern::Wildcard));
            }
            _ => panic!("expected tuple"),
        }
    }

    #[test]
    fn test_parse_pattern_enum_variant_uppercase() {
        match parse_pattern("Ready", span(), &EnumRegistry::new()).unwrap() {
            CHIRPattern::EnumVariant { name, inner: None } => assert_eq!(name, "Ready"),
            _ => panic!("expected enum variant"),
        }
    }

    #[test]
    fn test_parse_pattern_lowercase_binding_is_wildcard() {
        assert!(matches!(parse_pattern("x", span(), &EnumRegistry::new()).unwrap(), CHIRPattern::Wildcard));
    }

    #[test]
    fn test_parse_or_patterns_single() {
        let patterns = parse_or_patterns("1", span(), &EnumRegistry::new()).unwrap();
        assert_eq!(patterns.len(), 1);
        assert!(matches!(patterns[0], CHIRPattern::Lit(CHIRLit { value: 1, .. })));
    }

    #[test]
    fn test_parse_or_patterns_two_alternatives() {
        let patterns = parse_or_patterns("1 | 2", span(), &EnumRegistry::new()).unwrap();
        assert_eq!(patterns.len(), 2);
        assert!(matches!(patterns[0], CHIRPattern::Lit(CHIRLit { value: 1, .. })));
        assert!(matches!(patterns[1], CHIRPattern::Lit(CHIRLit { value: 2, .. })));
    }

    #[test]
    fn test_parse_or_patterns_three_alternatives() {
        let patterns = parse_or_patterns("0 | 1 | 2", span(), &EnumRegistry::new()).unwrap();
        assert_eq!(patterns.len(), 3);
    }

    #[test]
    fn test_parse_or_patterns_wildcard_alone() {
        let patterns = parse_or_patterns("_", span(), &EnumRegistry::new()).unwrap();
        assert_eq!(patterns.len(), 1);
        assert!(matches!(patterns[0], CHIRPattern::Wildcard));
    }

    // ── Expression lowering ──────────────────────────────────────────────────

    fn lower(src: &str) -> Result<CHIRExpr, CHIRLowerError> {
        use crate::parser::capture_frontend_ir;
        let func_src = format!("fn __test__() {{ {}; }}", src);
        let design_fn: syn::ItemFn = syn::parse_str(&func_src).unwrap();
        let hw = no_hw();
        let fir = capture_frontend_ir(&design_fn, &hw).unwrap();
        let expr_stmt = match &fir.raw_statements[0].kind {
            RawStmtKind::Expr(e) => e.expr.clone(),
            _ => panic!("expected expr"),
        };
        let hw2 = no_hw();
        let reg = empty_registry();
        let mut ctx = LowerCtx::new(&hw2, &reg);
        lower_expr(&expr_stmt, &mut ctx)
    }

    #[test]
    fn test_lower_expr_integer_literal() {
        match lower("42").unwrap() {
            CHIRExpr::Lit(lit) => assert_eq!(lit.value, 42),
            _ => panic!("expected lit"),
        }
    }

    #[test]
    fn test_lower_expr_identifier() {
        match lower("count").unwrap() {
            CHIRExpr::Var(name) => assert_eq!(name, "count"),
            _ => panic!("expected var"),
        }
    }

    #[test]
    fn test_lower_expr_binary_add() {
        match lower("a + b").unwrap() {
            CHIRExpr::BinOp { op: CHIRBinOp::Add { wrapping: false }, .. } => {}
            _ => panic!("expected binop add"),
        }
    }

    #[test]
    fn test_lower_expr_binary_bitand() {
        match lower("a & b").unwrap() {
            CHIRExpr::BinOp { op: CHIRBinOp::BitAnd, .. } => {}
            _ => panic!("expected bitand"),
        }
    }

    #[test]
    fn test_lower_expr_binary_eq() {
        match lower("a == b").unwrap() {
            CHIRExpr::BinOp { op: CHIRBinOp::Eq, .. } => {}
            _ => panic!("expected eq"),
        }
    }

    #[test]
    fn test_lower_expr_unary_not() {
        match lower("!flag").unwrap() {
            CHIRExpr::UnOp { op: CHIRUnOp::LogicalNot, .. } => {}
            _ => panic!("expected unary not"),
        }
    }

    #[test]
    fn test_lower_expr_unary_neg() {
        match lower("-x").unwrap() {
            CHIRExpr::UnOp { op: CHIRUnOp::Neg, .. } => {}
            _ => panic!("expected neg"),
        }
    }

    #[test]
    fn test_lower_method_call_wrapping_add() {
        match lower("count.wrapping_add(step)").unwrap() {
            CHIRExpr::BinOp { op: CHIRBinOp::Add { wrapping: true }, .. } => {}
            _ => panic!("expected wrapping add"),
        }
    }

    #[test]
    fn test_lower_method_call_wrapping_sub() {
        match lower("x.wrapping_sub(1)").unwrap() {
            CHIRExpr::BinOp { op: CHIRBinOp::Sub { wrapping: true }, .. } => {}
            _ => panic!("expected wrapping sub"),
        }
    }

    #[test]
    fn test_lower_method_call_saturating_add_rejected() {
        assert!(matches!(
            lower("x.saturating_add(1)"),
            Err(CHIRLowerError::UnsupportedConstruct { .. })
        ));
    }

    #[test]
    fn test_lower_method_call_checked_add_rejected() {
        assert!(matches!(
            lower("x.checked_add(1)"),
            Err(CHIRLowerError::UnsupportedConstruct { .. })
        ));
    }

    // ── While loop rejection ─────────────────────────────────────────────────

    #[test]
    fn test_while_loop_in_seq_body_rejected() {
        let fir = make_fir(
            "async fn m(clk: Clock<C>) {
                while true { clk.tick().await; }
            }"
        );
        assert!(matches!(
            lower_seq_body(&fir, &no_hw(), &empty_registry()),
            Err(CHIRLowerError::UnsupportedConstruct { .. })
        ));
    }

    // ── Tick detection ───────────────────────────────────────────────────────

    #[test]
    fn test_is_tick_await_true_for_tick_method() {
        use copper_core::frontend_ir::{ExprMethodCall, ExprLit};
        let mc = ExprType::MethodCall(ExprMethodCall {
            receiver: Box::new(ExprType::Lit(ExprLit { text: "clk".to_string(), span: span() })),
            method: "tick".to_string(),
            args: vec![],
            turbofish: vec![],
            span: span(),
        });
        assert!(is_tick_await(&mc));
    }

    #[test]
    fn test_is_tick_await_false_for_other_method() {
        use copper_core::frontend_ir::{ExprMethodCall, ExprLit};
        let mc = ExprType::MethodCall(ExprMethodCall {
            receiver: Box::new(ExprType::Lit(ExprLit { text: "clk".to_string(), span: span() })),
            method: "reset".to_string(),
            args: vec![],
            turbofish: vec![],
            span: span(),
        });
        assert!(!is_tick_await(&mc));
    }

    // ── Sequential body lowering ─────────────────────────────────────────────

    #[test]
    fn test_seq_body_extracts_clock_name() {
        let fir = make_fir(
            "async fn counter(clk: Clock<MainClk>, data: In<u8, MainClk>) {
                let mut count: u8 = 0u8;
                loop { clk.tick().await; }
            }"
        );
        let body = lower_seq_body(&fir, &no_hw(), &empty_registry()).unwrap();
        assert_eq!(body.clock, "clk");
    }

    #[test]
    fn test_seq_body_detects_register_decl() {
        let fir = make_fir(
            // The local must be a REGISTER by the shared liveness rule
            // (defined in the loop, live across the tick) — a pre-loop
            // `let mut` the loop never touches is a constant wire now that
            // this arm consults `FrontendModuleIR::registers` instead of
            // deciding syntactically (re-blessed with the authority change).
            "async fn counter(clk: Clock<MainClk>, data: In<u8, MainClk>) {
                let mut count: u8 = 0u8;
                loop { count = count.wrapping_add(1u8); clk.tick().await; }
            }"
        );
        let body = lower_seq_body(&fir, &no_hw(), &empty_registry()).unwrap();
        assert_eq!(body.registers.len(), 1);
        assert_eq!(body.registers[0].name, "count");
        assert_eq!(body.registers[0].ty, CHIRType::UInt { width: Width::Concrete(8) });
    }

    #[test]
    fn test_seq_body_loop_contains_await_tick() {
        let fir = make_fir(
            "async fn counter(clk: Clock<MainClk>, data: In<u8, MainClk>) {
                let mut count: u8 = 0u8;
                loop {
                    count = count;
                    clk.tick().await;
                }
            }"
        );
        let body = lower_seq_body(&fir, &no_hw(), &empty_registry()).unwrap();
        let has_tick = body.loop_body.iter().any(|s| matches!(s, CHIRStmt::AwaitTick { .. }));
        assert!(has_tick);
    }

    #[test]
    fn test_seq_body_missing_loop_returns_error() {
        let fir = make_fir(
            "async fn counter(clk: Clock<MainClk>) {
                let mut count: u8 = 0u8;
            }"
        );
        assert!(matches!(
            lower_seq_body(&fir, &no_hw(), &empty_registry()),
            Err(CHIRLowerError::UnsupportedConstruct { .. })
        ));
    }

    #[test]
    fn test_seq_body_missing_tick_returns_error() {
        let fir = make_fir(
            "async fn counter(clk: Clock<MainClk>) {
                loop {
                    let x: u8 = 0u8;
                }
            }"
        );
        assert!(matches!(
            lower_seq_body(&fir, &no_hw(), &empty_registry()),
            Err(CHIRLowerError::UnsupportedConstruct { .. })
        ));
    }

    #[test]
    fn test_seq_body_multiple_ticks_all_captured() {
        let fir = make_fir(
            "async fn two_phase(clk: Clock<MainClk>) {
                loop {
                    clk.tick().await;
                    clk.tick().await;
                }
            }"
        );
        let body = lower_seq_body(&fir, &no_hw(), &empty_registry()).unwrap();
        let tick_count = body.loop_body.iter()
            .filter(|s| matches!(s, CHIRStmt::AwaitTick { .. }))
            .count();
        assert_eq!(tick_count, 2);
    }

    // ── Full module lowering ─────────────────────────────────────────────────

    #[test]
    fn test_lower_to_chir_sequential_module_name() {
        let fir = make_fir(
            "async fn my_counter(clk: Clock<MainClk>, step: In<u8, MainClk>) {
                let mut count: u8 = 0u8;
                loop { clk.tick().await; }
            }"
        );
        let module = lower_to_chir(&fir, &no_hw(), &empty_registry()).unwrap();
        assert_eq!(module.name, "my_counter");
    }

    #[test]
    fn test_lower_to_chir_combinational_classifies_correctly() {
        let fir = make_fir("fn add(a: u8, b: u8) -> u8 { a }");
        let module = lower_to_chir(&fir, &no_hw(), &empty_registry()).unwrap();
        assert!(matches!(module.body, CHIRBody::Combinational(_)));
    }

    #[test]
    fn test_lower_to_chir_sequential_classifies_correctly() {
        let fir = make_fir(
            "async fn counter(clk: Clock<MainClk>, x: u8) {
                loop { clk.tick().await; }
            }"
        );
        let module = lower_to_chir(&fir, &no_hw(), &empty_registry()).unwrap();
        assert!(matches!(module.body, CHIRBody::Sequential(_)));
    }

    // ── Scope validation ─────────────────────────────────────────────────────

    #[test]
    fn test_validate_undefined_variable_returns_error() {
        use copper_core::chir::CHIRCombBody;
        let module = CHIRModule {
            name: "test".to_string(),
            params: vec![],
            localparams: vec![],
            ports: vec![],
            body: CHIRBody::Combinational(CHIRCombBody {
                submodules: vec![],
                stmts: vec![CHIRStmt::PortWrite {
                    port_name: "out".to_string(),
                    value: CHIRExpr::Var("undefined_var".to_string()),
                    span: span(),
                }],
            }),
            span: span(),
        };
        assert!(matches!(
            validate_module_scope(&module),
            Err(CHIRLowerError::UnsupportedConstruct { .. })
        ));
    }

    #[test]
    fn test_validate_port_reference_is_ok() {
        use copper_core::chir::CHIRCombBody;
        let module = CHIRModule {
            name: "test".to_string(),
            params: vec![],
            localparams: vec![],
            ports: vec![
                CHIRPort {
                    name: "a".to_string(),
                    direction: CHIRPortDir::Input,
                    kind: CHIRPortKind::Data { ty: CHIRType::UInt { width: Width::Concrete(8) } },
                    registered: false,
                    span: span(),
                },
                CHIRPort {
                    name: "out".to_string(),
                    direction: CHIRPortDir::Output,
                    kind: CHIRPortKind::Data { ty: CHIRType::UInt { width: Width::Concrete(8) } },
                    registered: false,
                    span: span(),
                },
            ],
            body: CHIRBody::Combinational(CHIRCombBody {
                submodules: vec![],
                stmts: vec![CHIRStmt::PortWrite {
                    port_name: "out".to_string(),
                    value: CHIRExpr::Var("a".to_string()),
                    span: span(),
                }],
            }),
            span: span(),
        };
        assert!(validate_module_scope(&module).is_ok());
    }

    #[test]
    fn test_validate_wire_reference_after_declaration_is_ok() {
        use copper_core::chir::CHIRCombBody;
        let module = CHIRModule {
            name: "test".to_string(),
            params: vec![],
            localparams: vec![],
            ports: vec![
                CHIRPort {
                    name: "a".to_string(),
                    direction: CHIRPortDir::Input,
                    kind: CHIRPortKind::Data { ty: CHIRType::UInt { width: Width::Concrete(8) } },
                    registered: false,
                    span: span(),
                },
                CHIRPort {
                    name: "out".to_string(),
                    direction: CHIRPortDir::Output,
                    kind: CHIRPortKind::Data { ty: CHIRType::UInt { width: Width::Concrete(8) } },
                    registered: false,
                    span: span(),
                },
            ],
            body: CHIRBody::Combinational(CHIRCombBody {
                submodules: vec![],
                stmts: vec![
                    CHIRStmt::Wire {
                        name: "doubled".to_string(),
                        ty: CHIRType::UInt { width: Width::Concrete(8) },
                        value: CHIRExpr::BinOp {
                            left: Box::new(CHIRExpr::Var("a".to_string())),
                            op: CHIRBinOp::Add { wrapping: false },
                            right: Box::new(CHIRExpr::Var("a".to_string())),
                        },
                        span: span(),
                    },
                    CHIRStmt::PortWrite {
                        port_name: "out".to_string(),
                        value: CHIRExpr::Var("doubled".to_string()),
                        span: span(),
                    },
                ],
            }),
            span: span(),
        };
        assert!(validate_module_scope(&module).is_ok());
    }

    // ── Module registry / hardware call ──────────────────────────────────────

    #[test]
    fn test_hardware_call_uses_registry_port_names() {
        // Define the callee FIR
        let callee_fir = make_fir("fn full_adder(a: u8, b: u8) -> u8 { a }");
        let mut registry = ModuleRegistry::new();
        registry.insert("full_adder".to_string(), callee_fir);

        // Caller that uses full_adder
        let caller_fir = make_fir_hw(
            "fn adder_top(x: u8, y: u8) -> u8 { full_adder(x, y) }",
            &hw(&["full_adder"]),
        );

        let module = lower_to_chir(&caller_fir, &hw(&["full_adder"]), &registry).unwrap();
        if let CHIRBody::Combinational(body) = &module.body {
            assert_eq!(body.submodules.len(), 1);
            let sub = &body.submodules[0];
            assert_eq!(sub.module_name, "full_adder");
            // Port names from registry (skip clock params — none here)
            assert_eq!(sub.inputs[0].0, "a");
            assert_eq!(sub.inputs[1].0, "b");
            // Output type from callee return type (u8)
            assert_eq!(sub.output_ty, CHIRType::UInt { width: Width::Concrete(8) });
        } else {
            panic!("expected combinational body");
        }
    }

    #[test]
    fn test_hardware_call_fallback_to_positional_when_not_in_registry() {
        let caller_fir = make_fir_hw(
            "fn top(x: u8) -> u8 { unknown_module(x) }",
            &hw(&["unknown_module"]),
        );
        let module = lower_to_chir(&caller_fir, &hw(&["unknown_module"]), &empty_registry()).unwrap();
        if let CHIRBody::Combinational(body) = &module.body {
            assert_eq!(body.submodules[0].inputs[0].0, "arg0");
        } else {
            panic!("expected combinational body");
        }
    }

    #[test]
    fn test_hardware_call_output_type_from_registry() {
        let callee_fir = make_fir("fn adder(a: u16, b: u16) -> u16 { a }");
        let mut registry = ModuleRegistry::new();
        registry.insert("adder".to_string(), callee_fir);

        let caller_fir = make_fir_hw(
            "fn top(x: u16, y: u16) -> u16 { adder(x, y) }",
            &hw(&["adder"]),
        );
        let module = lower_to_chir(&caller_fir, &hw(&["adder"]), &registry).unwrap();
        if let CHIRBody::Combinational(body) = &module.body {
            assert_eq!(body.submodules[0].output_ty, CHIRType::UInt { width: Width::Concrete(16) });
        } else {
            panic!("expected combinational body");
        }
    }

    // ── Integer / ident helpers ──────────────────────────────────────────────

    #[test]
    fn test_is_ident_valid() {
        assert!(is_ident("foo"));
        assert!(is_ident("_bar"));
        assert!(is_ident("count_0"));
    }

    #[test]
    fn test_is_ident_invalid() {
        assert!(!is_ident("42"));
        assert!(!is_ident(""));
        assert!(!is_ident("a b"));
        assert!(!is_ident("a+b"));
    }

    #[test]
    fn test_parse_int_literal_decimal() {
        assert_eq!(parse_int_literal("42"),    Some((42,  None)));
        assert_eq!(parse_int_literal("0"),     Some((0,   None)));
        assert_eq!(parse_int_literal("255"),   Some((255, None)));
    }

    #[test]
    fn test_parse_int_literal_with_suffix() {
        assert_eq!(parse_int_literal("42u8"),  Some((42,  Some(8))));
        assert_eq!(parse_int_literal("10u32"), Some((10,  Some(32))));
    }

    #[test]
    fn test_parse_int_literal_hex() {
        assert_eq!(parse_int_literal("0xFF"), Some((255, None)));
        assert_eq!(parse_int_literal("0x10"), Some((16,  None)));
    }

    #[test]
    fn test_parse_int_literal_binary() {
        assert_eq!(parse_int_literal("0b1010"), Some((10, None)));
    }

    #[test]
    fn test_parse_int_literal_non_integer() {
        assert_eq!(parse_int_literal("foo"), None);
        assert_eq!(parse_int_literal(""),    None);
    }

    #[test]
    fn test_split_top_level_commas_flat() {
        assert_eq!(split_top_level_commas("a, b, c"), vec!["a", " b", " c"]);
    }

    #[test]
    fn test_split_top_level_commas_nested() {
        let parts = split_top_level_commas("a, (b, c), d");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1].trim(), "(b, c)");
    }

    // ── End-to-end tests ─────────────────────────────────────────────────────

    /// Counter module: sequential, one register, wrapping increment, tick boundary.
    #[test]
    fn test_e2e_counter_module() {
        let fir = make_fir(
            "async fn counter(clk: Clock<MainClk>) {
                let mut count: u8 = 0u8;
                loop {
                    count = count.wrapping_add(1u8);
                    clk.tick().await;
                }
            }"
        );
        let module = lower_to_chir(&fir, &no_hw(), &empty_registry()).unwrap();

        assert_eq!(module.name, "counter");
        assert_eq!(module.ports.len(), 1); // just clk
        assert!(matches!(module.ports[0].kind, CHIRPortKind::Clock { .. }));

        if let CHIRBody::Sequential(body) = &module.body {
            assert_eq!(body.clock, "clk");
            assert_eq!(body.registers.len(), 1);
            assert_eq!(body.registers[0].name, "count");
            assert_eq!(body.registers[0].ty, CHIRType::UInt { width: Width::Concrete(8) });
            // Init value: 0u8
            assert_eq!(body.registers[0].init.as_ref().unwrap().value, 0);

            // Loop body: Assign then AwaitTick
            assert_eq!(body.loop_body.len(), 2);
            assert!(matches!(body.loop_body[0], CHIRStmt::Assign { .. }));
            assert!(matches!(body.loop_body[1], CHIRStmt::AwaitTick { .. }));

            // Assign value should be wrapping_add (BinOp with wrapping=true)
            if let CHIRStmt::Assign { value, .. } = &body.loop_body[0] {
                assert!(matches!(value, CHIRExpr::BinOp { op: CHIRBinOp::Add { wrapping: true }, .. }));
            }
        } else {
            panic!("expected sequential body");
        }
    }

    /// Combinational adder: two inputs, one output, simple add expression.
    #[test]
    fn test_e2e_combinational_adder() {
        let fir = make_fir("fn add(a: u8, b: u8) -> u8 { a + b }");
        let module = lower_to_chir(&fir, &no_hw(), &empty_registry()).unwrap();

        assert_eq!(module.name, "add");
        assert_eq!(module.ports.len(), 3); // a, b, out

        let port_names: Vec<_> = module.ports.iter().map(|p| p.name.as_str()).collect();
        assert!(port_names.contains(&"a"));
        assert!(port_names.contains(&"b"));
        assert!(port_names.contains(&"out"));

        if let CHIRBody::Combinational(body) = &module.body {
            // Final expression `a + b` becomes PortWrite to "out"
            assert_eq!(body.stmts.len(), 1);
            if let CHIRStmt::PortWrite { port_name, value, .. } = &body.stmts[0] {
                assert_eq!(port_name, "out");
                assert!(matches!(value, CHIRExpr::BinOp { op: CHIRBinOp::Add { .. }, .. }));
            } else {
                panic!("expected PortWrite");
            }
        } else {
            panic!("expected combinational body");
        }
    }

    /// Hardware call with full_adder submodule: checks submodule instantiation
    /// and named port connections using the registry.
    #[test]
    fn test_e2e_hardware_call_with_registry() {
        let adder_fir = make_fir("fn full_adder(a: u8, b: u8) -> u8 { a + b }");
        let mut registry = ModuleRegistry::new();
        registry.insert("full_adder".to_string(), adder_fir);

        let top_fir = make_fir_hw(
            "fn top(x: u8, y: u8) -> u8 { full_adder(x, y) }",
            &hw(&["full_adder"]),
        );
        let module = lower_to_chir(&top_fir, &hw(&["full_adder"]), &registry).unwrap();

        assert_eq!(module.name, "top");

        if let CHIRBody::Combinational(body) = &module.body {
            assert_eq!(body.submodules.len(), 1);
            let sub = &body.submodules[0];
            assert_eq!(sub.module_name, "full_adder");
            assert_eq!(sub.inst_name, "full_adder_0");
            assert_eq!(sub.inputs.len(), 2);
            assert_eq!(sub.inputs[0].0, "a");
            assert_eq!(sub.inputs[1].0, "b");
            assert_eq!(sub.output_ty, CHIRType::UInt { width: Width::Concrete(8) });

            // The PortWrite drives "out" with the submodule output wire
            if let CHIRStmt::PortWrite { port_name, value, .. } = body.stmts.last().unwrap() {
                assert_eq!(port_name, "out");
                assert!(matches!(value, CHIRExpr::Var(name) if name == "full_adder_0_out"));
            } else {
                panic!("expected PortWrite");
            }
        } else {
            panic!("expected combinational body");
        }
    }

    /// Sequential module with conditional: if/else on a register value.
    #[test]
    fn test_e2e_sequential_with_conditional() {
        let fir = make_fir(
            "async fn saturating_counter(clk: Clock<MainClk>, max: u8) {
                let mut count: u8 = 0u8;
                loop {
                    if count < max {
                        count = count + 1u8;
                    }
                    clk.tick().await;
                }
            }"
        );
        let module = lower_to_chir(&fir, &no_hw(), &empty_registry()).unwrap();

        if let CHIRBody::Sequential(body) = &module.body {
            assert_eq!(body.registers.len(), 1);
            // Loop: If, AwaitTick
            assert!(body.loop_body.iter().any(|s| matches!(s, CHIRStmt::If { .. })));
            assert!(body.loop_body.iter().any(|s| matches!(s, CHIRStmt::AwaitTick { .. })));
        } else {
            panic!("expected sequential body");
        }
    }

    // ── Free-function inlining (#7b) ─────────────────────────────────────────

    fn capture_fns(file_src: &str) -> Vec<FrontendFnIR> {
        let file: syn::File = syn::parse_str(file_src).unwrap();
        crate::parser::capture_file_scope(&file, &no_hw()).fns
    }

    #[test]
    fn inline_single_expr_fn_substitutes_param() {
        use copper_core::frontend_ir::{ExprPath, ExprType};
        // add_one(a) where body is `x + one` → `a + one`.
        let fns = capture_fns("fn add_one(x: Bits<8>) -> Bits<8> { x + one }");
        let arg = ExprType::Path(ExprPath { path_text: "a".into(), span: span() });
        let inlined = build_inlined_expr(&fns[0], &[arg], span()).unwrap();
        match inlined {
            ExprType::Binary(b) => {
                assert!(matches!(*b.left, ExprType::Path(p) if p.path_text == "a"));
                assert_eq!(b.op, "+");
                assert!(matches!(*b.right, ExprType::Path(p) if p.path_text == "one"));
            }
            other => panic!("expected binary, got {other:?}"),
        }
    }

    #[test]
    fn inline_let_tail_fn_folds_bindings() {
        use copper_core::frontend_ir::{ExprPath, ExprType};
        // f(z) with body `let b = a + one; b + two` → `(z + one) + two`.
        let fns = capture_fns("fn f(a: Bits<8>) -> Bits<8> { let b = a + one; b + two }");
        let arg = ExprType::Path(ExprPath { path_text: "z".into(), span: span() });
        let inlined = build_inlined_expr(&fns[0], &[arg], span()).unwrap();
        match inlined {
            ExprType::Binary(outer) => {
                assert_eq!(outer.op, "+");
                match *outer.left {
                    ExprType::Binary(inner) => {
                        assert!(matches!(*inner.left, ExprType::Path(p) if p.path_text == "z"));
                        assert!(matches!(*inner.right, ExprType::Path(p) if p.path_text == "one"));
                    }
                    other => panic!("expected inner binary (z + one), got {other:?}"),
                }
                assert!(matches!(*outer.right, ExprType::Path(p) if p.path_text == "two"));
            }
            other => panic!("expected binary, got {other:?}"),
        }
    }

    #[test]
    fn inline_arg_count_mismatch_is_rejected() {
        let fns = capture_fns("fn f(a: Bits<8>, b: Bits<8>) -> Bits<8> { a + b }");
        assert!(matches!(
            build_inlined_expr(&fns[0], &[], span()),
            Err(CHIRLowerError::UnsupportedConstruct { .. })
        ));
    }

    #[test]
    fn inline_non_let_statement_is_rejected() {
        use copper_core::frontend_ir::{ExprPath, ExprType};
        // A `;`-terminated statement before the tail can't fold into one expression.
        let fns = capture_fns("fn f(a: Bits<8>) -> Bits<8> { a.touch(); a }");
        let arg = ExprType::Path(ExprPath { path_text: "z".into(), span: span() });
        assert!(matches!(
            build_inlined_expr(&fns[0], &[arg], span()),
            Err(CHIRLowerError::UnsupportedConstruct { .. })
        ));
    }

    #[test]
    fn inline_end_to_end_lowers_helper_into_combinational_module() {
        // The module calls a file-scope helper; the call must inline and lower.
        let module = "#[hardware(combinational)] \
                      fn m(a: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) { \
                          out.write(add_one(a.read())); \
                      }";
        let mut fir = make_fir_hw(module, &no_hw());
        fir.file_fns = capture_fns("fn add_one(x: Bits<8>) -> Bits<8> { x + Bits::<8>::from_lit::<1>() }");
        let chir = lower_to_chir(&fir, &no_hw(), &empty_registry())
            .expect("module with an inlined helper call should lower");
        assert!(matches!(chir.body, CHIRBody::Combinational(_)));
    }

    #[test]
    fn inline_unknown_fn_still_errors() {
        // A call to a function that isn't a captured file_fn stays unsupported.
        let module = "#[hardware(combinational)] \
                      fn m(a: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) { \
                          out.write(mystery(a.read())); \
                      }";
        let fir = make_fir_hw(module, &no_hw()); // no file_fns injected
        assert!(lower_to_chir(&fir, &no_hw(), &empty_registry()).is_err());
    }

    // ── Impl-method (associated fn) inlining (#7b increment 2) ────────────────

    fn capture_impls(file_src: &str) -> Vec<copper_core::frontend_ir::FrontendImplIR> {
        let file: syn::File = syn::parse_str(file_src).unwrap();
        crate::parser::capture_file_scope(&file, &no_hw()).impls
    }

    #[test]
    fn fn_registry_keys_assoc_fns_by_qualified_name() {
        let mut fir = make_fir_hw(
            "#[hardware(combinational)] fn m(a: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) {}",
            &no_hw(),
        );
        fir.file_impls = capture_impls("impl Opcode { fn from_bits(op: Bits<8>) -> Bits<8> { op } }");
        let reg = build_fn_registry(&fir);
        // Reachable under the same `Type::method` path a call site uses.
        assert!(reg.contains_key("Opcode::from_bits"));
        assert!(!reg.contains_key("from_bits"));
    }

    #[test]
    fn inline_impl_associated_fn_end_to_end() {
        // `Doubler::double(a.read())` inlines its impl-block associated fn.
        let module = "#[hardware(combinational)] \
                      fn m(a: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) { \
                          out.write(Doubler::double(a.read())); \
                      }";
        let mut fir = make_fir_hw(module, &no_hw());
        fir.file_impls = capture_impls("impl Doubler { fn double(x: Bits<8>) -> Bits<8> { x + x } }");
        let chir = lower_to_chir(&fir, &no_hw(), &empty_registry())
            .expect("associated-fn call should inline and lower");
        assert!(matches!(chir.body, CHIRBody::Combinational(_)));
    }

    #[test]
    fn instance_methods_are_not_registered_for_inlining() {
        // A `&self` method is not an associated fn; it is not (yet) inlinable.
        let mut fir = make_fir_hw(
            "#[hardware(combinational)] fn m(a: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) {}",
            &no_hw(),
        );
        fir.file_impls = capture_impls("impl Foo { fn get(&self) -> Bits<8> { x } }");
        let reg = build_fn_registry(&fir);
        assert!(!reg.contains_key("Foo::get"));
        assert!(!reg.contains_key("get"));
    }

    // ── control-extraction guard (no silent miscompile) ──────────────────────

    #[test]
    fn tick_inside_for_loop_is_rejected_not_dropped() {
        // A counted delay (`for _ in 0..4 { clk.tick().await }`) needs control
        // extraction. Until that exists it must be rejected, never silently
        // dropped (which would emit hardware missing the delay).
        let module = "#[hardware(sequential)] \
                      async fn m(clk: Clock<MainClk>, d: In<Bits<8>, MainClk>, out: Out<Bits<8>, MainClk>) { \
                          loop { \
                              for _ in 0..4 { clk.tick().await; } \
                              out.write(d.read()); \
                              clk.tick().await; \
                          } \
                      }";
        let fir = make_fir_hw(module, &no_hw());
        let err = lower_to_chir(&fir, &no_hw(), &empty_registry());
        assert!(err.is_err(), "tick inside a for-loop must be rejected");
    }

    // ── forward width-inference ───────────────────────────────────────────────

    #[test]
    fn forward_inference_from_port_write() {
        // `let mut acc = Bits::zero()` has no annotation and an ambiguous width;
        // it is inferred from `out.write(acc)` where `out` is `Bits<8>`.
        let module = "#[hardware(combinational)] \
                      fn m(a: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) { \
                          let mut acc = Bits::zero(); \
                          acc = a.read(); \
                          out.write(acc); \
                      }";
        let fir = make_fir_hw(module, &no_hw());
        assert!(lower_to_chir(&fir, &no_hw(), &empty_registry()).is_ok());
    }

    #[test]
    fn forward_inference_propagates_across_assignment() {
        // `tmp`'s width isn't set by a port write; it comes from `r = tmp`, and
        // `r` from `out.write(r)`.
        let module = "#[hardware(combinational)] \
                      fn m(a: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) { \
                          let mut r = Bits::zero(); \
                          let mut tmp = Bits::zero(); \
                          tmp = a.read(); \
                          r = tmp; \
                          out.write(r); \
                      }";
        let fir = make_fir_hw(module, &no_hw());
        assert!(lower_to_chir(&fir, &no_hw(), &empty_registry()).is_ok());
    }

    // ── for-loop lowering ─────────────────────────────────────────────────────

    #[test]
    fn for_loop_lowers_to_chir_forloop_over_param_bound() {
        // `for i in 0..N { .. }` → CHIRStmt::ForLoop with the param bound and the
        // loop variable in scope for the (non-empty) body.
        let module = "#[hardware(combinational)] \
                      fn m<const N: usize>(a: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) { \
                          let mut acc: Bits<8> = a.read(); \
                          for i in 0..N { acc = acc + Bits::from_u8(1); } \
                          out.write(acc); \
                      }";
        let fir = make_fir_hw(module, &no_hw());
        let chir = lower_to_chir(&fir, &no_hw(), &empty_registry()).expect("lowers");
        let CHIRBody::Combinational(b) = chir.body else { panic!("expected comb body") };
        let forloop = b.stmts.iter().find_map(|s| match s {
            CHIRStmt::ForLoop { var, end, body, .. } => Some((var.clone(), end.clone(), body.len())),
            _ => None,
        });
        let (var, end, body_len) = forloop.expect("a ForLoop stmt");
        assert_eq!(var, "i");
        // Bound `N` is a module parameter reference.
        assert!(matches!(end, CHIRExpr::Var(ref n) if n == "N"), "end should be Var(N), got {end:?}");
        assert!(body_len >= 1, "loop body should be captured");
    }

    // ── const-block elision ───────────────────────────────────────────────────

    #[test]
    fn const_block_assertion_is_elided() {
        // `const { assert!(..) }` is a compile-time check; it lowers to nothing
        // and does not block the module.
        let module = "#[hardware(combinational)] \
                      fn m(a: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) { \
                          const { assert!(8 == 8, \"width\") }; \
                          out.write(a.read()); \
                      }";
        let fir = make_fir_hw(module, &no_hw());
        let chir = lower_to_chir(&fir, &no_hw(), &empty_registry())
            .expect("const-block should be elided, not block lowering");
        // The body has exactly the port write — the const block produced no stmt.
        if let CHIRBody::Combinational(b) = chir.body {
            assert_eq!(
                b.stmts.iter().filter(|s| matches!(s, CHIRStmt::PortWrite { .. })).count(),
                1
            );
        } else {
            panic!("expected combinational body");
        }
    }

    // ── Struct lowering (milestone 2) ─────────────────────────────────────────

    fn capture_structs(file_src: &str) -> Vec<ItemStruct> {
        let file: syn::File = syn::parse_str(file_src).unwrap();
        crate::parser::capture_file_scope(&file, &no_hw()).structs
    }

    /// Collect the wire names a combinational body declares, in order.
    fn comb_wire_names(fir: &FrontendModuleIR) -> Vec<String> {
        let chir = lower_to_chir(fir, &no_hw(), &empty_registry()).expect("lowers");
        match chir.body {
            CHIRBody::Combinational(b) => b
                .stmts
                .iter()
                .filter_map(|s| match s {
                    CHIRStmt::Wire { name, .. } => Some(name.clone()),
                    _ => None,
                })
                .collect(),
            _ => panic!("expected combinational body"),
        }
    }

    #[test]
    fn struct_binding_emits_one_wire_per_field() {
        // `let p = Point { x: a.read(), y: b.read() }` → wires `p_x`, `p_y`.
        let module = "#[hardware(combinational)] \
                      fn m(a: In<Bits<8>, ()>, b: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) { \
                          let p = Point { x: a.read(), y: b.read() }; \
                          out.write(p.x); \
                      }";
        let mut fir = make_fir_hw(module, &no_hw());
        fir.file_structs = capture_structs("struct Point { x: Bits<8>, y: Bits<8> }");
        let wires = comb_wire_names(&fir);
        assert!(wires.contains(&"p_x".to_string()), "wires: {wires:?}");
        assert!(wires.contains(&"p_y".to_string()), "wires: {wires:?}");
    }

    #[test]
    fn struct_field_access_reads_the_field_wire() {
        // `out.write(p.y)` must drive from wire `p_y`, not a scalar `p`.
        let module = "#[hardware(combinational)] \
                      fn m(a: In<Bits<8>, ()>, b: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) { \
                          let p = Point { x: a.read(), y: b.read() }; \
                          out.write(p.y); \
                      }";
        let mut fir = make_fir_hw(module, &no_hw());
        fir.file_structs = capture_structs("struct Point { x: Bits<8>, y: Bits<8> }");
        let chir = lower_to_chir(&fir, &no_hw(), &empty_registry()).expect("lowers");
        let drives_p_y = match chir.body {
            CHIRBody::Combinational(b) => b.stmts.iter().any(|s| matches!(
                s, CHIRStmt::PortWrite { value: CHIRExpr::Var(v), .. } if v == "p_y")),
            _ => false,
        };
        assert!(drives_p_y, "expected out driven from p_y");
    }

    #[test]
    fn struct_returning_helper_inlines_into_field_wires() {
        // `let p = make(a.read())` where `make` returns a `Point { .. }` literal
        // must still produce per-field wires (inline-through-call).
        let module = "#[hardware(combinational)] \
                      fn m(a: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) { \
                          let p = make(a.read()); \
                          out.write(p.x); \
                      }";
        let mut fir = make_fir_hw(module, &no_hw());
        fir.file_structs = capture_structs("struct Point { x: Bits<8>, y: Bits<8> }");
        fir.file_fns =
            capture_fns("fn make(v: Bits<8>) -> Point { Point { x: v, y: v } }");
        let wires = comb_wire_names(&fir);
        assert!(wires.contains(&"p_x".to_string()), "wires: {wires:?}");
        assert!(wires.contains(&"p_y".to_string()), "wires: {wires:?}");
    }

    // ── Match-as-value (match in expression position) ─────────────────────────

    #[test]
    fn match_as_value_alu_shape_lowers() {
        // alu_exec-shaped: `let r = match (tuple) { partial-wildcard arms }` with
        // value arms, one an if-expression, and a trailing `_` over the tuple.
        let module = "#[hardware(combinational)] \
                      fn m(a: In<Bits<8>, ()>, b: In<Bits<8>, ()>, sel: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) { \
                          let r: Bits<8> = match (sel.read().as_usize(), a.read().as_usize()) { \
                              (0, 0) => a.read(), \
                              (1, _) => b.read(), \
                              (2, _) => if a.read() == b.read() { a.read() } else { b.read() }, \
                              _      => Bits::from_lit::<0>(), \
                          }; \
                          out.write(r); \
                      }";
        let fir = make_fir_hw(module, &no_hw());
        let res = lower_to_chir(&fir, &no_hw(), &empty_registry());
        assert!(res.is_ok(), "alu-shaped match-as-value should lower: {:?}", res.err());
    }

    #[test]
    fn match_as_value_lone_wildcard_matches_tuple_scrutinee() {
        // A bare `_` arm matches a tuple scrutinee regardless of arity. The
        // partial-wildcard arm `(0, _)` forces the Mux-chain path (not `case`),
        // so this exercises the arity fix directly: `_` must not be rejected on an
        // element-count check, and the value lowers to a `Mux` chain.
        let module = "#[hardware(combinational)] \
                      fn m(a: In<Bits<8>, ()>, b: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) { \
                          let r: Bits<8> = match (a.read().as_usize(), b.read().as_usize()) { \
                              (0, _) => a.read(), \
                              _      => b.read(), \
                          }; \
                          out.write(r); \
                      }";
        let fir = make_fir_hw(module, &no_hw());
        let chir = lower_to_chir(&fir, &no_hw(), &empty_registry()).expect("lowers");
        // The match binds `let r`, so the Mux is the value of wire `r`.
        let r_is_mux = match chir.body {
            CHIRBody::Combinational(b) => b.stmts.iter().any(|s| matches!(
                s, CHIRStmt::Wire { name, value: CHIRExpr::Mux { .. }, .. } if name == "r")),
            _ => false,
        };
        assert!(r_is_mux, "expected wire `r` to be a Mux chain");
    }

    #[test]
    fn struct_field_type_falls_back_to_enum_width() {
        // A field typed as an enum resolves to the enum's encoding width.
        let module = "#[hardware(combinational)] \
                      fn m(a: In<Bits<8>, ()>, out: Out<Bits<8>, ()>) { \
                          let p = Wrap { inner: a.read() }; \
                          out.write(p.inner); \
                      }";
        let mut fir = make_fir_hw(module, &no_hw());
        fir.file_structs = capture_structs("struct Wrap { inner: Bits<8> }");
        // Should lower without an ambiguous-width error.
        assert!(lower_to_chir(&fir, &no_hw(), &empty_registry()).is_ok());
    }
}
