// `continue` in the module's top-level loop — a back edge that costs NO cycle.
//
// The counted `for` is what makes the shape well-formed: it guarantees the path
// to the `continue` ticks, so the back edge is not a zero-time cycle. (Remove it
// and `copper_analysis::check_reachability` rejects the module.)
//
// The cycle accounting is the whole point. `halt` is sampled in the same segment
// that would otherwise write `o` and take the trailing tick; taking the back edge
// instead must make that cycle the FIRST tick of the next `for`, not an extra
// cycle spent transitioning. A `continue` lowered as `pc = 0` would cost one more
// cycle per halt, and `n` changes every cycle so the two cannot be reconciled.
#[hardware(sequential)]
async fn skip_on_halt(
    clk: Clock<MainClk>,
    halt: In<Logic, MainClk>,
    n: In<Bits<8>, MainClk>,
    o: RegOut<Bits<8>, MainClk>,
) {
    loop {
        for _ in 0..2 {
            clk.tick().await;
        }
        if halt.read() == Logic::One {
            continue;
        }
        o.write(n.read());
        clk.tick().await;
    }
}
