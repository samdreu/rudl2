use copper_core::port::{In, Out, RegOut};
use copper_core::{Bits, Clock, ClockDomain, Logic};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// All four of these are ACCEPTED, and each isolates one clause of the rule.

// 1. The register update happens AFTER the tick, so the pre-tick segment only reads
//    state. This is the form measured to match independent hand-written Verilog.
#[hardware(sequential)]
async fn post_tick_update(clk: Clock<MainClk>, o: Out<Bits<8>, MainClk>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        o.write(r);
        clk.tick().await;
        r = r + Bits::from_lit::<1>();
    }
}

// 2. `RegOut` is immune — it commits at the clock edge, so the phase at which the
//    write executes cannot be observed.
#[hardware(sequential)]
async fn registered_output(clk: Clock<MainClk>, o: RegOut<Bits<8>, MainClk>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        r = r + Bits::from_lit::<1>();
        o.write(r);
        clk.tick().await;
    }
}

// 3. A leading `In` read PRECEDES the assignment, installing the barrier that pins
//    the segment to the pre-edge phase.
#[hardware(sequential)]
async fn leading_read(clk: Clock<MainClk>, i: In<Bits<8>, MainClk>, o: Out<Bits<8>, MainClk>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        r = r + i.read();
        o.write(r);
        clk.tick().await;
    }
}

// 4. The write is a CONSTANT, so it is idempotent across the phase shift — the
//    misalignment changes only *when* the write happens, which is unobservable.
#[hardware(sequential)]
async fn constant_write(clk: Clock<MainClk>, o: Out<Logic, MainClk>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        r = r + Bits::from_lit::<1>();
        o.write(Logic::One);
        clk.tick().await;
    }
}

fn main() {}
