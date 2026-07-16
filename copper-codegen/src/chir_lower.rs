use copper_core::chir::{
    CHIRBinOp, CHIRBody, CHIRCaseArm, CHIRCombBody, CHIRExpr, CHIRLit, CHIRLowerError,
    CHIRMatchArm, CHIRModule, CHIRPattern, CHIRPort, CHIRPortDir, CHIRPortKind, CHIRRegDecl,
    CHIRSeqBody, CHIRStmt, CHIRSubmoduleInst, CHIRType, CHIRUnOp, Width,
};
use copper_core::frontend_ir::{
    ExprCall, ExprIndex, ExprType, FrontendClassification, FrontendModuleIR, RawStmt, RawStmtKind,
    SourceSpan,
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
    };

    let module = CHIRModule {
        name: fir.module_name.clone(),
        params: Vec::new(),
        ports,
        body,
        span: fir.span,
    };

    validate_module(&module)?;

    Ok(module)
}

// ── Type resolution ───────────────────────────────────────────────────────────

/// Resolve a raw Copper type text to a `CHIRType`.
pub fn resolve_type(ty_text: &str, span: SourceSpan) -> Result<CHIRType, CHIRLowerError> {
    let compact: String = ty_text.chars().filter(|c| !c.is_whitespace()).collect();

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
        "bool" => Ok(CHIRType::Bool),
        "Bit"  => Ok(CHIRType::UInt { width: Width::Concrete(1) }),
        "Logic" => Ok(CHIRType::UInt { width: Width::Concrete(1) }),
        _ if compact.starts_with("Bits<") => parse_bits_type(&compact, span),
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
    match inner.and_then(|s| s.parse::<usize>().ok()) {
        Some(width) => Ok(CHIRType::UInt { width: Width::Concrete(width) }),
        None => Err(CHIRLowerError::UnresolvableType {
            ty_text: compact.to_string(),
            span,
        }),
    }
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
) -> Result<CHIRType, CHIRLowerError> {
    match expr {
        ExprType::Lit(lit) => {
            let compact: String = lit.text.chars().filter(|c| !c.is_whitespace()).collect();
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
            // A bare identifier: resolve via the symbol table.
            if is_ident(&compact) {
                if let Some(ty) = symbols.get(&compact) {
                    return Ok(ty.clone());
                }
            }
            Err(CHIRLowerError::AmbiguousWidth { span })
        }
        ExprType::Cast(cast) => resolve_type(&cast.target_ty.ty_text, cast.target_ty.span),
        ExprType::Reference(r) => infer_type_from_expr(&r.expr, span, symbols),
        ExprType::Unary(un) => infer_type_from_expr(&un.expr, span, symbols),
        ExprType::Binary(bin) => {
            if is_comparison_or_logical_op(&bin.op) {
                Ok(CHIRType::Bool)
            } else {
                // Width follows the operands; try left, then right.
                infer_type_from_expr(&bin.left, span, symbols)
                    .or_else(|_| infer_type_from_expr(&bin.right, span, symbols))
            }
        }
        ExprType::MethodCall(mc) => match mc.method.as_str() {
            // `.read()` on a port, and wrapping/lock/unwrap/clone wrappers, all
            // carry the width of their receiver.
            "read" | "lock" | "unwrap" | "clone" | "wrapping_add" | "wrapping_sub"
            | "wrapping_mul" => infer_type_from_expr(&mc.receiver, span, symbols),
            "as_bool" => Ok(CHIRType::Bool),
            _ => infer_type_from_expr(&mc.receiver, span, symbols),
        },
        ExprType::Call(call) => infer_type_from_call(call, span),
        // A single-bit index `x[i]` is 1-bit.
        ExprType::Index(_) => Ok(CHIRType::UInt { width: Width::Concrete(1) }),
        ExprType::If(if_expr) => match &if_expr.else_branch {
            Some(else_br) => infer_type_from_expr(else_br, span, symbols),
            None => Err(CHIRLowerError::AmbiguousWidth { span }),
        },
        ExprType::Match(m) => m
            .arms
            .first()
            .map(|a| infer_type_from_expr(&a.body, span, symbols))
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
            .map(|e| infer_type_from_expr(e, span, symbols))
            .unwrap_or(Err(CHIRLowerError::AmbiguousWidth { span })),
        _ => Err(CHIRLowerError::AmbiguousWidth { span }),
    }
}

