// use ast_macro::print_ast;
use crate::{logic, Direction};
use crate::logic::Logic;

#[derive(Debug)]
pub struct Register <const N: usize> {
    name: String,
    value: [Logic; N],
    next: [Logic; N],
    dir: Direction,
}

impl<const N: usize> Default for Register<N> {
    fn default() -> Self {
        Register {
            name: String::new(),
            value: Logic::new_logic_array(),
            next: Logic::new_logic_array(),
            dir: Direction::Internal,
        }
    }
}


impl<const N: usize> Register<N> {
    pub fn new(name: String, dir: Direction) -> Self {
        Register {
            name,
            value: Logic::new_logic_array(),
            next: Logic::new_logic_array(),
            dir,
        }
    }

    pub fn set_value(&mut self, value: [Logic; N]) {
        self.next = value;
    }

    pub fn update(&mut self) {
        self.value = self.next;
    }

    pub fn get_value(&self) -> &[Logic; N] {
        &self.value
    }

    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_next(&self) -> &[Logic; N] {
        &self.next
    }


}

