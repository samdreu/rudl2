/// IR Builder: Transform Rust AST into Copper IR
/// 
/// This module handles parsing Rust function signatures and bodies
/// to create Copper IR that can be code-generated to Verilog.

use std::collections::{HashMap, HashSet};

use syn::punctuated::Punctuated;
use syn::parse::Parser;
use syn::{Block, Expr, FnArg, ItemFn, Pat, ReturnType, Stmt, Token};
use quote::ToTokens;
use copper_core::ir::*;
use log::{debug, error, info, warn, trace};

/// Builder for transforming Rust AST into Copper IR
pub struct IRBuilder;

#[derive(Default)]
struct LoweringArtifacts {
    internal_wires: Vec<WireDecl>,
    submodules: Vec<SubmoduleInstance>,
    next_instance_id: usize,
    next_wire_id: usize,
}

impl IRBuilder {
    /// Build IR from a Rust function
    pub fn from_function(func: &ItemFn) -> Result<ModuleIR, String> {
        log::debug!("Building IR for a function.");
        let deps = HashMap::new();
        Self::from_function_with_deps(func, &deps)
    }

    pub fn from_function_with_deps(
        func: &ItemFn,
        deps: &HashMap<String, ItemFn>,
    ) -> Result<ModuleIR, String> {
        log::info!("Building IR for function '{}'", func.sig.ident);
        let module_name = func.sig.ident.to_string();
        log::debug!("Function name: {}", module_name);
        let is_async = func.sig.asyncness.is_some();
        log::debug!("Is async: {}", is_async);

        // Classify the module type as sequential or combinational
        let module_type = Self::classify_module(func, is_async);
        log::debug!("Classified module type: {:?}", module_type);

        // Extract ports from function signature
        let mut port_decls = Self::extract_ports(func, is_async)?;
        for port in &port_decls {
            log::debug!("Port: {} {} [{}]", match port.direction {
                Direction::Input => "input",
                Direction::Output => "output",
            }, port.name, port.width);
        }
        let mut artifacts = LoweringArtifacts::default();

        // Build the logic based on module type
        let logic = match module_type {
            ModuleType::Combinational => {
                let exprs = Self::extract_combinational_logic(&func.block, deps, &mut artifacts)?;
                log::debug!("Extracted combinational logic with {} expressions", exprs.len());
                for expr in &exprs {
                    log::trace!(
                        "Combinational assign: {} = {}",
                        expr.target,
                        Self::expression_to_inline_string(&expr.value)
                    );
                }
                ModuleLogic::Combinational {
                    expressions: exprs,
                }
            }

            ModuleType::Sequential => {
                // TODO (FSM — future work): the current lowering handles the common
                // `loop { emit!(); clk.tick().await; update_state; }` pattern, which
                // maps directly to a single `always @(posedge clk)` block.
                //
                // A more general async function can have multiple `.await` points
                // separated by conditional branches, e.g.:
                //
                //   loop {
                //       if ready { emit!(data); clk.tick().await; }
                //       else     { emit!(stall); clk.tick().await; }
                //   }
                //
                // This requires an FSM transformation: introduce an explicit state
                // register, one state per resume point, and a `case (state)` in the
                // always block to dispatch to the right continuation.  This
                // transformation is not yet implemented; such functions will produce
                // incomplete or incorrect Verilog until it is.
                let (registers, always_block) =
                    Self::extract_sequential_logic(&func.block, deps, &mut artifacts)?;

                // Separate emit! bindings (originally NonBlockingAssign targeting
                // output ports) from register-update assignments.
                //
                // In the Rust executor, after tick_clock() the task runs to
                // completion and writes the newly-computed value to its output.
                // This maps to a *continuous assign* in Verilog:
                //
                //   assign out = reg_sig;
                //
                // rather than a clocked non-blocking assignment.  We pull emit!
                // targets out of the always block and put them in output_assigns.
                let output_port_names: std::collections::HashSet<String> = port_decls
                    .iter()
                    .filter(|p| p.direction == Direction::Output)
                    .map(|p| p.name.clone())
                    .collect();

                let (emit_stmts, reg_stmts): (Vec<_>, Vec<_>) =
                    always_block.statements.into_iter().partition(|s| {
                        matches!(s,
                            Statement::NonBlockingAssign { target, .. }
                            if output_port_names.contains(target.as_str())
                        )
                    });

                let output_assigns: Vec<ContinuousAssign> = emit_stmts
                    .into_iter()
                    .map(|s| match s {
                        Statement::NonBlockingAssign { target, value } => {
                            ContinuousAssign { target, value }
                        }
                        _ => unreachable!(),
                    })
                    .collect();

                let always_block = AlwaysBlock { statements: reg_stmts };

                // Output ports driven by continuous assigns are wires, not regs.
                let emit_targets: std::collections::HashSet<String> =
                    output_assigns.iter().map(|a| a.target.clone()).collect();
                for port in port_decls.iter_mut() {
                    if port.direction == Direction::Output
                        && emit_targets.contains(&port.name)
                    {
                        port.ty = PortType::Wire;
                    }
                }

                ModuleLogic::Sequential {
                    clock: "clk".to_string(),
                    reset: None,
                    registers,
                    always_block,
                    output_assigns,
                }
            }

            ModuleType::AsyncCombinational => {
                return Err("Async functions without main loop not yet supported".to_string());
            }
        };

        Ok(ModuleIR {
            name: module_name,
            ports: port_decls,
            internal_wires: artifacts.internal_wires,
            submodules: artifacts.submodules,
            logic,
        })
    }

    /// Classify whether a module is combinational or sequential
    fn classify_module(func: &ItemFn, is_async: bool) -> ModuleType {
        if !is_async {
            return ModuleType::Combinational;
        }

        // Check if async function has main loop with tick().await pattern
        if Self::has_main_loop_with_tick(&func.block) {
            ModuleType::Sequential
        } else {
            ModuleType::AsyncCombinational
        }
    }

    /// Check if block contains a loop with clk.tick().await pattern
    fn has_main_loop_with_tick(block: &Block) -> bool {
        for stmt in &block.stmts {
            if let Stmt::Expr(Expr::Loop(_), _) = stmt {
                return true;
            }
        }
        false
    }

    /// Extract port declarations from function signature
    fn extract_ports(func: &ItemFn, is_async: bool) -> Result<Vec<PortDecl>, String> {
        let mut ports = vec![];

        // Extract input ports from parameters
        for param in &func.sig.inputs {
            if let FnArg::Typed(pt) = param {
                let param_name = match &*pt.pat {
                    Pat::Ident(ident) => ident.ident.to_string(),
                    _ => continue,
                };

                // Keep clock as an explicit input for sequential modules.
                if !is_async && param_name.contains("clk") {
                    continue;
                }

                // Skip common async output handle parameters.
                if is_async && param_name == "output" {
                    continue;
                }

                // Infer port width from type.
                let width = if is_async && param_name.contains("clk") {
                    1
                } else {
                    Self::infer_width_from_type(&pt.ty)
                };

                ports.push(PortDecl {
                    name: Self::sanitize_identifier(&param_name),
                    direction: Direction::Input,
                    width,
                    ty: PortType::Wire,
                });
            }
        }

        if is_async {
            // For async (sequential) functions the output width comes from the
            // declared return type, not from the emitted value (which may be a
            // variable whose type the parser doesn't track).
            let output_width = match &func.sig.output {
                ReturnType::Type(_, ty) => Self::infer_width_from_type(ty),
                ReturnType::Default => 1,
            };
            let emit_names = Self::collect_emit_output_names(&func.block)?;
            for name in emit_names {
                ports.push(PortDecl {
                    name,
                    direction: Direction::Output,
                    width: output_width,
                    ty: PortType::Reg,
                });
            }
        } else if let ReturnType::Type(_, ty) = &func.sig.output {
            match &**ty {
                syn::Type::Tuple(tuple_ty) => {
                    let targets = Self::infer_sync_output_targets(&func.block)?;
                    for (idx, elem_ty) in tuple_ty.elems.iter().enumerate() {
                        let name = targets
                            .get(idx)
                            .cloned()
                            .unwrap_or_else(|| format!("out{}", idx));
                        ports.push(PortDecl {
                            name: Self::sanitize_identifier(&name),
                            direction: Direction::Output,
                            width: Self::infer_width_from_type(elem_ty),
                            ty: PortType::Wire,
                        });
                    }
                }
                _ => {
                    let name = Self::infer_sync_output_targets(&func.block)?
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| "out".to_string());
                    ports.push(PortDecl {
                        name: Self::sanitize_identifier(&name),
                        direction: Direction::Output,
                        width: Self::infer_width_from_type(ty),
                        ty: PortType::Wire,
                    });
                }
            }
        }

