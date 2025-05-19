use crate::register::Register;
// use crate::wire::Wire;
use ast_macro::{DescribeModule, analyze_impl};

#[derive(Debug, DescribeModule)]
pub struct RegDesign {
    // pub w1: Wire<4>,
    pub r1: Register<4>,
    pub r2: Register<8>,
}

#[analyze_impl]
impl RegDesign {
    pub fn new() -> Self {
        Self {
            r1: Register::new("tracker1".to_string()),
            r2: Register::new("tracker2".to_string()),
        }
    }

    pub fn update(&mut self) {
        self.r1.set_value([true, false, true, false]);
        self.r2.set_value([false; 8]);
    }
}
