// use copper_core::{Module, Direction, ModuleIR, Statement, Expression, Signal, UnaryOp, BinaryOp};
mod parser;
mod verilog;
pub mod chir_lower;
pub mod shir_lower;

use copper_core::{Module};
use parser::IRBuilder;
use verilog::VerilogGenerator;
use syn::parse_str;
/*
pub fn to_verilog<M: Module>(module: &M) -> String {
    let ast = module.get_design_ast();
    let ports = module.get_ports();
    
    let mut verilog = format!("module {} (\n", ast.name);
    
    // Generate port declarations
    let input_ports: Vec<_> = ports.iter()
        .filter(|p| matches!(p.direction, Direction::Input))
        .collect();
    let output_ports: Vec<_> = ports.iter()
        .filter(|p| matches!(p.direction, Direction::Output))
        .collect();
    
    for (i, port) in input_ports.iter().enumerate() {
        let width = if port.width > 1 {
            format!("[{}:0] ", port.width - 1)
        } else {
            String::new()
        };
        verilog.push_str(&format!("  input  wire {}{}", width, port.name));
        if i < input_ports.len() - 1 || !output_ports.is_empty() {
            verilog.push_str(",\n");
        } else {
            verilog.push_str("\n");
        }
    }
    
    for (i, port) in output_ports.iter().enumerate() {
        let width = if port.width > 1 {
            format!("[{}:0] ", port.width - 1)
        } else {
            String::new()
        };
        verilog.push_str(&format!("  output wire {}{}", width, port.name));
        if i < output_ports.len() - 1 {
            verilog.push_str(",\n");
        } else {
            verilog.push_str("\n");
        }
    }
    
    verilog.push_str(");\n\n");
    
    // TODO: Parse AST for logic
    verilog.push_str("  // Logic implementation\n");
    verilog.push_str("  // TODO: Auto-generate from design() AST\n");
    
    verilog.push_str("\nendmodule\n");
    
    verilog
}
*/
// Parse AST to extract:
// 1. Input/Output ports (from Wire/Register declarations with Direction)
// 2. Logic operations (assignments, conditionals)
// 3. Sequential logic (Register updates)

pub fn to_verilog<M: Module>(module: &M) -> String {
    let ast_data = module.get_design_ast();
    let ports = module.get_ports();
    
    let design_fn = parse_str(&ast_data.ast).expect("Failed to parse AST");
    
    match IRBuilder::from_ast(&design_fn, ports) {
        Ok(mut ir) => {
            ir.name = ast_data.name;
            VerilogGenerator::generate(&ir)
        }
        Err(e) => format!("// Error: {}\n", e),
    }
}
