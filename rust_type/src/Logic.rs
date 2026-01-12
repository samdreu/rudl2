// needed? keeping for now, but could change to something more complex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Logic {
    Zero = 0b0,
    One = 0b1,
    X, // unknown
}

impl From<bool> for Logic {
    fn from(value: bool) -> Self {
        match value {
            true => Logic::One,
            false => Logic::Zero,
        }
    }
}
impl Logic {
    pub fn to_bool(&self) -> bool {
        match self {
            Logic::Zero => false,
            Logic::One => true,
            Logic::X => panic!("Cannot convert Logic::X to bool"),
        }
    }

    pub fn is_zero(&self) -> bool {
        matches!(self, Logic::Zero)
    }

    pub fn is_one(&self) -> bool {
        matches!(self, Logic::One)
    }

    pub fn is_x(&self) -> bool {
        matches!(self, Logic::X)
    }

    pub fn new_logic_array<const N: usize>() -> [Logic; N] {
        [Logic::Zero; N]
    }
}
