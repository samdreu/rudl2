use rust_type::Register;
use rust_type::Wire;
use rust_type::Logic;
use rust_logic::module;


pub struct Counter<const N: usize> {
    pub count: Register<N>,
    pub enable: Wire<1>,
}

#[module("counter")]
impl<const N: usize> Counter<N> {
    pub fn new() -> Self {
        Self {
            count: Register::new("count".to_string()),
            enable: Wire::new("enable".to_string()),
        }
    }

    pub fn design(&mut self) {
        if (self.enable.get_value()[0] == Logic::One) {
            self.count.set_value(*self.count.get_value()); // TODO: FIX THIsSSSS
        }
    }
}
