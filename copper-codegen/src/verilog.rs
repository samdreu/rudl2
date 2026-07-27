use copper_core::{ModuleIR, Logic};
use copper_core::ir::{UnaryOp, BinaryOp, Statement, Expression, Pattern, LogicValue};
use log::{debug, info, warn};

pub struct VerilogGenerator;

impl VerilogGenerator {
    pub fn generate(ir: &ModuleIR) -> String {
        info!("Generating Verilog for module: {}", ir.name);
        debug!("IR has {} statements", ir.statements.len());
        debug!("IR has {} ports", ir.ports.len());
        
        // Validate port names before generating
        for port in &ir.ports {
            if let Err(e) = Self::validate_identifier(&port.name) {
                panic!("Invalid port name '{}': {}", port.name, e);
            }
        }
        
        let mut verilog = String::new();
        
        // Module declaration
        verilog.push_str(&format!("module {} (\n", ir.name));
        
        // Ports
        for (i, port) in ir.ports.iter().enumerate() {
            let dir = match port.direction {
                Direction::Input => "input",
                Direction::Output => "output",
                Direction::Internal => continue,
            };

            debug!("Processing port: {} ({} bits)", port.name, port.width);
            
            let width = if port.width > 1 {
                format!("[{}:0] ", port.width - 1)
            } else {
                " ".to_string()
            };
            
            let comma = if i < ir.ports.len() - 1 { "," } else { "" };
            verilog.push_str(&format!("  {}{}{}{}\n", dir, width, port.name, comma));
        }
        
        verilog.push_str(");\n\n");
        
        // Statements
        for stmt in &ir.statements {
            verilog.push_str(&Self::generate_statement(stmt, 1));
        }
        
        verilog.push_str("\nendmodule\n");
        verilog
    }
    
    fn generate_statement(stmt: &Statement, indent: usize) -> String {
        let ind = "  ".repeat(indent);
        
        match stmt {
            Statement::Assign { target, value } => {
                if let Err(e) = Self::validate_identifier(&target.name) {
                    panic!("Invalid signal name '{}': {}", target.name, e);
                }
                format!("{}assign {} = {};\n", ind, target.name, Self::generate_expression(value))
            }
            
            Statement::If { condition, then_branch, else_branch } => {
            let mut s = format!("{}if ({}) begin\n", ind, Self::generate_expression(condition));
            for stmt in then_branch {
                s.push_str(&Self::generate_statement(stmt, indent + 1));
            }
            if let Some(else_stmts) = else_branch {
                s.push_str(&format!("{}end else begin\n", ind));
                for stmt in else_stmts {
                    s.push_str(&Self::generate_statement(stmt, indent + 1));
                }
            }
            s.push_str(&format!("{}end\n", ind));
            s
        }
            
            _ => format!("{}// TODO: Implement statement\n", ind),
        }
    }
    
    fn generate_expression(expr: &Expression) -> String {
        match expr {
            Expression::Signal(sig) => {
                if let Err(e) = Self::validate_identifier(&sig.name) {
                    panic!("Invalid signal name '{}': {}", sig.name, e);
                }
                sig.name.clone()
            }
            
            Expression::Literal(lit) => {
                // Convert Rust literal names to Verilog
                match lit.as_str() {
                    "Zero" => "1'b0".to_string(),
                    "One" => "1'b1".to_string(),
                    "X" => "1'bx".to_string(),
                    _ => lit.clone(),
                }
            }
            
            Expression::UnaryOp(op, inner) => {
                let op_str = match op {
                    UnaryOp::Not => "~",
                    UnaryOp::And => "&",
                    UnaryOp::Or => "|",
                    UnaryOp::Xor => "^",
                };
                format!("{}{}", op_str, Self::generate_expression(inner))
            }
            
            Expression::BinaryOp(left, op, right) => {
                let op_str = match op {
                    BinaryOp::And => "&",
                    BinaryOp::Or => "|",
                    BinaryOp::Xor => "^",
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Eq => "==",
                    BinaryOp::Neq => "!=",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gt => ">",
                };
                format!("({} {} {})", Self::generate_expression(left), op_str, Self::generate_expression(right))
            }
            
            Expression::Index(base, idx) => {
                format!("{}[{}]", Self::generate_expression(base), Self::generate_expression(idx))
            }
            
            Expression::Case { selector, arms } => {
                debug!("Generating case expression with {} arms", arms.len());
                let selector_expr = Self::generate_expression(selector);
                
                // For Verilog case expressions, we can't use a full case statement in an expression
                // Instead, we'll use a ternary-like approach with nested conditionals
                // (sel == pat0) ? val0 : (sel == pat1) ? val1 : default
                
                let mut ternary_parts = Vec::new();
                let mut default_val = None;
                
                for arm in arms {
                    match &arm.pattern {
                        Pattern::Literal(logic_val) => {
                            let pattern_str = match logic_val {
                                LogicValue::Bit(Logic::Zero) => "1'b0".to_string(),
                                LogicValue::Bit(Logic::One) => "1'b1".to_string(),
                                LogicValue::Bit(Logic::X) => "1'bx".to_string(),
                                LogicValue::Vector(bits) => {
                                    let bit_str = bits.iter()
                                        .map(|b| match b {
                                            Logic::Zero => '0',
                                            Logic::One => '1',
                                            Logic::X => 'x',
                                        })
                                        .collect::<String>();
                                    format!("{}'b{}", bits.len(), bit_str)
                                }
                            };
                            let val_expr = Self::generate_expression(&arm.value);
                            let cond = format!("(({}) == {}) ? {}", selector_expr, pattern_str, val_expr);
                            ternary_parts.push(cond);
                        }
                        Pattern::Default => {
                            default_val = Some(Self::generate_expression(&arm.value));
                        }
                        _ => {
                            warn!("Unsupported pattern in case expression");
                        }
                    }
                }
                
                // Build the nested ternary expression
                let mut result = ternary_parts.join(" : ");
                if let Some(default) = default_val {
                    result.push_str(&format!(" : {}", default));
                } else if !ternary_parts.is_empty() {
                    // If no default, use a placeholder
                    result.push_str(" : 1'bx");
                }
                format!("({})", result)
            }
            
            _ => "/* TODO */".to_string(),
        }
    }
    
    fn validate_identifier(name: &str) -> Result<(), String> {
        // Verilog reserved keywords that cannot be used as identifiers
        let keywords = [
            "input", "output", "inout", "wire", "reg", "module", "endmodule",
            "always", "assign", "begin", "end", "if", "else", "case", "endcase",
            "default", "for", "while", "posedge", "negedge", "or", "and", "not",
            "xor", "nor", "nand", "buf", "generate", "endgenerate", "genvar",
            "parameter", "localparam", "defparam", "specify", "endspecify",
            "function", "endfunction", "task", "endtask", "initial", "final",
            "integer", "real", "time", "realtime", "supply0", "supply1",
            "tri", "triand", "trior", "tri0", "tri1", "uwire", "wand", "wor",
        ];
        
        if keywords.contains(&name) {
            Err(format!("'{}' is a Verilog reserved keyword and cannot be used as a signal/port name", name))
        } else {
            Ok(())
        }
    }
}

use copper_core::Direction;
