use copper_core::{Bits, Clock, Logic, ClockDomain};
use copper_core::port::{In, Out};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: the body DOES contain a `clk.tick().await`, but only on the
// `sel == 1` branch. When `sel == 0` the loop returns to its head without
// ticking — a zero-time combinational cycle on that path. This is the case the
// CFG-based reachability check exists to catch (a tick-free *path*, not a
// tick-free *body*), enforced in the macro.
#[hardware(sequential)]
async fn partial(clk: Clock<MainClk>, sel: In<Logic, MainClk>, out: Out<Bits<8>, MainClk>) {
    let mut n: Bits<8> = Bits::zero();
    loop {
        if sel.read() == Logic::One {
            clk.tick().await;
        }
        out.write(n);
        n = n + Bits::from_lit::<1>();
    }
}

fn main() {}
