use copper_core::{Register, Direction, Module};
use copper_macros::module;

pub struct MultiMethod<const N: usize> {
    pub data: Register<N>,
}

#[module("multi")]
impl<const N: usize> MultiMethod<N> {
    pub fn new() -> Self {
        Self {
            data: Register::new("data", Direction::Output),
        }
    }

    pub fn design(&mut self) {
        // Design implementation
    }

    pub fn reset(&mut self) {
        // Other methods should work fine
    }

    pub fn compute(&self) -> usize {
        N
    }
}

fn main() {
    let mut m = MultiMethod::<16>::new();
    m.design();
    m.reset();
    assert_eq!(m.compute(), 16);
}
