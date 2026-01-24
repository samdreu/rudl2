use copper_core::{ModuleIR};
use copper_core::ir::{UnaryOp, BinaryOp, Statement, Expression, Signal};


pub struct VerilogGenerator;

impl VerilogGenerator {
    pub fn generate(ir: &ModuleIR) -> String {
        let mut verilog = String::new();
        
        // Module declaration
        verilog.push_str(&format!("module {} (\n", ir.name));
        
        // Ports
        for (i, port) in ir.ports.iter().enumerate() {
            let dir = match port.direction {
                Direction::Input => "input ",
                Direction::Output => "output",
                Direction::Internal => continue,
            };
            
            let width = if port.width > 1 {
                format!("[{}:0] ", port.width - 1)
            } else {
                String::new()
            };
            
            let comma = if i < ir.ports.len() - 1 { "," } else { "" };
            verilog.push_str(&format!("  {} wire {}{}{}\n", dir, width, port.name, comma));
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
            Expression::Signal(sig) => sig.name.clone(),
            
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
            
            _ => "/* TODO */".to_string(),
        }
    }
}

use copper_core::Direction;