        // Verilog module ports must have globally unique names.
        // If names collide (e.g. input `a` and output `a`), keep the first and
        // rename later ports with a direction-aware suffix.
        Self::dedupe_port_names(&mut ports);

        Ok(ports)
    }

    fn dedupe_port_names(ports: &mut [PortDecl]) {
        let mut used = HashSet::<String>::new();

        for port in ports.iter_mut() {
            let original = port.name.clone();
            if used.insert(original.clone()) {
                continue;
            }

            let suffix = match port.direction {
                Direction::Input => "in",
                Direction::Output => "out",
            };

            let mut candidate = format!("{}_{}", original, suffix);
            let mut counter = 2usize;
            while used.contains(&candidate) {
                candidate = format!("{}_{}{}", original, suffix, counter);
                counter += 1;
            }

            port.name = candidate.clone();
            used.insert(candidate);
        }
    }

    /// Infer bit width from type annotation
    fn infer_width_from_type(ty: &syn::Type) -> usize {
        fn infer_inner(ty: &syn::Type) -> Option<usize> {
            match ty {
                syn::Type::Path(type_path) => {
                    for seg in &type_path.path.segments {
                        let ident = seg.ident.to_string();
                        if ident == "Bit" {
                            return Some(1);
                        }
                        if ident == "Bits" {
                            if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                                for arg in &args.args {
                                    if let syn::GenericArgument::Const(syn::Expr::Lit(expr_lit)) = arg {
                                        if let syn::Lit::Int(int_lit) = &expr_lit.lit {
                                            if let Ok(width) = int_lit.base10_parse::<usize>() {
                                                return Some(width);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                            for arg in &args.args {
                                if let syn::GenericArgument::Type(inner_ty) = arg {
                                    if let Some(width) = infer_inner(inner_ty) {
                                        return Some(width);
                                    }
                                }
                            }
                        }
                    }
                    None
                }
                syn::Type::Reference(reference) => infer_inner(&reference.elem),
                _ => None,
            }
        }

        infer_inner(ty).unwrap_or(8)
    }

    /// Extract combinational logic from function body
    fn extract_combinational_logic(
        block: &Block,
        deps: &HashMap<String, ItemFn>,
        artifacts: &mut LoweringArtifacts,
    ) -> Result<Vec<ContinuousAssign>, String> {
        // gets return expression (output of the combinational function)
        let return_expr = Self::find_return_expr(block)
            .ok_or_else(|| "No return statement found in combinational function".to_string())?;

        // matches on return expression to support both single and tuple outputs
        match return_expr {
            // multiple outputs (expressed as a tuple)
            Expr::Tuple(tuple_expr) => {
                let mut assigns = Vec::new();
                // iterate through outputs
                for (idx, elem) in tuple_expr.elems.iter().enumerate() {
                    let target = match elem {
                        Expr::Path(path) => path
                            .path
                            .get_ident()
                            .map(|id| id.to_string())
                            .unwrap_or_else(|| format!("out{}", idx)),
                        _ => format!("out{}", idx),
                    };

                    let empty_aliases = HashMap::new();
                    // could be a direct expression or a submodule call, e.g. `my_submodule(input)`
                    let value = match elem {
                        Expr::Call(call_expr) => {
                            Self::lower_call_to_submodule(call_expr, deps, artifacts, &empty_aliases)?
                        }
                        _ => Self::parse_expression(elem)?,
                    };

                    // push to assignments
                    assigns.push(ContinuousAssign {
                        target: Self::sanitize_identifier(&target),
                        value,
                    });
                }
                Ok(assigns)
            }
            other => {
                let target = match other {
                    Expr::Path(path) => path
                        .path
                        .get_ident()
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "out".to_string()),
                    _ => "out".to_string(),
                };

                let empty_aliases = HashMap::new();
                let value = match other {
                    Expr::Call(call_expr) => {
                        Self::lower_call_to_submodule(call_expr, deps, artifacts, &empty_aliases)?
                    }
                    _ => Self::parse_expression(other)?,
                };

                Ok(vec![ContinuousAssign {
                    target: Self::sanitize_identifier(&target),
                    value,
                }])
            }
        }
    }

    /// Extract sequential logic from async function body
    fn extract_sequential_logic(
        block: &Block,
        deps: &HashMap<String, ItemFn>,
        artifacts: &mut LoweringArtifacts,
    ) -> Result<(Vec<RegisterDecl>, AlwaysBlock), String> {
        log::debug!("Extracting sequential logic from async function body");
        let mut registers = vec![];
        let mut statements = vec![];
        let mut aliases = HashMap::<String, String>::new();

        // Extract register declarations from `let mut` statements.
        // In syn 2, `let mut x: T = val` has Pat::Type wrapping Pat::Ident,
        // so we use pat_to_ident() to handle both forms.
        for stmt in &block.stmts {
            if let Stmt::Local(local) = stmt {
                if let Some(ident) = Self::pat_to_ident(&local.pat) {
                    if ident.mutability.is_some() {
                        let reg_name = ident.ident.to_string();
                        let width = if let Some(init) = &local.init {
                            Self::infer_width_from_expr(&init.expr)
                        } else {
                            8
                        };

                        registers.push(RegisterDecl {
                            name: Self::sanitize_identifier(&reg_name),
                            width,
                            initial: None,
                        });
                    }
                }
            }
        }
        log::debug!("Extracted {} register declarations", registers.len());

        // Extract loop body statements
        for stmt in &block.stmts {
            if let Stmt::Expr(Expr::Loop(loop_expr), _) = stmt {
                // Parse loop body
                Self::extract_loop_statements(
                    &loop_expr.body,
                    &mut statements,
                    deps,
                    artifacts,
                    &mut aliases,
                )?;
            }
        }
        log::debug!("Extracted {} statements from loop body", statements.len());

        // Auto-declare blocking-assign temporaries as registers.
        //
        // In Verilog, every variable assigned inside an always block must be
        // declared as `reg` (or `logic`).  Blocking-assign targets that come
        // from `let x = <computed>;` in the Rust loop body are intermediate
        // temporaries — they need a `reg` declaration even though they aren't
        // architectural state.  We add them here rather than requiring the IR
        // builder's caller to track them separately.
        let existing_reg_names: std::collections::HashSet<String> =
            registers.iter().map(|r| r.name.clone()).collect();

        for stmt in &statements {
            if let Statement::BlockingAssign { target, .. } = stmt {
                if !existing_reg_names.contains(target.as_str()) {
                    // Width defaults to 8; the type annotation on the Rust `let`
                    // binding isn't propagated here, but u8 covers the majority
                    // of current examples.
                    registers.push(RegisterDecl {
                        name: target.clone(),
                        width: 8,
                        initial: None,
                    });
                }
            }
        }

        let always_block = AlwaysBlock { statements };

        Ok((registers, always_block))
    }

    /// Extract statements from loop body
    fn extract_loop_statements(
        block: &Block,
        statements: &mut Vec<Statement>,
        deps: &HashMap<String, ItemFn>,
        artifacts: &mut LoweringArtifacts,
        aliases: &mut HashMap<String, String>,
    ) -> Result<(), String> {
        log::debug!("Extracting statements from loop body with {} statements", block.stmts.len());
        for stmt in &block.stmts {
            match stmt {
                // Local bindings inside sequential loops.
                //
                // Two cases:
                // 1. Simple accessor alias: `let x = *sig.lock().unwrap();`
                //    → record x → sig in the alias table so later references
                //      to x resolve to sig.
                //
                // 2. Computed value: `let x = a.wrapping_add(1);`
                //    → emit a blocking assignment `x = a + 1;` in the always
                //      block so x is available as a named intermediate wire.
                //      Blocking assignment is correct here because x is not a
                //      registered state element; it's a combinational temporary
                //      computed fresh every clock cycle.
                syn::Stmt::Local(local) => {
                    if let Some(ident) = Self::pat_to_ident(&local.pat) {
                        let var_name = ident.ident.to_string();
                        if let Some(init) = &local.init {
                            if let Some(source) = Self::extract_signal_source(&init.expr) {
                                // Simple alias path.
                                aliases.insert(var_name, Self::sanitize_identifier(&source));
                            } else if let Ok(value) = Self::parse_expression(&init.expr) {
                                // Computed value — emit blocking assignment.
                                // Apply alias substitution so any input signals in
                                // the RHS (e.g. `input_val`) resolve to the actual
                                // Verilog port name (e.g. `in_data`).
                                let target = Self::sanitize_identifier(&var_name);
                                let value = Self::apply_aliases(value, aliases);
                                statements.push(Statement::BlockingAssign {
                                    target: target.clone(),
                                    value,
                                });
                                // Alias so later references to the Rust variable
                                // name resolve to the (sanitized) Verilog wire name.
                                aliases.insert(var_name, target);
                            }
                            // If parse also fails, skip silently — best-effort.
                        }
                    }
                }

                // emit!() calls become non-blocking assignments.
                // syn 2 produces Stmt::Expr(Expr::Macro) when the macro is in
                // expression position (no semicolon) and Stmt::Macro when it has
                // a trailing semicolon.  Handle both.
                syn::Stmt::Expr(Expr::Macro(macro_expr), _) => {
                    if macro_expr.mac.path.is_ident("emit") {
                        let bindings = Self::parse_emit_bindings(macro_expr)?;
                        for (target, value) in bindings {
                            statements.push(Statement::NonBlockingAssign { target, value });
                        }
                    }
                }
                syn::Stmt::Macro(stmt_mac) if stmt_mac.mac.path.is_ident("emit") => {
                    let bindings = Self::parse_emit_bindings_from_mac(&stmt_mac.mac)?;
                    for (target, value) in bindings {
                        statements.push(Statement::NonBlockingAssign { target, value });
                    }
                }

                // Regular assignments
                syn::Stmt::Expr(
                    Expr::Assign(assign_expr),
                    _,
                ) => {
                    if let Expr::Path(path) = assign_expr.left.as_ref() {
                        let target = path.path.get_ident()
                            .map(|id| id.to_string())
                            .unwrap_or_default();
                        let value = match assign_expr.right.as_ref() {
                            Expr::Call(call_expr) => {
                                Self::lower_call_to_submodule(call_expr, deps, artifacts, aliases)?
                            },
                            _ => {
                                let v = Self::parse_expression(&assign_expr.right)?;
                                Self::apply_aliases(v, aliases)
                            }
                        };

                        statements.push(Statement::NonBlockingAssign {
                            target: Self::sanitize_identifier(&target),
                            value,
                        });
                    }
                }

                // output.lock().unwrap().clone_from(&reg) -> out <= reg;
                syn::Stmt::Expr(Expr::MethodCall(method_call), _) => {
                    if method_call.method == "clone_from" && !method_call.args.is_empty() {
                        if let Some(source_expr) = method_call.args.first() {
                            let source = match source_expr {
                                Expr::Reference(reference) => Self::parse_expression(&reference.expr)?,
                                _ => Self::parse_expression(source_expr)?,
                            };

                            statements.push(Statement::NonBlockingAssign {
                                target: "out".to_string(),
                                value: source,
                            });
                        }
                    }
                }

                // clk.tick().await - skip, it's just a timing marker
                syn::Stmt::Expr(Expr::Await(_), _) => {
                    // Skip await expressions
                }

                // If statements
                syn::Stmt::Expr(Expr::If(if_expr), _) => {
                    let condition = Self::parse_expression(&if_expr.cond)?;
                    let mut then_branch = vec![];
                    Self::extract_loop_statements(
                        &if_expr.then_branch,
                        &mut then_branch,
                        deps,
                        artifacts,
                        aliases,
                    )?;

                    let else_branch = if let Some((_, else_body)) = &if_expr.else_branch {
                        let mut else_stmts = vec![];
                        if let Expr::Block(block_expr) = else_body.as_ref() {
                            Self::extract_loop_statements(
                                &block_expr.block,
                                &mut else_stmts,
                                deps,
                                artifacts,
                                aliases,
                            )?;
                        }
                        Some(else_stmts)
                    } else {
                        None
                    };

                    statements.push(Statement::If {
                        condition,
                        then_branch,
                        else_branch,
                    });
                }

                // Standalone match statement in loop body.
                //
                // Pattern:
                //   match selector {
                //       Pat1 => { stmts... }
                //       Pat2 => { stmts... }
                //       _    => { stmts... }
                //   }
                //
                // Lowered to a Verilog `case` statement.  Each arm body is
                // recursively extracted.  Single-expression arms (no braces)
                // are skipped — the arms that matter for hardware state
                // updates always contain block bodies.
                //
                // Note: tuple selectors like `match (state, input) { ... }`
                // are partially supported: `parse_expression` will produce an
                // expression for the selector, but Verilog's `case` cannot
                // natively match tuples.  This produces syntactically valid
                // but logically incorrect Verilog for tuple selectors.
                // Single-variable selectors work correctly.
                //
                // TODO (FSM): when the match is guarding different `.await`
                // points (not just different register updates within one cycle),
                // this needs FSM transformation instead — see the note in
                // `from_function_with_deps`.
                syn::Stmt::Expr(Expr::Match(match_expr), _) => {
                    let selector = Self::parse_expression(&match_expr.expr)?;
                    let mut arms: Vec<CaseArm> = Vec::new();
                    let mut default: Option<Vec<Statement>> = None;

                    for arm in &match_expr.arms {
                        let mut arm_stmts = Vec::new();

                        // Extract statements from the arm body.
                        match arm.body.as_ref() {
                            Expr::Block(block_expr) => {
                                Self::extract_loop_statements(
                                    &block_expr.block,
                                    &mut arm_stmts,
                                    deps,
                                    artifacts,
                                    aliases,
                                )?;
                            }
                            // Single-expression arm — skip; meaningful state
                            // updates use block arms.
                            _ => {}
                        }

                        if matches!(&arm.pat, Pat::Wild(_)) {
                            default = Some(arm_stmts);
                        } else {
                            match Self::arm_pat_to_case_pattern(&arm.pat) {
                                Ok(pattern) => arms.push(CaseArm { pattern, body: arm_stmts }),
                                Err(_) => {} // Skip arms with unsupported patterns.
                            }
                        }
                    }

                    statements.push(Statement::Case { selector, arms, default });
                }

                _ => {
                    // Other statement types not yet supported — skip silently.
                }
            }
        }

        Ok(())
    }

    /// Convert a match arm pattern to a Verilog case pattern.
    ///
    /// Handles:
    /// - Path patterns: `Logic::Zero`, `State::Idle`, `Zero`, `ONE`
    /// - Literal patterns: `0`, `1`, `42`
    /// - Tuple patterns: `(State::Idle, Logic::Zero)` — lowered to the first
    ///   element only (a limitation; full tuple matching requires FSM expansion)
    fn arm_pat_to_case_pattern(pat: &Pat) -> Result<Pattern, String> {
        match pat {
            Pat::Path(path_pat) => {
                if let Some(expr) = Self::path_to_literal_or_const(&path_pat.path) {
                    Ok(Pattern::Literal(Self::expression_to_inline_string(&expr)))
                } else {
                    let text = path_pat.path.segments.iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::");
                    Ok(Pattern::Literal(Self::sanitize_identifier(&text)))
                }
            }
            Pat::Lit(pat_lit) => {
                match &pat_lit.lit {
                    syn::Lit::Int(int_lit) => {
                        let value = int_lit.base10_parse::<u128>()
                            .map_err(|_| "Failed to parse integer literal in pattern".to_string())?;
                        Ok(Pattern::Literal(format!("{}", value)))
                    }
                    _ => Err("Unsupported literal pattern type".to_string()),
                }
            }
            Pat::TupleStruct(tuple_pat) => {
                // Handle enum variant patterns like `State::Idle(..)`
                if let Some(expr) = Self::path_to_literal_or_const(&tuple_pat.path) {
                    Ok(Pattern::Literal(Self::expression_to_inline_string(&expr)))
                } else {
                    let text = tuple_pat.path.segments.iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::");
                    Ok(Pattern::Literal(Self::sanitize_identifier(&text)))
                }
            }
            Pat::Tuple(tuple_pat) => {
                // Tuple pattern (state, input) — take only the first element.
                // Full tuple matching requires FSM transformation (future work).
                if let Some(first) = tuple_pat.elems.first() {
                    Self::arm_pat_to_case_pattern(first)
                } else {
                    Err("Empty tuple pattern".to_string())
                }
            }
            Pat::Wild(_) => Ok(Pattern::Default),
            Pat::Or(or_pat) => {
                // Take first alternative (Verilog doesn't support OR patterns natively)
                if let Some(first) = or_pat.cases.first() {
                    Self::arm_pat_to_case_pattern(first)
                } else {
                    Err("Empty or-pattern".to_string())
                }
            }
            _ => Err(format!("Unsupported pattern in case arm: {:?}", pat.to_token_stream())),
        }
    }

    /// Parse a Rust expression into IR expression
    fn parse_expression(expr: &Expr) -> Result<Expression, String> {
        log::debug!("Parsing expression: {}", expr.to_token_stream());
        match expr {
            Expr::Path(path) => {
                if let Some(const_expr) = Self::path_to_literal_or_const(&path.path) {
                    Ok(const_expr)
                } else {
                    let name = path.path.get_ident()
                        .map(|id| Self::sanitize_identifier(&id.to_string()))
                        .unwrap_or_else(|| path.path.segments.iter()
                            .map(|seg| seg.ident.to_string())
                            .collect::<Vec<_>>()
                            .join("::"));
                    Ok(Expression::var(name))
                }
            }

            Expr::Lit(lit_expr) => {
                use syn::Lit;
                match &lit_expr.lit {
                    Lit::Int(int_lit) => {
                        let value = int_lit.base10_parse::<u128>()
                            .map_err(|_| "Failed to parse integer literal".to_string())?;
                        Ok(Expression::literal(8, value)) // TODO: Infer actual width
                    }
                    _ => Err("Only integer literals supported".to_string()),
                }
            }

            Expr::Binary(binary_expr) => {
                let left = Self::parse_expression(&binary_expr.left)?;
                let right = Self::parse_expression(&binary_expr.right)?;
                let op = Self::parse_binary_op(&binary_expr.op)?;

                Ok(Expression::BinaryOp {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                })
            }

            // Dereference (*x) is a no-op in the hardware model — lower to the
            // inner expression. This handles `*signal.lock().unwrap()` patterns.
            Expr::Unary(unary_expr) if matches!(unary_expr.op, syn::UnOp::Deref(_)) => {
                Self::parse_expression(&unary_expr.expr)
            }

            Expr::Unary(unary_expr) => {
                let operand = Self::parse_expression(&unary_expr.expr)?;
                let op = Self::parse_unary_op(&unary_expr.op)?;

                Ok(Expression::UnaryOp {
                    op,
                    operand: Box::new(operand),
                })
            }

            // Parenthesized expression — transparently unwrap.
            Expr::Paren(paren_expr) => Self::parse_expression(&paren_expr.expr),

            // Reference (&x) — lower to the inner expression.
            Expr::Reference(ref_expr) => Self::parse_expression(&ref_expr.expr),

            // Method calls. We handle common wrapping arithmetic and accessor
            // chains; everything else falls back to a StringLiteral placeholder.
            Expr::MethodCall(method_call) => {
                let method_name = method_call.method.to_string();
                match method_name.as_str() {
                    // Wrapping arithmetic → hardware binary ops (hardware can't overflow).
                    "wrapping_add" | "saturating_add" => {
                        let receiver = Self::parse_expression(&method_call.receiver)?;
                        let arg = method_call.args.first()
                            .ok_or_else(|| format!("{} requires one argument", method_name))?;
                        Ok(Expression::BinaryOp {
                            left: Box::new(receiver),
                            op: BinaryOp::Add,
                            right: Box::new(Self::parse_expression(arg)?),
                        })
                    }
                    "wrapping_sub" | "saturating_sub" => {
                        let receiver = Self::parse_expression(&method_call.receiver)?;
                        let arg = method_call.args.first()
                            .ok_or_else(|| format!("{} requires one argument", method_name))?;
                        Ok(Expression::BinaryOp {
                            left: Box::new(receiver),
                            op: BinaryOp::Sub,
                            right: Box::new(Self::parse_expression(arg)?),
                        })
                    }
                    "wrapping_mul" | "saturating_mul" => {
                        let receiver = Self::parse_expression(&method_call.receiver)?;
                        let arg = method_call.args.first()
                            .ok_or_else(|| format!("{} requires one argument", method_name))?;
                        Ok(Expression::BinaryOp {
                            left: Box::new(receiver),
                            op: BinaryOp::Mul,
                            right: Box::new(Self::parse_expression(arg)?),
                        })
                    }
                    // Accessor/unwrapping chains — transparent in hardware.
                    "lock" | "unwrap" | "clone" | "borrow" | "as_ref" | "as_mut" | "into" => {
                        Self::parse_expression(&method_call.receiver)
                    }
                    // Unknown method — fall back to receiver signal as best effort.
                    _ => Self::parse_expression(&method_call.receiver),
                }
            }

            Expr::If(if_expr) => {
                let condition = Self::parse_expression(&if_expr.cond)?;
                // Extract the last expression from then branch (it's a Block, not an Expr)
                let then_expr = Self::extract_last_expression(&if_expr.then_branch)?;

                let else_expr = if let Some((_, else_body)) = &if_expr.else_branch {
                    if let Expr::Block(block_expr) = else_body.as_ref() {
                        Some(Box::new(Self::extract_last_expression(&block_expr.block)?))
                    } else {
                        Some(Box::new(Self::parse_expression(else_body)?))
                    }
                } else {
                    None
                };

                if let Some(else_val) = else_expr {
                    Ok(Expression::Ternary {
                        condition: Box::new(condition),
                        then_val: Box::new(then_expr),
                        else_val,
                    })
                } else {
                    Err("If expression without else not supported".to_string())
                }
            }

            Expr::Field(field_expr) => {
                // Common Copper pattern: matching on tuple-struct payloads like `bit.0`.
                // Lower this to the base signal expression for now.
                Self::parse_expression(&field_expr.base)
            }

            Expr::Match(match_expr) => {
                let selector = Self::parse_expression(&match_expr.expr)?;
                let mut else_expr: Option<Expression> = None;

                for arm in match_expr.arms.iter().rev() {
                    let arm_value = Self::parse_expression(&arm.body)?;

                    if matches!(&arm.pat, Pat::Wild(_)) {
                        else_expr = Some(arm_value);
                        continue;
                    }

                    let pattern_expr = Self::pattern_to_expression(&arm.pat)?;
                    let condition = Expression::binary(selector.clone(), BinaryOp::Eq, pattern_expr);
                    let fallback = else_expr.take().unwrap_or_else(|| Expression::StringLiteral("1'bx".to_string()));

                    else_expr = Some(Expression::ternary(condition, arm_value, fallback));
                }

                else_expr.ok_or_else(|| "Match expression has no arms".to_string())
            }

            Expr::Call(call_expr) => {
                // Special case: `Type::from_u128(n)` — convert to a literal.
                // This handles `Bits::from_u128(0)`, `Bits::<N>::from_u128(k)`, etc.
                // Used as the default/fallback arm in match expressions over Bits.
                let last_seg_name = match call_expr.func.as_ref() {
                    Expr::Path(path) => path.path.segments.last()
                        .map(|s| s.ident.to_string()),
                    _ => None,
                };
                if last_seg_name.as_deref() == Some("from_u128") {
                    if let Some(arg) = call_expr.args.first() {
                        if let Expr::Lit(lit_expr) = arg {
                            if let syn::Lit::Int(int_lit) = &lit_expr.lit {
                                if let Ok(val) = int_lit.base10_parse::<u128>() {
                                    // Use an unsized literal so Verilog zero-extends to
                                    // match the output width without WIDTHEXPAND warnings.
                                    return Ok(Expression::StringLiteral(val.to_string()));
                                }
                            }
                        }
                    }
                    // from_u128 with non-literal arg — emit zero (unsized)
                    return Ok(Expression::StringLiteral("0".to_string()));
                }

                // Lower function calls to a textual expression for now.
                // This enables sync-function calls inside lowered logic (e.g. inverter(x)).
                let func_name = match call_expr.func.as_ref() {
                    Expr::Path(path) => path.path.segments.last()
                        .map(|seg| Self::sanitize_identifier(&seg.ident.to_string()))
                        .unwrap_or_else(|| "unknown_fn".to_string()),
                    other => other.to_token_stream().to_string(),
                };

                let mut arg_strs = Vec::new();
                for arg in &call_expr.args {
                    let ir_arg = Self::parse_expression(arg)?;
                    arg_strs.push(Self::expression_to_inline_string(&ir_arg));
                }

                Ok(Expression::StringLiteral(format!("{}({})", func_name, arg_strs.join(", "))))
            }

            _ => {
                // Fall back to a placeholder for unsupported expression types
                // Ok(Expression::StringLiteral("unsupported_expr".to_string()))
                Err(format!(
                    "Unsupported expression type: {}",
                    expr.to_token_stream()
                ))
            }
        }
    }

    /// Extract the last expression from a block
    fn extract_last_expression(block: &Block) -> Result<Expression, String> {
        if let Some(syn::Stmt::Expr(expr, _)) = block.stmts.last() {
            Self::parse_expression(expr)
        } else {
            Err("Block has no expression".to_string())
        }
    }

    fn find_return_expr(block: &Block) -> Option<&Expr> {
        for stmt in &block.stmts {
            if let syn::Stmt::Expr(Expr::Return(ret_expr), _) = stmt {
                if let Some(expr) = &ret_expr.expr {
                    return Some(expr);
                }
            }
        }

        if let Some(syn::Stmt::Expr(expr, _)) = block.stmts.last() {
            return Some(expr);
        }

        None
    }

    fn infer_sync_output_targets(block: &Block) -> Result<Vec<String>, String> {
        let expr = Self::find_return_expr(block)
            .ok_or_else(|| "No return statement found in combinational function".to_string())?;

        let names = match expr {
            Expr::Tuple(tuple_expr) => tuple_expr
                .elems
                .iter()
                .enumerate()
                .map(|(idx, elem)| match elem {
                    Expr::Path(path) => path
                        .path
                        .get_ident()
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| format!("out{}", idx)),
                    _ => format!("out{}", idx),
                })
                .collect(),
            Expr::Path(path) => vec![path
                .path
                .get_ident()
                .map(|id| id.to_string())
                .unwrap_or_else(|| "out".to_string())],
            _ => vec!["out".to_string()],
        };

        Ok(names)
    }

    /// Parse the bindings from an `emit!(...)` call given a reference to the
    /// macro's token stream.  Accepts both `ExprMacro` and `StmtMacro` callers
    /// by taking the inner `syn::Macro` directly.
    fn parse_emit_bindings_from_mac(mac: &syn::Macro) -> Result<Vec<(String, Expression)>, String> {
        let args: Punctuated<Expr, Token![,]> = Punctuated::<Expr, Token![,]>::parse_terminated
            .parse2(mac.tokens.clone())
            .map_err(|e| format!("Failed to parse emit! args: {}", e))?;
        let arg_vec: Vec<Expr> = args.into_iter().collect();
        Self::parse_emit_bindings_args(&arg_vec)
    }

    fn parse_emit_bindings(macro_expr: &syn::ExprMacro) -> Result<Vec<(String, Expression)>, String> {
        Self::parse_emit_bindings_from_mac(&macro_expr.mac)
    }

    fn parse_emit_bindings_args(arg_vec: &[Expr]) -> Result<Vec<(String, Expression)>, String> {
        match arg_vec {
            [value] => match value {
                Expr::Tuple(tuple_expr) => {
                    let mut pairs = Vec::new();
                    for (idx, elem) in tuple_expr.elems.iter().enumerate() {
                        pairs.push((format!("out{}", idx), Self::parse_expression(elem)?));
                    }
                    Ok(pairs)
                }
                _ => Ok(vec![("out".to_string(), Self::parse_expression(value)?)]),
            },
            [target, value] => {
                let name = match target {
                    Expr::Path(path) => path
                        .path
                        .segments
                        .last()
                        .map(|seg| seg.ident.to_string())
                        .unwrap_or_else(|| "out".to_string()),
                    _ => "out".to_string(),
                };
                Ok(vec![(Self::sanitize_identifier(&name), Self::parse_expression(value)?)] )
            }
            _ => Err("Unsupported emit! form. Expected emit!(value) or emit!(target, value)".to_string()),
        }
    }

    fn collect_emit_output_names(block: &Block) -> Result<Vec<String>, String> {
        let mut names = Vec::<String>::new();
        Self::collect_emit_output_names_from_block(block, &mut names)?;

        if names.is_empty() {
            names.push("out".to_string());
        }

        Ok(names)
    }

    fn collect_emit_output_names_from_block(block: &Block, names: &mut Vec<String>) -> Result<(), String> {
        for stmt in &block.stmts {
            match stmt {
                syn::Stmt::Expr(Expr::Loop(loop_expr), _) => {
                    Self::collect_emit_output_names_from_block(&loop_expr.body, names)?;
                }
                syn::Stmt::Expr(Expr::If(if_expr), _) => {
                    Self::collect_emit_output_names_from_block(&if_expr.then_branch, names)?;
                    if let Some((_, else_body)) = &if_expr.else_branch {
                        if let Expr::Block(block_expr) = else_body.as_ref() {
                            Self::collect_emit_output_names_from_block(&block_expr.block, names)?;
                        }
                    }
                }
                // syn 2: expression-position macro (no trailing semicolon)
                syn::Stmt::Expr(Expr::Macro(macro_expr), _) if macro_expr.mac.path.is_ident("emit") => {
                    let bindings = Self::parse_emit_bindings(macro_expr)?;
                    for (name, _) in bindings {
                        if !names.iter().any(|existing| existing == &name) {
                            names.push(name);
                        }
                    }
                }
                // syn 2: statement-position macro (trailing semicolon → Stmt::Macro)
                syn::Stmt::Macro(stmt_mac) if stmt_mac.mac.path.is_ident("emit") => {
                    let bindings = Self::parse_emit_bindings_from_mac(&stmt_mac.mac)?;
                    for (name, _) in bindings {
                        if !names.iter().any(|existing| existing == &name) {
                            names.push(name);
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn lower_call_to_submodule(
        call_expr: &syn::ExprCall,
        deps: &HashMap<String, ItemFn>,
        artifacts: &mut LoweringArtifacts,
        aliases: &HashMap<String, String>,
    ) -> Result<Expression, String> {
        let callee = match call_expr.func.as_ref() {
            Expr::Path(path) => path.path.segments.last().map(|s| s.ident.to_string()),
            _ => None,
        }
        .ok_or_else(|| "Unsupported function call target".to_string())?;

        let callee_fn = deps.get(&callee)
            .ok_or_else(|| format!("Unknown called function '{}'. Ensure it exists in the same source file.", callee))?;

        if callee_fn.sig.asyncness.is_some() {
            return Err(format!("Async callee '{}' is not supported as a submodule instance", callee));
        }

        let input_ports: Vec<String> = callee_fn.sig.inputs.iter().filter_map(|arg| {
            if let FnArg::Typed(pt) = arg {
                if let Pat::Ident(ident) = &*pt.pat {
                    return Some(Self::sanitize_identifier(&ident.ident.to_string()));
                }
            }
            None
        }).collect();

        if input_ports.len() != call_expr.args.len() {
            return Err(format!(
                "Call '{}' argument count mismatch: expected {}, got {}",
                callee,
                input_ports.len(),
                call_expr.args.len()
            ));
        }

        let output_ports = Self::infer_sync_output_targets(&callee_fn.block)?;
        if output_ports.is_empty() {
            return Err(format!("Callee '{}' has no output", callee));
        }

        let width = match &callee_fn.sig.output {
            ReturnType::Type(_, ty) => Self::infer_width_from_type(ty),
            ReturnType::Default => 8,
        };

        let output_wire = format!("__{}_out_{}", Self::sanitize_identifier(&callee), artifacts.next_wire_id);
        artifacts.next_wire_id += 1;

        artifacts.internal_wires.push(WireDecl {
            name: output_wire.clone(),
            width,
        });

        let mut connections = Vec::<PortConnection>::new();
        for (port, arg) in input_ports.iter().zip(call_expr.args.iter()) {
            let signal = if let Expr::Path(path) = arg {
                if let Some(id) = path.path.get_ident() {
                    if let Some(alias) = aliases.get(&id.to_string()) {
                        alias.clone()
                    } else {
                        let arg_expr = Self::parse_expression(arg)?;
                        Self::expression_to_inline_string(&arg_expr)
                    }
                } else {
                    let arg_expr = Self::parse_expression(arg)?;
                    Self::expression_to_inline_string(&arg_expr)
                }
            } else {
                let arg_expr = Self::parse_expression(arg)?;
                Self::expression_to_inline_string(&arg_expr)
            };

            connections.push(PortConnection {
                port: port.clone(),
                signal,
            });
        }

        connections.push(PortConnection {
            port: Self::sanitize_identifier(&output_ports[0]),
            signal: output_wire.clone(),
        });

        let instance_name = format!("{}_inst_{}", Self::sanitize_identifier(&callee), artifacts.next_instance_id);
        artifacts.next_instance_id += 1;

        artifacts.submodules.push(SubmoduleInstance {
            module_name: callee,
            instance_name,
            connections,
        });

        Ok(Expression::var(output_wire))
    }

    /// Extract the `PatIdent` from a pattern, unwrapping a `Pat::Type` wrapper
    /// if present.  In syn 2, `let mut x: T = val` produces `Pat::Type { pat:
    /// Pat::Ident(x), ty: T }` rather than a bare `Pat::Ident`.
    fn pat_to_ident(pat: &Pat) -> Option<&syn::PatIdent> {
        match pat {
            Pat::Ident(ident) => Some(ident),
            Pat::Type(pat_type) => {
                if let Pat::Ident(ident) = &*pat_type.pat {
                    Some(ident)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn extract_signal_source(expr: &Expr) -> Option<String> {
        match expr {
            Expr::Path(path) => path.path.get_ident().map(|id| id.to_string()),
            Expr::Unary(unary) => Self::extract_signal_source(&unary.expr),
            Expr::Paren(paren) => Self::extract_signal_source(&paren.expr),
            Expr::Reference(reference) => Self::extract_signal_source(&reference.expr),
            Expr::MethodCall(method) => {
                // Only follow through accessor/unwrapping methods that don't transform
                // the value. For computation methods (wrapping_add, etc.) return None
                // so the caller falls back to full expression parsing.
                let method_name = method.method.to_string();
                if matches!(
                    method_name.as_str(),
                    "lock" | "unwrap" | "clone" | "borrow" | "as_ref" | "as_mut" | "into"
                ) {
                    Self::extract_signal_source(&method.receiver)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn infer_width_from_expr(expr: &Expr) -> usize {
        match expr {
            Expr::Path(path) => {
                let name = path.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
                if name == "ZERO" || name == "ONE" || name == "X" {
                    1
                } else {
                    8
                }
            }
            Expr::Field(field_expr) => Self::infer_width_from_expr(&field_expr.base),
            Expr::Unary(unary_expr) => Self::infer_width_from_expr(&unary_expr.expr),
            _ => 8,
        }
    }

    /// Recursively substitute alias variable names in an expression.
    ///
    /// When the ir_builder processes `let input_val = *sig.lock().unwrap()` it
    /// adds `"input_val" → "in_data"` to the alias table.  Later expressions
    /// that reference `input_val` must use `in_data` (the actual Verilog port)
    /// instead.  Without this substitution the generated always block would
    /// reference undeclared temporaries.
    fn apply_aliases(expr: Expression, aliases: &HashMap<String, String>) -> Expression {
        match expr {
            Expression::Var(name) => {
                if let Some(resolved) = aliases.get(&name) {
                    Expression::Var(resolved.clone())
                } else {
                    Expression::Var(name)
                }
            }
            Expression::BinaryOp { left, op, right } => Expression::BinaryOp {
                left:  Box::new(Self::apply_aliases(*left,  aliases)),
                op,
                right: Box::new(Self::apply_aliases(*right, aliases)),
            },
            Expression::UnaryOp { op, operand } => Expression::UnaryOp {
                op,
                operand: Box::new(Self::apply_aliases(*operand, aliases)),
            },
            Expression::Ternary { condition, then_val, else_val } => Expression::Ternary {
                condition: Box::new(Self::apply_aliases(*condition, aliases)),
                then_val:  Box::new(Self::apply_aliases(*then_val,  aliases)),
                else_val:  Box::new(Self::apply_aliases(*else_val,  aliases)),
            },
            Expression::Concat(parts) => {
                Expression::Concat(parts.into_iter().map(|e| Self::apply_aliases(e, aliases)).collect())
            }
            Expression::BitSelect { signal, index } => Expression::BitSelect {
                signal: Box::new(Self::apply_aliases(*signal, aliases)),
                index:  Box::new(Self::apply_aliases(*index,  aliases)),
            },
            Expression::RangeSelect { signal, high, low } => Expression::RangeSelect {
                signal: Box::new(Self::apply_aliases(*signal, aliases)),
                high:   Box::new(Self::apply_aliases(*high,   aliases)),
                low:    Box::new(Self::apply_aliases(*low,    aliases)),
            },
            // Leaf nodes — pass through unchanged
            other => other,
        }
    }

    fn sanitize_identifier(name: &str) -> String {
        let keywords = [
            "input", "output", "inout", "wire", "reg", "module", "endmodule", "always",
            "assign", "begin", "end", "if", "else", "case", "endcase", "default",
            "bit",
        ];

        if keywords.iter().any(|kw| kw == &name) {
            format!("{}_sig", name)
        } else {
            name.to_string()
        }
    }

    /// Parse a binary operator
    fn parse_binary_op(op: &syn::BinOp) -> Result<BinaryOp, String> {
        use syn::BinOp;
        Ok(match op {
            BinOp::Add(_) => BinaryOp::Add,
            BinOp::Sub(_) => BinaryOp::Sub,
            BinOp::Mul(_) => BinaryOp::Mul,
            BinOp::Div(_) => BinaryOp::Div,
            BinOp::Rem(_) => BinaryOp::Mod,
            BinOp::BitAnd(_) => BinaryOp::And,
            BinOp::BitOr(_) => BinaryOp::Or,
            BinOp::BitXor(_) => BinaryOp::Xor,
            BinOp::Shl(_) => BinaryOp::LeftShift,
            BinOp::Shr(_) => BinaryOp::RightShift,
            BinOp::And(_) => BinaryOp::LogicalAnd,
            BinOp::Or(_) => BinaryOp::LogicalOr,
            BinOp::Eq(_) => BinaryOp::Eq,
            BinOp::Ne(_) => BinaryOp::Neq,
            BinOp::Lt(_) => BinaryOp::Lt,
            BinOp::Le(_) => BinaryOp::Lte,
            BinOp::Gt(_) => BinaryOp::Gt,
            BinOp::Ge(_) => BinaryOp::Gte,
            _ => return Err("Unsupported binary operator".to_string()),
        })
    }

    /// Parse a unary operator
    fn parse_unary_op(op: &syn::UnOp) -> Result<UnaryOp, String> {
        use syn::UnOp;
        match op {
            UnOp::Not(_) => Ok(UnaryOp::BitwiseNot),
            UnOp::Neg(_) => Err("Negation not supported in hardware".to_string()),
            UnOp::Deref(_) => Err("Deref not supported in hardware".to_string()),
            _ => Err("Unknown unary operator".to_string()),
        }
    }

    fn pattern_to_expression(pat: &Pat) -> Result<Expression, String> {
        match pat {
            Pat::Path(path_pat) => {
                if let Some(expr) = Self::path_to_literal_or_const(&path_pat.path) {
                    Ok(expr)
                } else {
                    let text = path_pat.path.segments.iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::");
                    Ok(Expression::var(Self::sanitize_identifier(&text)))
                }
            }
            Pat::TupleStruct(tuple_pat) => {
                // Handle tuple-struct enum patterns like `Logic::Zero(..)` by path name.
                if let Some(expr) = Self::path_to_literal_or_const(&tuple_pat.path) {
                    Ok(expr)
                } else {
                    Err("Unsupported tuple-struct match pattern".to_string())
                }
            }
            Pat::Lit(pat_lit) => {
                match &pat_lit.lit {
                    syn::Lit::Int(int_lit) => {
                        let value = int_lit.base10_parse::<u128>()
                            .map_err(|_| "Failed to parse integer literal in pattern".to_string())?;
                        // Use an unsized integer literal (no width prefix) so that
                        // comparisons don't trigger Verilator's WIDTHEXPAND warning.
                        // Verilog automatically extends unsized literals to match context.
                        Ok(Expression::StringLiteral(value.to_string()))
                    }
                    _ => Err("Unsupported literal match pattern".to_string()),
                }
            }
            _ => Err("Unsupported match pattern".to_string()),
        }
    }

    fn path_to_literal_or_const(path: &syn::Path) -> Option<Expression> {
        let last = path.segments.last()?.ident.to_string();
        match last.as_str() {
            "Zero" | "ZERO" => Some(Expression::Literal { width: 1, value: 0 }),
            "One" | "ONE" => Some(Expression::Literal { width: 1, value: 1 }),
            "X" => Some(Expression::StringLiteral("1'bx".to_string())),
            _ => None,
        }
    }

    fn expression_to_inline_string(expr: &Expression) -> String {
        match expr {
            Expression::Var(name) => name.clone(),
            Expression::Literal { width, value } => format!("{}'d{}", width, value),
            Expression::StringLiteral(s) => s.clone(),
            Expression::BinaryOp { left, op, right } => {
                let op_str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Mod => "%",
                    BinaryOp::And => "&",
                    BinaryOp::Or => "|",
                    BinaryOp::Xor => "^",
                    BinaryOp::LeftShift => "<<",
                    BinaryOp::RightShift => ">>",
                    BinaryOp::LogicalAnd => "&&",
                    BinaryOp::LogicalOr => "||",
                    BinaryOp::Eq => "==",
                    BinaryOp::Neq => "!=",
                    BinaryOp::Lt => "<",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Gt => ">",
                    BinaryOp::Gte => ">=",
                };
                format!(
                    "({} {} {})",
                    Self::expression_to_inline_string(left),
                    op_str,
                    Self::expression_to_inline_string(right)
                )
            }
            Expression::UnaryOp { op, operand } => {
                let op_str = match op {
                    UnaryOp::BitwiseNot => "~",
                    UnaryOp::LogicalNot => "!",
                    UnaryOp::ReductionAnd => "&",
                    UnaryOp::ReductionOr => "|",
                    UnaryOp::ReductionXor => "^",
                };
                format!("({}{})", op_str, Self::expression_to_inline_string(operand))
            }
            Expression::Ternary { condition, then_val, else_val } => format!(
                "({} ? {} : {})",
                Self::expression_to_inline_string(condition),
                Self::expression_to_inline_string(then_val),
                Self::expression_to_inline_string(else_val)
            ),
            Expression::BitSelect { signal, index } => format!(
                "{}[{}]",
                Self::expression_to_inline_string(signal),
                Self::expression_to_inline_string(index)
            ),
            Expression::RangeSelect { signal, high, low } => format!(
                "{}[{}:{}]",
                Self::expression_to_inline_string(signal),
                Self::expression_to_inline_string(high),
                Self::expression_to_inline_string(low)
            ),
            Expression::Concat(parts) => {
                let rendered: Vec<String> = parts.iter().map(Self::expression_to_inline_string).collect();
                format!("{{{}}}", rendered.join(", "))
            }
        }
    }
}

/// Module type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleType {
    /// Pure combinational logic (no state)
    Combinational,
    /// Sequential logic with state
    Sequential,
    /// Async but no main loop (unsupported)
    AsyncCombinational,
}

#[cfg(test)]
#[path = "ir_builder_extract_ports_tests.rs"]
mod ir_builder_extract_ports_tests;

#[cfg(test)]
mod sequential_codegen_tests {
    use crate::{to_verilog_from_str, IRBuilder};

    fn codegen(code: &str) -> String {
        to_verilog_from_str(code)
    }

    fn build_ir(code: &str) -> copper_core::ir::ModuleIR {
        let func: syn::ItemFn = syn::parse_str(code).expect("parse failed");
        IRBuilder::from_function(&func).expect("IR build failed")
    }

    // ── Statement extraction tests ───────────────────────────────────────────

    #[test]
    fn sequential_extracts_registers_from_let_mut() {
        let ir = build_ir(r#"
            async fn counter(clk: Clock<MainClk>) -> u8 {
                let mut count: u8 = 0;
                loop {
                    emit!(count);
                    clk.tick().await;
                }
            }
        "#);
        match &ir.logic {
            copper_core::ir::ModuleLogic::Sequential { registers, .. } => {
                assert_eq!(registers.len(), 1);
                assert_eq!(registers[0].name, "count");
            }
            other => panic!("expected Sequential, got {:?}", other),
        }
    }

    #[test]
    fn sequential_emit_becomes_continuous_assign() {
        let ir = build_ir(r#"
            async fn counter(clk: Clock<MainClk>) -> u8 {
                let mut count: u8 = 0;
                loop {
                    emit!(count);
                    clk.tick().await;
                }
            }
        "#);
        // emit!(count) should produce a continuous assign outside the always block,
        // not a non-blocking assign inside it. This matches Rust tick_clock() semantics
        // where the output reflects the newly-computed register value immediately.
        match &ir.logic {
            copper_core::ir::ModuleLogic::Sequential { always_block, output_assigns, .. } => {
                // The emit! should NOT be in the always block
                let no_out_nba = always_block.statements.iter().all(|s| {
                    !matches!(s, copper_core::ir::Statement::NonBlockingAssign { target, .. } if target == "out")
                });
                assert!(no_out_nba, "emit! produced a NBA in always block; expected continuous assign");

                // The emit! SHOULD be in output_assigns
                let has_out = output_assigns.iter().any(|a| a.target == "out");
                assert!(has_out, "expected output_assigns to contain 'out'");
            }
            other => panic!("expected Sequential, got {:?}", other),
        }
    }

    #[test]
    fn sequential_let_computed_becomes_blocking_assign() {
        let ir = build_ir(r#"
            async fn pipeline(clk: Clock<MainClk>, in_data: Arc<Mutex<u8>>) -> u8 {
                let mut reg: u8 = 0;
                loop {
                    emit!(reg);
                    clk.tick().await;
                    let next = reg.wrapping_add(1u8);
                    reg = next;
                }
            }
        "#);
        match &ir.logic {
            copper_core::ir::ModuleLogic::Sequential { always_block, .. } => {
                // Should contain: out <= reg, then next = reg + 1, then reg <= next
                let has_blocking = always_block.statements.iter().any(|s| {
                    matches!(s, copper_core::ir::Statement::BlockingAssign { target, .. } if target == "next")
                });
                assert!(has_blocking, "expected blocking assignment for `let next = ...`");
            }
            other => panic!("expected Sequential, got {:?}", other),
        }
    }

    #[test]
    fn sequential_match_stmt_becomes_case() {
        let ir = build_ir(r#"
            async fn fsm(clk: Clock<MainClk>, input: Arc<Mutex<u8>>) -> u8 {
                let mut state: u8 = 0;
                loop {
                    emit!(state);
                    clk.tick().await;
                    let inp = *input.lock().unwrap();
                    match inp {
                        0 => { state = 1; }
                        1 => { state = 2; }
                        _ => { state = 0; }
                    }
                }
            }
        "#);
        match &ir.logic {
            copper_core::ir::ModuleLogic::Sequential { always_block, .. } => {
                let has_case = always_block.statements.iter().any(|s| {
                    matches!(s, copper_core::ir::Statement::Case { .. })
                });
                assert!(has_case, "expected case statement from match");
            }
            other => panic!("expected Sequential, got {:?}", other),
        }
    }

    // ── Verilog output tests ─────────────────────────────────────────────────

    #[test]
    fn sequential_verilog_has_always_block() {
        let verilog = codegen(r#"
            async fn counter(clk: Clock<MainClk>) -> u8 {
                let mut count: u8 = 0;
                loop {
                    emit!(count);
                    clk.tick().await;
                    count = count.wrapping_add(1u8);
                }
            }
        "#);
        assert!(verilog.contains("always @(posedge clk)"), "missing always block:\n{}", verilog);
        // emit!(count) → continuous assign outside always block
        assert!(verilog.contains("assign out = count"), "missing continuous output assign:\n{}", verilog);
        // register update → non-blocking inside always block
        assert!(verilog.contains("count <= (count + 8'd1)"), "missing register update:\n{}", verilog);
    }

    #[test]
    fn sequential_verilog_wrapping_add_becomes_add() {
        let verilog = codegen(r#"
            async fn adder(clk: Clock<MainClk>) -> u8 {
                let mut x: u8 = 0;
                loop {
                    emit!(x);
                    clk.tick().await;
                    x = x.wrapping_add(3u8);
                }
            }
        "#);
        assert!(verilog.contains("x <= (x + 8'd3)") || verilog.contains("x <= (x +"),
            "expected wrapping_add to lower to +:\n{}", verilog);
    }

    #[test]
    fn sequential_verilog_let_computed_produces_blocking_assign() {
        let verilog = codegen(r#"
            async fn pipeline(clk: Clock<MainClk>, in_data: Arc<Mutex<u8>>) -> u8 {
                let mut reg: u8 = 0;
                loop {
                    emit!(reg);
                    clk.tick().await;
                    let next = reg.wrapping_add(1u8);
                    reg = next;
                }
            }
        "#);
        // next should be a blocking assignment (=), reg update non-blocking (<=)
        // Note: `reg` is a Verilog keyword so the sanitizer renames it to `reg_sig`
        assert!(verilog.contains("next = (reg_sig +"), "expected blocking assign for next:\n{}", verilog);
        assert!(verilog.contains("reg_sig <= next"), "expected non-blocking assign for reg:\n{}", verilog);
    }

    #[test]
    fn sequential_verilog_match_stmt_produces_case() {
        let verilog = codegen(r#"
            async fn fsm(clk: Clock<MainClk>, input: Arc<Mutex<u8>>) -> u8 {
                let mut state: u8 = 0;
                loop {
                    emit!(state);
                    clk.tick().await;
                    let inp = *input.lock().unwrap();
                    match inp {
                        0 => { state = 1; }
                        1 => { state = 2; }
                        _ => { state = 0; }
                    }
                }
            }
        "#);
        assert!(verilog.contains("case (inp)") || verilog.contains("case (input"),
            "expected case statement:\n{}", verilog);
        assert!(verilog.contains("default:"), "expected default arm:\n{}", verilog);
    }
}
