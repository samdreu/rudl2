use copper_core::{Register, Wire, Logic, Direction};
use copper_macros::{module, module_struct};

#[module_struct]
pub struct Inverter {
    pub input: Wire<1>,
    pub output: Wire<1>,
}

#[module("inverter")]
impl Inverter {
    pub fn new() -> Self {
        Self {
            input: Wire::new("input", Direction::Input),
            output: Wire::new("output", Direction::Output),
        }
    }

    pub fn design(&mut self) {
        // Inverter logic
        self.output.set_value([match self.input.get_value()[0] {
            Logic::Zero => Logic::One,
            Logic::One => Logic::Zero,
            Logic::X => Logic::X,
        }]);
    }

    pub fn set_input(&mut self, value: Logic) {
        self.input.set_value([value]);
    }

    pub fn get_output(&self) -> Logic {
        self.output.get_value()[0]
    }
}

fn main() {
    let mut inv = Inverter::new();
    inv.set_input(Logic::Zero);
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
    let mut inv = Inverter::new();

    inv.set_input(Logic::Zero);
    inv.design();
    assert_eq!(inv.get_output(), Logic::One);

    inv.set_input(Logic::One);
    inv.design();
    assert_eq!(inv.get_output(), Logic::Zero);

    inv.set_input(Logic::X);
    inv.design();
    assert_eq!(inv.get_output(), Logic::X);
}

// right now this just checks that verilog generation runs without error
// in future, actually verify the output
#[test]
fn inverter_verilog_generation() {
    let inv = Inverter::new();
    let verilog = copper_codegen::to_verilog(&inv);

    assert!(verilog.contains("module inverter"));
}
