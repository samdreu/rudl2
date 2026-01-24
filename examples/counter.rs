// examples/counter.rs
use copper_core::{Register, Wire, Logic, Direction};
use copper_macros::module;

pub struct Counter<const N: usize> {
    pub count: Register<N>,
    pub enable: Wire<1>,
}

#[module("counter")]
impl<const N: usize> Counter<N> {
    pub fn new() -> Self {
        Self {
            count: Register::new("count", Direction::Output),
            enable: Wire::new("enable", Direction::Input),
        }
    }

    pub fn design(&mut self) {
        // Counter logic
        if self.enable.get_value()[0] == Logic::One {
            self.count.set_value(Logic::One);
        }
    }

    pub fn set_enable(&mut self, value: Logic) {
        self.enable.set_value([value]);
    }

    pub fn get_count(&self) -> &[Logic; N] {
        self.count.get_value()
    }
}

fn main() {
    let mut counter = Counter::<4>::new();
    counter.set_enable(Logic::One);
    counter.design();

    // Generate Verilog
    let verilog = copper_codegen::to_verilog(&counter);
    println!("=== Generated Verilog ===\n{}", verilog);
    
    // Simulate
    let mut sim = copper_sim::Simulator::new(counter);
    sim.run_cycles(10);
    println!("\n=== Simulation Complete: {} cycles ===", sim.get_cycles());
}
