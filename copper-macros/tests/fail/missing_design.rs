use copper_core::{Register, Direction};
use copper_macros::module;

pub struct NoDesign<const N: usize> {
    pub data: Register<N>,
}

#[module("no_design")]
impl<const N: usize> NoDesign<N> {
    pub fn new() -> Self {
        Self {
            data: Register::new("data", Direction::Output),
        }
    }

    // Missing design() method - should cause panic at macro expansion
}

fn main() {}
