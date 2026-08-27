// A received memory is clocked on its OWN domain, so a memory in another domain is
// a real clock-domain crossing — and one a synchronizer cannot fix. Without this
// rule a two-clock design would compile as a single-domain module.
use copper_core::{Bits, Clock, ClockDomain, Memory};
use copper_core::port::{In, RegOut};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}
struct OtherClk;
impl ClockDomain for OtherClk {}

#[hardware(sequential)]
async fn rom_reader(
    clk: Clock<MainClk>,
    rom: Memory<Bits<16>, 1, 0, OtherClk, 1, 1>,
    addr: In<Bits<8>, MainClk>,
    data: RegOut<Bits<16>, MainClk>,
) {
    loop {
        rom.read_port::<0>().read(addr.read().as_usize());
        clk.tick().await;
        if rom.read_port::<0>().is_ready() {
            data.write(rom.read_port::<0>().data());
        }
    }
}

fn main() {}
