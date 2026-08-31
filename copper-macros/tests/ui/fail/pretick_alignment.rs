#![allow(unused_imports)]
use copper_core::port::{In, Out};
use copper_core::{Bits, Clock, ClockDomain};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

// Should fail: MIXED alignment — the input read reaches the register assignment
// on one arm only, so the region boundary is path-dependent: the shared
// `o.write(r)` executes at the cycle's opening on the read-free arm and at the
// pre-edge on the other, and no single emission matches both. Measured (the W4
// witness, re-measured under forwarded emission 2026-08-26): the simulator holds
// `i + 1` while the SV alternates.
//
// The V1 shape that used to live here (`r = r + 1; o.write(r);` with no read
// anywhere) was DISSOLVED by the cycle-dataflow forwarded emission and now lives
// in ui/pass/pretick_alignment_ok.rs.
#[hardware(sequential)]
async fn mixed_alignment(clk: Clock<MainClk>, i: In<Bits<8>, MainClk>, o: Out<Bits<8>, MainClk>) {
    let mut phase: u8 = 0;
    let mut r: Bits<8> = Bits::zero();
    loop {
        if phase == 0 {
            r = i.read();
            phase = 1;
        } else {
            r = r + Bits::from_lit::<1>();
            phase = 0;
        }
        o.write(r);
        clk.tick().await;
    }
}

fn main() {}
