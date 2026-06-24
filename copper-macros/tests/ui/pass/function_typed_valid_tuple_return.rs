use copper_core::Logic;
use copper_core::port::{In, Out};
use copper_macros::hardware;

// Should pass: valid combinational module
#[hardware(combinational)]
fn and_gate(a: In<Logic>, b: In<Logic>, out: Out<Logic>) {
    out.write(a.read() & b.read());
}

fn main() {}
