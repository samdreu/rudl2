// `for _ in 0..N { clk.tick().await; }` — a counted DELAY, N clock cycles.
//
// The simulator runs Rust's own `for`, so this is 3 ticks per iteration of the
// module loop and `o` is rewritten every 3rd cycle. `n` carries a distinct byte
// every cycle, so a period that is off by one shows up as a different value, not
// the same value at a different time.
#[hardware(sequential)]
async fn counted_delay(
    clk: Clock<MainClk>,
    n: In<Bits<8>, MainClk>,
    o: RegOut<Bits<8>, MainClk>,
) {
    loop {
        o.write(n.read());
        for _ in 0..3 {
            clk.tick().await;
        }
    }
}
