use copper_core::{ModuleIR, Port, Logic};
use copper_core::ir::{Statement, Expression, Signal};
use syn::{ItemFn, Stmt, Expr, ExprMatch};
use quote::{ToTokens, quote};
use syn::ExprMethodCall;
use log::{debug, warn, error};


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

pub struct IRBuilder {
    ports: Vec<Port>,
    statements: Vec<Statement>,
    locals: std::collections::HashMap<String, Expression>,
}

impl IRBuilder {
    pub fn from_ast(design_fn: &ItemFn, ports: Vec<Port>) -> Result<ModuleIR, LowerError> {
        let mut builder = IRBuilder {
            ports,
            statements: Vec::new(),
            locals: std::collections::HashMap::new(),
        };

        builder.lower_statements(&design_fn.block.stmts)?;

        Ok(ModuleIR {
            name: "".to_string(), // will be filled by caller
            ports: builder.ports,
            statements: builder.statements,
            submodules: Vec::new(), 
        })
    }

    fn lower_statements(&mut self, stmts: &[Stmt]) -> Result<(), LowerError> {
        for stmt in stmts {
            if let Some(hw_stmt) = self.lower_statement(stmt)? {
                self.statements.push(hw_stmt);
            }
        }
        Ok(())
    }

    fn lower_statement(&mut self, stmt: &Stmt) -> Result<Option<Statement>, LowerError> {
        match stmt {
            Stmt::Expr(expr, _) => {
                self.lower_expr_stmt(expr)
            }
            Stmt::Local(local) => {
                // Handle: let x = value;
                if let Some(init) = &local.init {
                    let value = self.lower_expr(&init.expr)?;
                    let name = match &local.pat {
                        syn::Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
                        _ => {
                            warn!("Unsupported pattern in let statement: {}", quote!(#local.pat));
                            return Err(LowerError::UnsupportedStmt("complex patterns in let".to_string()));
                        }
                    };
                    debug!("Lowered local variable: {} = {:?}", name, value);
                    self.locals.insert(name, value);
                    Ok(None)  // Locals don't emit statements
                } else {
                    error!("Let statement without initialization");
                    Err(LowerError::UnsupportedStmt("let without initialization".to_string()))
                }
            }
            _ => {
                let stmt_type = std::any::type_name_of_val(stmt);
                let stmt_tokens = quote!(#stmt).to_string();
                error!("Unsupported statement type: {}, content: {}", stmt_type, stmt_tokens);
                Err(LowerError::UnsupportedStmt(
                    format!("statement type: {}", stmt_type)
                ))
            }
        }
    }


    fn lower_expr(&mut self, expr: &Expr) -> Result<Expression, LowerError> {
        match expr {
            // Literals: numbers, booleans, identifiers
            Expr::Lit(lit) => {
                let lit_str = lit.to_token_stream().to_string();
                Ok(Expression::Literal(lit_str))
            }

            // Paths: Logic::Zero, variable names, etc.
            Expr::Path(path_expr) => {
                let ident = path_expr.path.segments.last()
                    .ok_or_else(|| LowerError::UnsupportedExpr("empty path".to_string()))?
                    .ident.to_string();
                
                // Check if it's a local variable
                if let Some(expr) = self.locals.get(&ident) {
                    return Ok(expr.clone());
                }
                
                // Otherwise treat as literal (e.g., Logic::Zero)
                Ok(Expression::Literal(ident))
            }

            // Array literal: [expr, expr, ...]
            Expr::Array(array) => {
                if array.elems.is_empty() {
                    return Err(LowerError::UnsupportedExpr("empty array".to_string()));
                }
                if array.elems.len() == 1 {
                    // Single-element array: unwrap it
                    return self.lower_expr(&array.elems[0]);
                }
                // Multi-element array: create Concat
                let mut elems = Vec::new();
                for elem in &array.elems {
                    elems.push(self.lower_expr(elem)?);
                }
                Ok(Expression::Concat(elems))
            }

            // Indexing: array[index]
            Expr::Index(index_expr) => {
                let base = self.lower_expr(&index_expr.expr)?;
                let index = self.lower_expr(&index_expr.index)?;
                Ok(Expression::Index(Box::new(base), Box::new(index)))
            }

            // Method calls: get_value(), map(), etc.
            Expr::MethodCall(method_call) => {
                if method_call.method == "get_value" {
                    let signal = self.extract_signal(&method_call.receiver)
                        .ok_or_else(|| LowerError::SignalNotFound("get_value receiver".to_string()))?;
                    Ok(Expression::Signal(signal))
                } else if method_call.method == "map" {
                    // Handle: array.map(|x| body)
                    let array_expr = self.lower_expr(&method_call.receiver)?;
                    let closure = method_call.args.first()
                        .ok_or_else(|| LowerError::MissingArgument("map closure".to_string()))?;
                    self.lower_map(&array_expr, closure)
                } else {
                    warn!("Unsupported method call: {}", method_call.method);
                    Err(LowerError::UnsupportedExpr(format!("method: {}", method_call.method)))
                }
            }

            // Match expressions: match value { arm1 => ..., arm2 => ... }
            Expr::Match(match_expr) => {
                self.lower_match(match_expr)
            }

            // If expressions: if cond { then } else { else }
            Expr::If(_if_expr) => {
                warn!("Unsupported If expression encountered");
                Err(LowerError::UnsupportedExpr("if expression".to_string()))
            }

            // Block: { ... }
            Expr::Block(block_expr) => {
                if let Some(Stmt::Expr(inner, _)) = block_expr.block.stmts.first() {
                    self.lower_expr(inner)
                } else {
                    Err(LowerError::UnsupportedExpr("empty block".to_string()))
                }
            }

            // Parenthesized: (expr)
            Expr::Paren(paren_expr) => {
                self.lower_expr(&paren_expr.expr)
            }

            // Everything else is unsupported
            _ => {
                let expr_tokens = quote!(#expr).to_string();
                error!("Unsupported expression type: {}, content: {}", 
                    std::any::type_name_of_val(expr), expr_tokens);
                Err(LowerError::UnsupportedExpr(
                    std::any::type_name_of_val(expr).to_string()
                ))
            }
        }
    }

    fn lower_match(&mut self, match_expr: &ExprMatch) -> Result<Expression, LowerError> {
        use copper_core::ir::{CaseExprArm, Pattern};
        
        debug!("Lowering match expression with {} arms", match_expr.arms.len());
        
        // Lower the scrutinee (the value being matched)
        let selector = self.lower_expr(&match_expr.expr)?;
        
        // Lower all arms into case expression arms
        let mut arms = Vec::new();
        for (idx, arm) in match_expr.arms.iter().enumerate() {
            // Extract pattern - for now we support literal paths like Logic::Zero
            let pattern = match &arm.pat {
                syn::Pat::Path(path_pat) => {
                    let path_str = quote!(#path_pat).to_string();
                    debug!("Arm {}: pattern = {}", idx, path_str);
                    
                    // Parse the path to determine the pattern
                    // Logic::Zero -> Literal(Bit(Logic::Zero))
                    // Logic::One -> Literal(Bit(Logic::One))
                    // Logic::X -> Literal(Bit(Logic::X))
                    if path_str.contains("Zero") {
                        Pattern::Literal(copper_core::ir::LogicValue::Bit(Logic::Zero))
                    } else if path_str.contains("One") {
                        Pattern::Literal(copper_core::ir::LogicValue::Bit(Logic::One))
                    } else if path_str.contains("X") {
                        Pattern::Literal(copper_core::ir::LogicValue::Bit(Logic::X))
                    } else {
                        warn!("Unknown pattern in match arm: {}", path_str);
                        continue;  // Skip unknown patterns for now
                    }
                }
                syn::Pat::Wild(_) => {
                    debug!("Arm {}: default pattern", idx);
                    Pattern::Default
                }
                _ => {
                    warn!("Unsupported pattern type in match arm: {}", quote!(#arm.pat));
                    continue;
                }
            };
            
            // Lower the arm body expression
            let value = self.lower_expr(&arm.body)?;
            debug!("Arm {}: body = {:?}", idx, value);
            
            arms.push(CaseExprArm {
                pattern,
                value: Box::new(value),
            });
        }
        
        if arms.is_empty() {
            return Err(LowerError::UnsupportedExpr("match with no valid arms".to_string()));
        }
        
        Ok(Expression::Case {
            selector: Box::new(selector),
            arms,
        })
    }

    fn lower_map(&mut self, array_expr: &Expression, closure: &Expr) -> Result<Expression, LowerError> {
        // Handle: array.map(|param| body)
        if let Expr::Closure(closure_expr) = closure {
            debug!("Lowering map closure with {} inputs", closure_expr.inputs.len());
            
            // Extract the parameter name from the closure
            let param_name = match closure_expr.inputs.first() {
                Some(syn::Pat::Ident(pat_ident)) => pat_ident.ident.to_string(),
                Some(pat) => {
                    warn!("Unsupported closure pattern: {}", quote!(#pat));
                    return Err(LowerError::UnsupportedExpr("complex closure patterns".to_string()));
                }
                None => {
                    return Err(LowerError::UnsupportedExpr("closure with no parameters".to_string()));
                }
            };
            
            debug!("Map closure parameter: {}", param_name);
            
            // Save the current locals state
            let saved_locals = self.locals.clone();
            
            // Bind the parameter to the array expression
            // The parameter represents each element as we iterate
            self.locals.insert(param_name.clone(), array_expr.clone());
            
            let result = self.lower_expr(&closure_expr.body)?;
            
            // Restore locals
            self.locals = saved_locals;
            
            debug!("Map closure body evaluated to: {:?}", result);
            
            Ok(result)
        } else {
            warn!("Map argument is not a closure: {}", std::any::type_name_of_val(closure));
            Err(LowerError::UnsupportedExpr("map with non-closure argument".to_string()))
        }
    }

    fn lower_expr_stmt(&mut self, expr: &Expr) -> Result<Option<Statement>, LowerError> {
        // Look for: self.output.set_value(...)
        if let Expr::MethodCall(method_call) = expr {
            if method_call.method == "set_value" {
                return Ok(Some(self.lower_assignment(method_call)?));
            } else {
                debug!("Expression statement with method call (not set_value): {}", method_call.method);
            }
        } else {
            debug!("Expression statement (not a method call): {}", std::any::type_name_of_val(expr));
        }
        Ok(None)
    }

    fn lower_assignment(&mut self, method_call: &ExprMethodCall) -> Result<Statement, LowerError> {
        let target = self.extract_signal(&method_call.receiver)
            .ok_or_else(|| LowerError::SignalNotFound("target of set_value".to_string()))?;
        
        let value_expr = method_call.args.first()
            .ok_or_else(|| LowerError::MissingArgument("set_value argument".to_string()))?;
        let value = self.lower_expr(value_expr)?;
        
        debug!("Lowered assignment to {}: {:?}", target.name, value);
        Ok(Statement::Assign { target, value })
    }    


    fn extract_signal(&self, expr: &Expr) -> Option<Signal> {
        // Parse: self.field_name
        if let Expr::Field(field_expr) = expr {
            if let Expr::Path(base) = &*field_expr.base {
                if base.path.is_ident("self") {
                    let name = field_expr.member.to_token_stream().to_string();
                    // Look up width from ports
                    let width = self.ports.iter()
                        .find(|p| p.name == name)
                        .map(|p| p.width)?;
                    
                    return Some(Signal { name, width });
                }
            }
        }
        None
    }
}

