use copper_core::{Bits, Logic, ClockDomain};
use copper_core::port::{In, Out};
use copper_macros::hardware;

// Should pass: `o` is driven on *every* path (both arms of the `if/else`), so no
// latch is inferred — the combinational counterpart to the rejected `leaky_mux`.
#[hardware(combinational)]
fn mux2(sel: In<Logic, ()>, a: In<Bits<8>, ()>, b: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    if sel.read() == Logic::One {
        o.write(a.read());
    } else {
        o.write(b.read());
    }
}

fn main() {}
