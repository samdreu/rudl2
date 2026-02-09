use copper_core::{Register, Wire, Logic, Direction};
use copper_macros::{module, module_struct};
use log::{debug, info, warn, error};
use std::fs;


#[module_struct]
pub struct Inverter<const N: usize> {
    pub in_data: Wire<N>,
    pub out_data: Wire<N>,
}

#[module("inverter")]
impl<const N: usize> Inverter<N> {
    pub fn new() -> Self {
        Self {
            in_data: Wire::new("in_data", Direction::Input),
            out_data: Wire::new("out_data", Direction::Output),
        }
    }

    pub fn design(&mut self) {
        // Inverter logic
        let input_bits = self.in_data.get_value();
        let output_bits = input_bits.map(|bit| match bit {
            Logic::Zero => Logic::One,
            Logic::One => Logic::Zero,
            Logic::X => Logic::X,
        });
        self.out_data.set_value(output_bits);
    }

    pub fn set_input(&mut self, values: [Logic; N]) {
        self.in_data.set_value(values);
    }

    pub fn get_output(&self) -> [Logic; N] {
        *self.out_data.get_value()
    }
}

fn main() {
    // env_logger::init();
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Trace)
        .init();
    
    let mut inv = Inverter::<1>::new();
    
    // Create a simulation trace to capture behavior
    let mut trace = copper_sim::SimulationTrace::new();
    
    // Test case 1: Input = 0, Output = 1
    inv.set_input([Logic::Zero]);
    inv.design();
    trace.add_cycle(
        0,
        vec![("in_data".to_string(), vec![Logic::Zero])],
        vec![("out_data".to_string(), vec![Logic::One])],
    );
    println!("Cycle 0: Input: {:?}, Output: {:?}", Logic::Zero, inv.get_output());
    
    // Test case 2: Input = 1, Output = 0
    inv.set_input([Logic::One]);
    inv.design();
    trace.add_cycle(
        1,
        vec![("in_data".to_string(), vec![Logic::One])],
        vec![("out_data".to_string(), vec![Logic::Zero])],
    );
    println!("Cycle 1: Input: {:?}, Output: {:?}", Logic::One, inv.get_output());
    
    // Test case 3: Input = 0 again, Output = 1
    inv.set_input([Logic::Zero]);
    inv.design();
    trace.add_cycle(
        2,
        vec![("in_data".to_string(), vec![Logic::Zero])],
        vec![("out_data".to_string(), vec![Logic::One])],
    );
    println!("Cycle 2: Input: {:?}, Output: {:?}", Logic::Zero, inv.get_output());

    // Verilog generation
    let verilog = copper_codegen::to_verilog(&inv);
    fs::write("inverter.v", &verilog).expect("Failed to write Verilog file");
    println!("\nGenerated Verilog written to inverter.v");

    // Verify with Verilator
    println!("\n=== Verifying with Verilator ===");
    match copper_sim::verify_with_verilator("inverter.v", "inverter", &trace) {
        Ok(true) => println!("✓ Verilator verification passed!"),
        Ok(false) => println!("✗ Verilator verification failed"),
        Err(e) => println!("⚠ Verilator verification error: {}", e),
    }
}


#[test]
fn inverter_flips_bits() {
    let mut inv = Inverter::<1>::new();

    inv.set_input([Logic::Zero]);
    inv.design();
    assert_eq!(inv.get_output(), [Logic::One]);

    inv.set_input([Logic::One]);
    inv.design();
    assert_eq!(inv.get_output(), [Logic::Zero]);

    inv.set_input([Logic::X]);
    inv.design();
    assert_eq!(inv.get_output(), [Logic::X]);
}

#[test]
fn inverter_multicycle_same_input() {
    let mut inv = Inverter::<1>::new();
    inv.set_input([Logic::Zero]);
    inv.design();
    
    println!("\n=== Inverter Multicycle Test (Same Input) ===");
    
    let mut sim = copper_sim::Simulator::new(inv);
    
    // Run through 5 cycles with the same input
    for cycle in 0..5 {
        let module = sim.get_module_mut();
        module.design();
        let output = module.get_output();
        println!("Cycle {}: Input: {:?}, Output: {:?}", cycle, Logic::Zero, output);
        assert_eq!(output, [Logic::One]);
        
        sim.clock();
    }
    
    println!("=== Test Complete ===\n");
}

// right now this just checks that verilog generation runs without error
// in future, actually verify the output
#[test]
fn inverter_verilog_generation() {
    let inv = Inverter::<1>::new();
    let verilog = copper_codegen::to_verilog(&inv);

    assert!(verilog.contains("module inverter"));
}


#[test]
fn inverter_verilog_is_correct() {
    let inv = Inverter::<1>::new();
    let verilog = copper_codegen::to_verilog(&inv);

    assert!(verilog.contains("module inverter"));
    assert!(verilog.contains("input wire input"));
    assert!(verilog.contains("output wire output"));
}
