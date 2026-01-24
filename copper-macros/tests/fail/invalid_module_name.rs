use copper_core::{Register, Direction};
use copper_macros::module;

pub struct BadName<const N: usize> {
    pub data: Register<N>,
}

// Should only accept string literals, not numbers
#[module(123)]
impl<const N: usize> BadName<N> {
    pub fn design(&mut self) {}
}

fn main() {}
