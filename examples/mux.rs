use copper_core::{Wire, Logic, Direction};
use copper_macros::{module, module_struct};

#[module_struct]
pub struct Mux<const N: usize> {
    pub select: Wire<1>,
    pub input0: Wire<N>,
    pub input1: Wire<N>,
    pub output: Wire<N>,
}

#[module("mux")]
impl<const N: usize> Mux<N> {
    pub fn new() -> Self {
        Self {
            select: Wire::new("select", Direction::Input),
            input0: Wire::new("input0", Direction::Input),
            input1: Wire::new("input1", Direction::Input),
            output: Wire::new("output", Direction::Output),
        }
    }

    pub fn design(&mut self) {
        let select_bits = self.select.get_value();
        let input0_bits = self.input0.get_value();
        let input1_bits = self.input1.get_value();

        let output_bits: [Logic; N] = std::array::from_fn(|i| {
            match select_bits[0] {
                Logic::Zero => input0_bits[i],
                Logic::One => input1_bits[i],
                Logic::X => Logic::X,
            }
        });

        self.output.set_value(output_bits);
    }

    pub fn set_inputs(&mut self, select: [Logic; 1], input0: [Logic; N], input1: [Logic; N]) {
        self.select.set_value(select);
        self.input0.set_value(input0);
        self.input1.set_value(input1);
    }

    pub fn get_output(&self) -> [Logic; N] {
        *self.output.get_value()
    }
}

fn main() {
    let mut mux = Mux::<2>::new();
    mux.set_inputs([Logic::Zero], [Logic::Zero, Logic::One], [Logic::One, Logic::Zero]);
    mux.design();
    println!("out = {:?}", mux.get_output());

    mux.set_inputs([Logic::One], [Logic::Zero, Logic::One], [Logic::One, Logic::Zero]);
    mux.design();
    println!("out = {:?}", mux.get_output());

    // Verilog generation
    let verilog = copper_codegen::to_verilog(&mux);
    println!("=== Generated Verilog ===\n{}", verilog);

    // Simulation
    let mut sim = copper_sim::Simulator::new(mux);
    sim.run_cycles(2);
    println!("=== Simulation Complete: {} cycles ===", sim.get_cycles());
}

#[test]
fn mux_selects_input0() {
    let mut mux = Mux::<2>::new();
    mux.set_inputs([Logic::Zero], [Logic::Zero, Logic::One], [Logic::One, Logic::Zero]);
    mux.design();
    assert_eq!(mux.get_output(), [Logic::Zero, Logic::One]);
}

#[test]
fn mux_selects_input1() {
    let mut mux = Mux::<2>::new();
    mux.set_inputs([Logic::One], [Logic::Zero, Logic::One], [Logic::One, Logic::Zero]);
    mux.design();
    assert_eq!(mux.get_output(), [Logic::One, Logic::Zero]);
}

#[test]
fn mux_x_propagates() {
    let mut mux = Mux::<2>::new();
    mux.set_inputs([Logic::X], [Logic::Zero, Logic::One], [Logic::One, Logic::Zero]);
    mux.design();
    assert_eq!(mux.get_output(), [Logic::X, Logic::X]);
}

#[test]
fn mux_verilog_generation() {
    let mux = Mux::<2>::new();
    let verilog = copper_codegen::to_verilog(&mux);

    assert!(verilog.contains("module mux"));
    assert!(verilog.contains("input wire [1:0] input0"));
    assert!(verilog.contains("input wire [1:0] input1"));
    assert!(verilog.contains("input wire  select"));
    assert!(verilog.contains("output wire [1:0] output"));
}
