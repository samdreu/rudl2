use copper_code::{Register, Wire, Direction, Module};
use copper_macros::module;

pub struct BasicCounter<const N: usize> {
    pub count: Register<N>,
}

#[module("basic_counter")]
impl<const N: usize> BasicCounter<N> {
    pub fn new() -> Self {
        Self {
            count: Register::new("count", Direction::Output),
        }
    }

    pub fn design(&mut self) {
        // Simple counter logic
        // On each clock cycle, increment the count register
    }
}

fn main() {
    let mut counter = BasicCounter::<4>::new();
    counter.design();
    let ast = counter.get_design_ast();
    assert_eq!(ast.name, "basic_counter");
}
