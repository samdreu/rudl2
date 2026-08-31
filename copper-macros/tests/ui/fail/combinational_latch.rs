use copper_core::{Bits, Logic, ClockDomain};
use copper_core::port::{In, Out};
use copper_macros::hardware;

// Should fail: `o` is driven on the `sel == 1` path but not the implicit `else`
// path. A some-but-not-all drive of a *combinational* output infers a latch —
// the shared definite-assignment check, enforced in the macro, rejects it.
#[hardware(combinational)]
fn leaky_mux(sel: In<Logic, ()>, a: In<Bits<8>, ()>, o: Out<Bits<8>, ()>) {
    if sel.read() == Logic::One {
        o.write(a.read());
    }
}

fn main() {}
