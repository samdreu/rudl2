// mod Logic;
use crate::logic::Logic;
use crate::Direction;



#[derive(Debug)]
pub struct Wire<const N: usize> {
    name: String,
    value: [Logic; N],
    dir: Direction,
}

impl<const N: usize> Wire<N> {
    pub fn new(name: String, dir: Direction) -> Self {
        Wire { 
            name,
            value: Logic::new_logic_array(),
            dir,
        }
    }

    pub fn set_value(&mut self, value: [Logic; N]) {
        self.value = value;
    }

    pub fn get_value(&self) -> &[Logic; N] {
        &self.value
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }
}
