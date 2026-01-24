use copper_core::{ModuleIR, Port};
use copper_core::ir::{UnaryOp, BinaryOp, Statement, Expression, Signal, LogicValue};
use syn::{ItemFn, Stmt, Expr, ExprMatch};
use quote::ToTokens;
use syn::ExprMethodCall;

pub struct IRBuilder {
    ports: Vec<Port>,
    statements: Vec<Statement>,
}

impl IRBuilder {
    pub fn from_ast(design_fn: &ItemFn, ports: Vec<Port>) -> ModuleIR {
        let mut builder = IRBuilder {
            ports,
            statements: Vec::new(),
        };

        builder.parse_statements(&design_fn.block.stmts);

        ModuleIR {
            name: "".to_string(), // will be filled by caller
            ports: builder.ports,
            statements: builder.statements,
            submodules: Vec::new(), 
        }
    }

    fn parse_statements(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            if let Some(hw_stmt) = self.parse_statement(stmt) {
                self.statements.push(hw_stmt);
            }
        }
    }

    fn parse_statement(&mut self, stmt: &Stmt) -> Option<Statement> {
        match stmt {
            Stmt::Expr(expr, _) => self.parse_expr_stmt(expr),
            _ => None,
        }
    }

    fn parse_expr_stmt(&mut self, expr: &Expr) -> Option<Statement> {
        // look for: self.output.set_value([...])
        if let Expr::MethodCall(method_call) = expr {
            if method_call.method == "set_value" {
                return self.parse_assignment(method_call);
            }
        }
        None
    }

    fn parse_assignment(&mut self, method_call: &ExprMethodCall) -> Option<Statement> {
        // Extract target signal
        let target = self.extract_signal(&method_call.receiver)?;
        
        // Extract value expression
        let value_expr = method_call.args.first()?;
        let value = self.parse_expression(value_expr)?;
        print!("Parsed assignment to {}: {:?}\n", target.name, value);
        
        Some(Statement::Assign { target, value })
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
    
    fn parse_expression(&self, expr: &Expr) -> Option<Expression> {
        // Debug: print the actual variant name
        let expr_type = match expr {
            Expr::Array(_) => "Array",
            Expr::Match(_) => "Match",
            Expr::Index(_) => "Index",
            Expr::MethodCall(_) => "MethodCall",
            Expr::Block(_) => "Block",
            Expr::Field(_) => "Field",
            Expr::Path(_) => "Path",
            Expr::Lit(_) => "Lit",
            Expr::Paren(_) => "Paren",
            _ => "Other",
        };
        print!("parse_expression called with type: {}\n", expr_type);
        match expr {
            // Array literal: [expr]
            Expr::Array(array) => {
                if let Some(first) = array.elems.first() {
                    print!("Parsing array expression...\n");
                    return self.parse_expression(first);
                }
                None
            }
            
            // Match expression
            Expr::Match(match_expr) => {
                println!("Parsing match expression...");
                self.parse_match_expression(match_expr)
            }
            
            Expr::Index(index_expr) => {
                print!("Parsing index expression...\n");
                let base = self.parse_expression(&index_expr.expr)?;
                let index = self.parse_expression(&index_expr.index)?;
                return Some(Expression::Index(Box::new(base), Box::new(index)));
            }
            
            // Method call: self.signal.get_value()
            Expr::MethodCall(method_call) => {
                if method_call.method == "get_value" {
                    print!("Parsing get_value method call...\n");
                    let signal = self.extract_signal(&method_call.receiver)?;
                    print!("Extracted signal: {}\n", signal.name);
                    return Some(Expression::Signal(signal));
                }
                None
            }

            Expr::Block(block_expr) => {
                print!("Parsing block expression...\n");
                // Try to parse the first statement in the block
                if let Some(stmt) = block_expr.block.stmts.first() {
                    if let Stmt::Expr(inner_expr, _) = stmt {
                        return self.parse_expression(inner_expr);
                    }
                }
                None
            }

            Expr::Lit(lit_expr) => {
                print!("Parsing literal expression...\n");
                // Convert the literal to a string representation
                let lit_str = lit_expr.to_token_stream().to_string();
                print!("Literal value: {}\n", lit_str);
                // Store as a Literal expression (now accepts String)
                Some(Expression::Literal(lit_str))
            }
            
            _ => {
                print!("No match for expression type: {:?}\n", std::any::type_name_of_val(expr));
                None
            }
        }
    }
    
    fn parse_match_expression(&self, match_expr: &ExprMatch) -> Option<Expression> {
        // Detect common patterns
        
        // Pattern 1: Inversion (Zero => One, One => Zero, X => X)
        if self.is_inversion_pattern(match_expr) {
            print!("Detected inversion pattern...\n");
            let input = self.parse_expression(&match_expr.expr)?;
            print!("Creating NOT expression...\n");
            return Some(Expression::UnaryOp(UnaryOp::Not, Box::new(input)));
        }
        print!("No known pattern matched in match expression.\n");
        
        // Pattern 2: AND/OR/XOR gates
        // Pattern 3: MUX
        
        // Fallback: convert to case statement
        None
    }
    
    fn is_inversion_pattern(&self, match_expr: &ExprMatch) -> bool {
        // Check if arms match inversion pattern
        // Simplified for now
        print!("Checking inversion pattern...\n");
        true
    }
}