/// True for operators that always produce a 1-bit boolean result.
fn is_comparison_or_logical_op(op: &str) -> bool {
    matches!(op, "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||")
}

/// The whitespace-stripped callee path of a call, e.g. `"Bits::from_u32"`.
fn call_path(call: &ExprCall) -> Option<String> {
    match &*call.func {
        ExprType::Lit(lit) => Some(lit.text.chars().filter(|c| !c.is_whitespace()).collect()),
        _ => None,
    }
}

/// If `path` is a `Bits`-style value constructor, return its declared bit width.
/// An explicit turbofish (`Bits::<8>::from_u8`) wins; otherwise the `from_uNN`
/// name implies an NN-bit value (`from_u32` → 32) — the width the constructor
/// names for its source value.
fn constructor_width(path: &str) -> Option<usize> {
    if let Some(w) = width_from_turbofish(path) {
        return Some(w);
    }
    for (name, w) in [
        ("from_u128", 128usize),
        ("from_u64", 64),
        ("from_u32", 32),
        ("from_u16", 16),
        ("from_u8", 8),
    ] {
        if path.ends_with(name) {
            return Some(w);
        }
    }
    None
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

/// Build a symbol table of a module's data ports (`In<T,D>` / `Out<T,D>`) mapping
/// port name → inner hardware type. Clock ports are excluded.
fn build_port_symbols(fir: &FrontendModuleIR) -> SymbolTable {
    let mut symbols = SymbolTable::new();
    for p in &fir.signature.params {
        let compact: String = p.ty.ty_text.chars().filter(|c| !c.is_whitespace()).collect();
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
        ("usize", CHIRType::UInt { width: Width::Concrete(64) }),
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
fn lower_init_to_lit(expr: &ExprType) -> Option<CHIRLit> {
    match expr {
        ExprType::Lit(lit) => {
            let compact: String = lit.text.chars().filter(|c| !c.is_whitespace()).collect();
            if let Some((value, _)) = parse_int_literal(&compact) {
                let ty = infer_type_from_suffix(&compact)
                    .unwrap_or(CHIRType::UInt { width: Width::Concrete(64) });
                return Some(CHIRLit { ty, value });
            }
            match compact.as_str() {
                "true"  => Some(CHIRLit { ty: CHIRType::Bool, value: 1 }),
                "false" => Some(CHIRLit { ty: CHIRType::Bool, value: 0 }),
                _ => None,
            }
        }
        _ => None,
    }
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

fn lower_ports(fir: &FrontendModuleIR) -> Result<Vec<CHIRPort>, CHIRLowerError> {
    let mut ports = Vec::new();

    for param in &fir.signature.params {
        let compact: String = param.ty.ty_text.chars().filter(|c| !c.is_whitespace()).collect();

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
                span: param.span,
            });
        } else if let Some(inner) = strip_port_wrapper("In<", &compact) {
            let ty = resolve_type(inner, param.ty.span)?;
            ports.push(CHIRPort {
                name: param.name.clone(),
                direction: CHIRPortDir::Input,
                kind: CHIRPortKind::Data { ty },
                span: param.span,
            });
        } else if let Some(inner) = strip_port_wrapper("Out<", &compact) {
            let ty = resolve_type(inner, param.ty.span)?;
            ports.push(CHIRPort {
                name: param.name.clone(),
                direction: CHIRPortDir::Output,
                kind: CHIRPortKind::Data { ty },
                span: param.span,
            });
        } else {
            // Plain type — input data port (for combinational modules and hardware submodules)
            let ty = resolve_type(&param.ty.ty_text, param.ty.span)?;
            ports.push(CHIRPort {
                name: param.name.clone(),
                direction: CHIRPortDir::Input,
                kind: CHIRPortKind::Data { ty },
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
    ctx.output_ports = fir.signature.params.iter()
        .filter_map(|p| {
            let compact: String = p.ty.ty_text.chars().filter(|c| !c.is_whitespace()).collect();
            if compact.starts_with("Out<") { Some(p.name.clone()) } else { None }
        })
        .collect();
    ctx.symbols = build_port_symbols(fir);

    let mut stmts = Vec::new();

    for raw_stmt in &fir.raw_statements {
        match &raw_stmt.kind {
            RawStmtKind::Local(local) => {
                if let Some(init) = &local.init {
                    let ty = match &local.ty {
                        Some(t) => resolve_type(&t.ty_text, t.span)?,
                        None => infer_type_from_expr(init, local.span, &ctx.symbols)?,
                    };
                    let value = lower_expr(init, &mut ctx)?;
                    ctx.symbols.insert(local.name.clone(), ty.clone());
                    stmts.push(CHIRStmt::Wire {
                        name: local.name.clone(),
                        ty,
                        value,
                        span: local.span,
                    });
                }
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
    let mut loop_body_stmts: Option<&[RawStmt]> = None;
    // Pre-loop non-`mut` `let`s: combinational constants/wires available in the
    // loop body. Collected here, lowered once the context exists, and prepended
    // to the loop body. Stored as (name, type, init expr, span).
    let mut pre_loop_wires: Vec<(String, CHIRType, &ExprType, SourceSpan)> = Vec::new();
    // Seeded with the module's ports so pre-loop register inits that reference
    // ports (or later registers) can infer their width.
    let mut symbols = build_port_symbols(fir);

    for stmt in &fir.raw_statements {
        match &stmt.kind {
            RawStmtKind::Local(local) if local.is_mut => {
                let ty = match (&local.ty, &local.init) {
                    (Some(t), _) => resolve_type(&t.ty_text, t.span)?,
                    (None, Some(init)) => infer_type_from_expr(init, local.span, &symbols)?,
                    (None, None) => return Err(CHIRLowerError::AmbiguousWidth { span: local.span }),
                };
                symbols.insert(local.name.clone(), ty.clone());
                let init = local.init.as_ref().and_then(lower_init_to_lit);
                registers.push(CHIRRegDecl {
                    name: local.name.clone(),
                    ty,
                    init,
                    span: local.span,
                });
            }
            RawStmtKind::Local(local) => {
                // Pre-loop non-`mut` `let` → a combinational wire (often a
                // constant) visible throughout the loop body.
                if let Some(init) = &local.init {
                    let ty = match &local.ty {
                        Some(t) => resolve_type(&t.ty_text, t.span)?,
                        None => infer_type_from_expr(init, local.span, &symbols)?,
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
    ctx.clock_name = clock.clone();
    ctx.output_ports = fir.signature.params.iter()
        .filter_map(|p| {
            let compact: String = p.ty.ty_text.chars().filter(|c| !c.is_whitespace()).collect();
            if compact.starts_with("Out<") { Some(p.name.clone()) } else { None }
        })
        .collect();
    ctx.symbols = symbols;

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
        submodules: ctx.submodules,
        loop_body,
    })
}

// ── Lowering context ──────────────────────────────────────────────────────────

struct LowerCtx<'a> {
    hardware_fns: &'a std::collections::HashSet<String>,
    registry: &'a ModuleRegistry,
    submodules: Vec<CHIRSubmoduleInst>,
    inst_counters: std::collections::HashMap<String, usize>,
    clock_name: String,
    /// Names of `Out<T,D>` ports — used to validate `.write()` targets.
    output_ports: std::collections::HashSet<String>,
    /// In-scope names (ports, wires, registers) → type, for width inference.
    symbols: SymbolTable,
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

fn lower_stmt(
    stmt: &RawStmt,
    ctx: &mut LowerCtx,
    out: &mut Vec<CHIRStmt>,
) -> Result<(), CHIRLowerError> {
    match &stmt.kind {
        RawStmtKind::Local(local) => {
            if let Some(init) = &local.init {
                let ty = match &local.ty {
                    Some(t) => resolve_type(&t.ty_text, t.span)?,
                    None => infer_type_from_expr(init, local.span, &ctx.symbols)?,
                };
                let value = lower_expr(init, ctx)?;
                ctx.symbols.insert(local.name.clone(), ty.clone());
                out.push(CHIRStmt::Wire {
                    name: local.name.clone(),
                    ty,
                    value,
                    span: local.span,
                });
            }
        }

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

        // port.write(value) → PortWrite
        ExprType::MethodCall(mc) if mc.method == "write" && mc.args.len() == 1 => {
            let port_name = match mc.receiver.as_ref() {
                ExprType::Lit(lit) => lit.text.trim().to_string(),
                _ => return Err(CHIRLowerError::UnsupportedConstruct {
                    description: "port.write() receiver must be a simple port name".to_string(),
                    span,
                    suggested_rewrite: None,
                }),
            };
            let value = lower_expr(&mc.args[0], ctx)?;
            out.push(CHIRStmt::PortWrite { port_name, value, span });
        }

        ExprType::Assign(assign) => {
            let target = extract_assign_target(&assign.left, span)?;
            let value = lower_expr(&assign.right, ctx)?;
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
                let patterns = parse_or_patterns(&arm.pattern_text, span)?;
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
                            _ => return Err(CHIRLowerError::UnsupportedConstruct {
                                description: "port.write() receiver must be a simple port name".to_string(),
                                span,
                                suggested_rewrite: None,
                            }),
                        };
                        let value = lower_expr(&mc.args[0], ctx)?;
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

        ExprType::While(_) => {
            return Err(CHIRLowerError::UnsupportedConstruct {
                description: "while loops are not supported in hardware; use `loop { ... clk.tick().await; }`".to_string(),
                span,
                suggested_rewrite: Some("replace with a top-level `loop { ... }` with `clk.tick().await` boundaries".to_string()),
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
                _ => return Err(CHIRLowerError::UnsupportedConstruct {
                    description: "port.write() receiver must be a simple port name".to_string(),
                    span,
                    suggested_rewrite: None,
                }),
            };
            let value = lower_expr(&mc.args[0], ctx)?;
            Ok(vec![CHIRStmt::PortWrite { port_name, value, span }])
        }
        other => {
            let _ = lower_expr(other, ctx)?;
            Ok(vec![])
        }
    }
}

// ── Expression lowering ───────────────────────────────────────────────────────

pub fn lower_expr(expr: &ExprType, ctx: &mut LowerCtx) -> Result<CHIRExpr, CHIRLowerError> {
    match expr {
        ExprType::Lit(lit) => lower_lit_expr(&lit.text, lit.span),

        ExprType::Binary(bin) => {
            let left = lower_expr(&bin.left, ctx)?;
            let right = lower_expr(&bin.right, ctx)?;
            let op = lower_binop(&bin.op, bin.span)?;
            Ok(CHIRExpr::BinOp {
                left: Box::new(left),
                op,
                right: Box::new(right),
            })
        }

        ExprType::Unary(un) => {
            let inner = lower_expr(&un.expr, ctx)?;
            let op = lower_unop(&un.op, un.span)?;
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
            for arm in &match_expr.arms {
                let patterns = parse_or_patterns(&arm.pattern_text, match_expr.span)?;
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
            } else if let Some(width) = call_path(call).as_deref().and_then(constructor_width) {
                // `Bits::from_u32(x)` etc. — the value is the argument,
                // reinterpreted as a `width`-bit value.
                lower_bits_constructor(call, width, ctx)
            } else {
                Err(CHIRLowerError::UnsupportedConstruct {
                    description: "non-hardware function calls cannot appear in hardware expressions; add #[hardware]".to_string(),
                    span: call.span,
                    suggested_rewrite: Some("annotate the function with #[hardware]".to_string()),
                })
            }
        }

        ExprType::Cast(cast) => {
            // Strip the cast — width changes are handled at VLIR emission
            lower_expr(&cast.expr, ctx)
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

        // A block used as an expression (e.g. an `if`/`else` branch) evaluates to
        // its tail expression.
        ExprType::Block(b) => extract_block_expr_value(&b.stmts, b.span, ctx),

        other => Err(CHIRLowerError::UnsupportedConstruct {
            description: format!("expression type not supported in hardware: {:?}", std::mem::discriminant(other)),
            span: SourceSpan::default(),
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

/// Lower a bit-index `base[i]` to a single-bit slice. The index must be a
/// compile-time constant (variable indices require loop unrolling, which is a
/// separate lowering step).
fn lower_index(idx: &ExprIndex, ctx: &mut LowerCtx) -> Result<CHIRExpr, CHIRLowerError> {
    let base = lower_expr(&idx.base, ctx)?;
    let bit = eval_const_usize(&idx.index).ok_or_else(|| CHIRLowerError::UnsupportedConstruct {
        description: "bit index must be a compile-time constant".to_string(),
        span: idx.span,
        suggested_rewrite: Some("use a literal index, or unroll the loop so the index is constant".to_string()),
    })?;
    Ok(CHIRExpr::Slice { expr: Box::new(base), high: bit, low: bit })
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

fn lower_lit_expr(text: &str, span: SourceSpan) -> Result<CHIRExpr, CHIRLowerError> {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();

    if is_ident(&compact) {
        return Ok(CHIRExpr::Var(compact));
    }

    if let Some((value, _)) = parse_int_literal(&compact) {
        let ty = infer_type_from_suffix(&compact).unwrap_or(CHIRType::UInt { width: Width::Concrete(64) });
        return Ok(CHIRExpr::Lit(CHIRLit { ty, value }));
    }

    match compact.as_str() {
        "true"  => return Ok(CHIRExpr::Lit(CHIRLit { ty: CHIRType::Bool, value: 1 })),
        "false" => return Ok(CHIRExpr::Lit(CHIRLit { ty: CHIRType::Bool, value: 0 })),
        // 4-state single-bit constants → 1-bit literals.
        "Logic::One"  => return Ok(CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(1) }, value: 1 })),
        "Logic::Zero" => return Ok(CHIRExpr::Lit(CHIRLit { ty: CHIRType::UInt { width: Width::Concrete(1) }, value: 0 })),
        _ => {}
    }

    Err(CHIRLowerError::UnsupportedConstruct {
        description: format!("cannot lower literal: {}", text),
        span,
        suggested_rewrite: None,
    })
}

fn lower_text_expr(text: &str, span: SourceSpan) -> Result<CHIRExpr, CHIRLowerError> {
    lower_lit_expr(text.trim(), span)
}

fn lower_binop(op: &str, span: SourceSpan) -> Result<CHIRBinOp, CHIRLowerError> {
    match op {
        "+"  => Ok(CHIRBinOp::Add { wrapping: false }),
        "-"  => Ok(CHIRBinOp::Sub { wrapping: false }),
        "*"  => Ok(CHIRBinOp::Mul { wrapping: false }),
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
    match mc.method.as_str() {
        // Value passthroughs (simulation-only conversions on already-hardware
        // values): `port.read()`, `logic.as_bool()`. Lower to the receiver.
        "read" | "as_bool" if mc.args.is_empty() => lower_expr(&mc.receiver, ctx),

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
        "lock" | "unwrap" => lower_expr(&mc.receiver, ctx),
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
                let compact: String = p.ty.ty_text.chars().filter(|c| !c.is_whitespace()).collect();
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
        span: call.span,
    });

    Ok(CHIRExpr::Var(output_wire))
}

// ── Pattern parsing ───────────────────────────────────────────────────────────

/// Parse a `pattern_text` string into a single `CHIRPattern`.
pub fn parse_pattern(text: &str, span: SourceSpan) -> Result<CHIRPattern, CHIRLowerError> {
    let s = text.trim();

    if s == "_" {
        return Ok(CHIRPattern::Wildcard);
    }

    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len() - 1];
        let parts = split_top_level_commas(inner);
        let sub: Result<Vec<_>, _> = parts.iter().map(|p| parse_pattern(p.trim(), span)).collect();
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
pub fn parse_or_patterns(text: &str, span: SourceSpan) -> Result<Vec<CHIRPattern>, CHIRLowerError> {
    let mut patterns = Vec::new();
    let mut remaining = text.trim();

    loop {
        match find_top_level_pipe(remaining) {
            Some(idx) => {
                patterns.push(parse_pattern(&remaining[..idx], span)?);
                remaining = remaining[idx + 1..].trim_start();
            }
            None => {
                patterns.push(parse_pattern(remaining, span)?);
                break;
            }
        }
    }

    Ok(patterns)
}

// ── Post-lowering validation ──────────────────────────────────────────────────

/// Validate scope: all `CHIRExpr::Var` references must resolve to a declared name.
/// Also checks that `emit!` is only used when an output port exists.
fn validate_module(module: &CHIRModule) -> Result<(), CHIRLowerError> {
    let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();

    for port in &module.ports {
        known.insert(port.name.clone());
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
    }

    Ok(())
}

fn validate_stmts(
    stmts: &[CHIRStmt],
    known: &mut std::collections::HashSet<String>,
    span: SourceSpan,
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
    match expr {
        ExprType::Lit(lit) => {
            let name = lit.text.trim().to_string();
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
        _ => Err(CHIRLowerError::UnsupportedConstruct {
            description: "complex assignment targets not supported".to_string(),
            span,
            suggested_rewrite: None,
        }),
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
        ("usize", 64),
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
    fn test_resolve_bits_invalid_width_returns_error() {
        assert!(matches!(
            resolve_type("Bits<foo>", span()),
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
        assert_eq!(infer_type_from_expr(&expr, span(), &SymbolTable::new()).unwrap(), CHIRType::UInt { width: Width::Concrete(8) });
    }

    #[test]
    fn test_infer_type_from_expr_bool_literal() {
        use copper_core::frontend_ir::ExprLit;
        let expr = ExprType::Lit(ExprLit { text: "true".to_string(), span: span() });
        assert_eq!(infer_type_from_expr(&expr, span(), &SymbolTable::new()).unwrap(), CHIRType::Bool);
    }

    #[test]
    fn test_infer_type_from_expr_ambiguous_returns_error() {
        use copper_core::frontend_ir::ExprLit;
        let expr = ExprType::Lit(ExprLit { text: "42".to_string(), span: span() });
        assert!(matches!(
            infer_type_from_expr(&expr, span(), &SymbolTable::new()),
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
    fn non_constant_bit_index_is_rejected() {
        // A variable index needs loop unrolling first; must error, not miscompile.
        let fir = make_fir("fn f(d: In<Bits<8>, ()>, i: In<u8, ()>, o: Out<Logic, ()>) { let w = d.read()[i.read() as usize]; o.write(w); }");
        let result = lower_to_chir(&fir, &no_hw(), &empty_registry());
        assert!(result.is_err(), "variable index should be rejected");
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
        let lit = lower_init_to_lit(&expr).unwrap();
        assert_eq!(lit.value, 0);
        assert_eq!(lit.ty, CHIRType::UInt { width: Width::Concrete(8) });
    }

    #[test]
    fn test_lower_init_to_lit_bool_true() {
        use copper_core::frontend_ir::ExprLit;
        let expr = ExprType::Lit(ExprLit { text: "true".to_string(), span: span() });
        let lit = lower_init_to_lit(&expr).unwrap();
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
        assert!(lower_init_to_lit(&expr).is_none());
    }

    #[test]
    fn test_seq_register_has_init_value() {
        let fir = make_fir(
            "async fn counter(clk: Clock<MainClk>) {
                let mut count: u8 = 5u8;
                loop { clk.tick().await; }
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
                loop { clk.tick().await; }
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
        assert!(matches!(parse_pattern("_", span()).unwrap(), CHIRPattern::Wildcard));
    }

    #[test]
    fn test_parse_pattern_integer_literal() {
        match parse_pattern("0", span()).unwrap() {
            CHIRPattern::Lit(lit) => assert_eq!(lit.value, 0),
            _ => panic!("expected lit"),
        }
    }

    #[test]
    fn test_parse_pattern_integer_with_suffix() {
        match parse_pattern("42u8", span()).unwrap() {
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
            parse_pattern("true", span()).unwrap(),
            CHIRPattern::Lit(CHIRLit { ty: CHIRType::Bool, value: 1 })
        ));
    }

    #[test]
    fn test_parse_pattern_bool_false() {
        assert!(matches!(
            parse_pattern("false", span()).unwrap(),
            CHIRPattern::Lit(CHIRLit { ty: CHIRType::Bool, value: 0 })
        ));
    }

    #[test]
    fn test_parse_pattern_tuple_two_elements() {
        match parse_pattern("(0 , 1)", span()).unwrap() {
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
        match parse_pattern("(_, _)", span()).unwrap() {
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
        match parse_pattern("Ready", span()).unwrap() {
            CHIRPattern::EnumVariant { name, inner: None } => assert_eq!(name, "Ready"),
            _ => panic!("expected enum variant"),
        }
    }

    #[test]
    fn test_parse_pattern_lowercase_binding_is_wildcard() {
        assert!(matches!(parse_pattern("x", span()).unwrap(), CHIRPattern::Wildcard));
    }

    #[test]
    fn test_parse_or_patterns_single() {
        let patterns = parse_or_patterns("1", span()).unwrap();
        assert_eq!(patterns.len(), 1);
        assert!(matches!(patterns[0], CHIRPattern::Lit(CHIRLit { value: 1, .. })));
    }

    #[test]
    fn test_parse_or_patterns_two_alternatives() {
        let patterns = parse_or_patterns("1 | 2", span()).unwrap();
        assert_eq!(patterns.len(), 2);
        assert!(matches!(patterns[0], CHIRPattern::Lit(CHIRLit { value: 1, .. })));
        assert!(matches!(patterns[1], CHIRPattern::Lit(CHIRLit { value: 2, .. })));
    }

    #[test]
    fn test_parse_or_patterns_three_alternatives() {
        let patterns = parse_or_patterns("0 | 1 | 2", span()).unwrap();
        assert_eq!(patterns.len(), 3);
    }

    #[test]
    fn test_parse_or_patterns_wildcard_alone() {
        let patterns = parse_or_patterns("_", span()).unwrap();
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
            "async fn counter(clk: Clock<MainClk>, data: In<u8, MainClk>) {
                let mut count: u8 = 0u8;
                loop { clk.tick().await; }
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
            validate_module(&module),
            Err(CHIRLowerError::UnsupportedConstruct { .. })
        ));
    }

    #[test]
    fn test_validate_port_reference_is_ok() {
        use copper_core::chir::CHIRCombBody;
        let module = CHIRModule {
            name: "test".to_string(),
            params: vec![],
            ports: vec![
                CHIRPort {
                    name: "a".to_string(),
                    direction: CHIRPortDir::Input,
                    kind: CHIRPortKind::Data { ty: CHIRType::UInt { width: Width::Concrete(8) } },
                    span: span(),
                },
                CHIRPort {
                    name: "out".to_string(),
                    direction: CHIRPortDir::Output,
                    kind: CHIRPortKind::Data { ty: CHIRType::UInt { width: Width::Concrete(8) } },
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
        assert!(validate_module(&module).is_ok());
    }

    #[test]
    fn test_validate_wire_reference_after_declaration_is_ok() {
        use copper_core::chir::CHIRCombBody;
        let module = CHIRModule {
            name: "test".to_string(),
            params: vec![],
            ports: vec![
                CHIRPort {
                    name: "a".to_string(),
                    direction: CHIRPortDir::Input,
                    kind: CHIRPortKind::Data { ty: CHIRType::UInt { width: Width::Concrete(8) } },
                    span: span(),
                },
                CHIRPort {
                    name: "out".to_string(),
                    direction: CHIRPortDir::Output,
                    kind: CHIRPortKind::Data { ty: CHIRType::UInt { width: Width::Concrete(8) } },
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
        assert!(validate_module(&module).is_ok());
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
}
