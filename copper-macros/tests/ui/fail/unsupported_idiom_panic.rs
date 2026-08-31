use copper_core::Logic;
use copper_core::port::{In, Out};
use copper_macros::hardware;

// Should fail: `panic!` has no honest gate-level meaning — there is no "impossible"
// path in hardware, every input combination produces a value. The macro rejects
// `panic!`/`unreachable!`/`todo!`/`unimplemented!` in a module body.
#[hardware(combinational)]
fn boomer(i: In<Logic, ()>, o: Out<Logic, ()>) {
    if i.read() == Logic::One {
        panic!("cannot happen");
    }
    o.write(i.read());
}

fn main() {}
