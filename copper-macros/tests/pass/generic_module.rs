use copper_core::{Register, Wire, Direction, Module};
use copper_macros::module;

pub struct GenericModule<const WIDTH: usize> {
    pub input: Wire<WIDTH>,
    pub output: Register<WIDTH>,
}

#[module("generic")]
impl<const WIDTH: usize> GenericModule<WIDTH> {
    pub fn new() -> Self {
        Self {
            input: Wire::new("in", Direction::Input),
            output: Register::new("out", Direction::Output),
        }
    }

    pub fn design(&mut self) {
        // Implementation
    }
}

fn main() {
    let module = GenericModule::<8>::new();
    let ast = module.get_design_ast();
    assert!(!ast.ast.is_empty());
}
