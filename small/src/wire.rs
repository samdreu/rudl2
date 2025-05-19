enum Direction {
    Input,
    Output,
    Internal,
}

#[derive(Debug)]
pub struct Wire<const N: usize> {
    name: String,
    value: [bool; N],
    dir: Direction,
}

impl<const N: usize> Wire<N> {
    pub fn new(name: String) -> Self {
        Wire {
            name,
            value: [false; N],
            dir: Direction::Internal,
        }
    }

    pub fn set_value(&mut self, value: [bool; N]) {
        self.value = value;
    }

    pub fn get_value(&self) -> &[bool; N] {
        &self.value
    }

    pub fn get_name(&self) -> &str {
        &self.name
    }
}
