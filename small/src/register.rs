// use ast_macro::print_ast;

#[derive(Debug)]
pub struct Register <const N: usize> {
    name: String,
    value: [bool; N],
    next: [bool; N],
}

impl<const N: usize> Default for Register<N> {
    fn default() -> Self {
        Register {
            name: String::new(),
            value: [false; N],
            next: [false; N],
        }
    }
}


impl<const N: usize> Register<N> {
    pub fn new(name: String) -> Self {
        Register {
            name,
            value: [false; N],
            next: [false; N],
        }
    }

    pub fn set_value(&mut self, value: [bool; N]) {
        self.next = value;
    }

    fn update(&mut self) {
        self.value = self.next;
    }

    fn get_value(&self) -> &[bool; N] {
        &self.value
    }

    fn get_name(&self) -> &str {
        &self.name
    }

    fn get_next(&self) -> &[bool; N] {
        &self.next
    }


}

