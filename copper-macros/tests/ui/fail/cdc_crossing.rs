use copper_core::{Clock, ClockDomain, Logic};
use copper_core::port::{In, Out};
use copper_macros::hardware;

struct Fast;
impl ClockDomain for Fast {}
struct Slow;
impl ClockDomain for Slow {}

// Should fail: a regular sequential module clocked on `Slow` declares a `Fast`-domain
// input `d` and forwards it to a `Slow` output with no synchronizer — an unsynchronized
// clock-domain crossing. Only a `#[hardware(synchronizer)]` module may cross domains.
#[hardware(sequential)]
async fn crosser(clk: Clock<Slow>, d: In<Logic, Fast>, q: Out<Logic, Slow>) {
    loop {
        q.write(d.read());
        clk.tick().await;
    }
}

fn main() {}
