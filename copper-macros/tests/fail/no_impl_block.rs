use copper_macros::module;

// Macro should only work on impl blocks, not structs
#[module("bad")]
pub struct NotAnImplBlock {
    field: u32,
}

fn main() {}
