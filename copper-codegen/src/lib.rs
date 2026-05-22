// use copper_core::{Module, Direction, ModuleIR, Statement, Expression, Signal, UnaryOp, BinaryOp};
mod parser;
mod verilog;
pub mod chir_lower;
pub mod shir_lower;

use copper_core::{Module};
use parser::IRBuilder;
use verilog::VerilogGenerator;
use syn::parse_str;

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
