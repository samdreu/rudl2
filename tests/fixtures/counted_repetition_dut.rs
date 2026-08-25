// Counted REPETITION, not just counted delay: the outer `for` body does work and
// uses the loop variable, and its own clock boundary comes from an inner counted
// `for` — the UART's serialiser shape.
//
//     for i in 0..8 { tx.write(byte[i]); for _ in 0..CLKS_PER_BIT { tick } }
//
// `i` is a counter REGISTER, so `d.read()[i]` is a dynamic bit select, not an
// unrolled static slice. Two cycles per bit rather than 434 keeps the trace short
// without changing the shape.
#[hardware(sequential)]
async fn counted_repetition(
    clk: Clock<MainClk>,
    d: In<Bits<8>, MainClk>,
    serial: RegOut<Logic, MainClk>,
) {
    loop {
        for i in 0..8 {
            serial.write(d.read()[i]);
            for _ in 0..2 {
                clk.tick().await;
            }
        }
    }
}
