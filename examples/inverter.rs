use copper_core::{Register, Wire, Logic, Direction};
use copper_macros::{module, module_struct};

#[module_struct]
pub struct Inverter<const N: usize> {
    pub input: Wire<N>,
    pub output: Wire<N>,
}

#[module("inverter")]
impl<const N: usize> Inverter<N> {
    pub fn new() -> Self {
        Self {
            input: Wire::new("input", Direction::Input),
            output: Wire::new("output", Direction::Output),
        }
    }

    pub fn design(&mut self) {
        // Inverter logic
        let input_bits = self.input.get_value();
        let output_bits = input_bits.map(|bit| match bit {
            Logic::Zero => Logic::One,
            Logic::One => Logic::Zero,
            Logic::X => Logic::X,
        });
        self.output.set_value(output_bits);
    }

    pub fn set_input(&mut self, values: [Logic; N]) {
        self.input.set_value(values);
    }

    pub fn get_output(&self) -> [Logic; N] {
        *self.output.get_value()
    }
}

fn main() {
    let mut inv = Inverter::<1>::new();
    inv.set_input([Logic::Zero]);
    inv.design();
    println!("out = {:?}", inv.get_output());

    // Verilog generation
    let verilog = copper_codegen::to_verilog(&inv);
    println!("=== Generated Verilog ===\n{}", verilog);

    // Simulation
    let mut sim = copper_sim::Simulator::new(inv);
    sim.run_cycles(2);
    println!("=== Simulation Complete: {} cycles ===", sim.get_cycles());
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
