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

// ── Accepting clauses of the write-between-read-and-update rule (V8) ─────────
// Each is a measured-agreeing flip of the V8a divergent shape.

// V8b: the write comes AFTER the update, so it publishes the committing
// (forwarded) value — which is Q from the next observation on. lfsr's shape.
#[hardware(sequential)]
async fn v8b_publish_after_update(
    clk: Clock<MainClk>,
    step: In<Bits<8>, MainClk>,
    o: Out<Bits<8>, MainClk>,
) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        let s = step.read();
        r = r + s;
        o.write(r);
        clk.tick().await;
    }
}

// V8c: the write comes BEFORE the read, so it executes ahead of the barrier
// point — at the cycle's opening, on committed state, exactly where the
// observation samples it.
#[hardware(sequential)]
async fn v8c_publish_before_read(
    clk: Clock<MainClk>,
    step: In<Bits<8>, MainClk>,
    o: Out<Bits<8>, MainClk>,
) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        o.write(r);
        r = r + step.read();
        clk.tick().await;
    }
}

// 5. The V1 shape — a register updated pre-tick with NO input read anywhere, and
//    a plain `Out` written from it. DISSOLVED 2026-08-26 by the cycle-dataflow
//    forwarded emission (phase B): the opening-prefix drive now emits
//    `assign o = r + 1`, the meaning the simulator always had — measured
//    agreeing. This used to be the ui/fail case.
#[hardware(sequential)]
async fn forwarded_opening_drive(clk: Clock<MainClk>, o: Out<Bits<8>, MainClk>) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        r = r + Bits::from_lit::<1>();
        o.write(r);
        clk.tick().await;
    }
}
