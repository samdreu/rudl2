use copper_codegen::ir_builder::IRBuilder;
use copper_codegen::verilog_gen::VerilogGenerator;
use syn::parse_str;

fn main() {
    // Test 1: Simple combinational inverter
    println!("=== Test 1: Simple Inverter ===");
    let inverter_code = r#"
fn inv(a: u8) -> u8 {
    !a
}
"#;

    match parse_str::<syn::ItemFn>(inverter_code) {
        Ok(func) => {
            match IRBuilder::from_function(&func) {
                Ok(ir) => {
                    let verilog = VerilogGenerator::generate(&ir);
                    println!("{}", verilog);
                }
                Err(e) => println!("IR Error: {}", e),
            }
        }
        Err(e) => println!("Parse error: {}", e),
    }

    // Test 2: Simple combinational AND
    println!("\n=== Test 2: AND Gate ===");
    let and_code = r#"
fn and_gate(a: u8, b: u8) -> u8 {
    a & b
}
"#;

    match parse_str::<syn::ItemFn>(and_code) {
        Ok(func) => {
            match IRBuilder::from_function(&func) {
                Ok(ir) => {
                    let verilog = VerilogGenerator::generate(&ir);
                    println!("{}", verilog);
                }
                Err(e) => println!("IR Error: {}", e),
            }
        }
        Err(e) => println!("Parse error: {}", e),
    }

    // Test 3: Simple addition
    println!("\n=== Test 3: Adder ===");
    let adder_code = r#"
fn add(a: u8, b: u8) -> u8 {
    a + b
}
"#;

    match parse_str::<syn::ItemFn>(adder_code) {
        Ok(func) => {
            match IRBuilder::from_function(&func) {
                Ok(ir) => {
                    let verilog = VerilogGenerator::generate(&ir);
                    println!("{}", verilog);
                }
                Err(e) => println!("IR Error: {}", e),
            }
        }
        Err(e) => println!("Parse error: {}", e),
    }
}
