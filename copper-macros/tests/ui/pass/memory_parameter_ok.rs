// A `Memory<…>` parameter is accepted: a module may RECEIVE its storage instead of
// declaring it, the same disposition as `Clock<D>` (received, never constructed).
// The memory here is in the module's own domain, so there is no crossing.
use copper_core::{Bits, Clock, ClockDomain, Memory};
use copper_core::port::{In, RegOut};
use copper_macros::hardware;

struct MainClk;
impl ClockDomain for MainClk {}

#[hardware(sequential)]
async fn rom_reader(
    clk: Clock<MainClk>,
    rom: Memory<Bits<16>, 1, 0, MainClk, 1, 1>,
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
