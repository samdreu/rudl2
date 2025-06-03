mod counter;
use crate::counter::Counter;
use rust_type::{to_verilog, New};


fn main() {
    let mut counter = Counter::<4>::new();
    counter.design();
    let verilog = to_verilog(&counter);
    println!("Counter Verilog:\n{}", verilog);
}
