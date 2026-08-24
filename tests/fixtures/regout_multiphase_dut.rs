// Single source of truth for the multi-phase RegOut codegen bug.
// `include!`d for simulation and `include_str!`d for transpilation.
//
// A two-phase shift-in: capture a nibble, tick, capture a second nibble, pack
// both, write the `RegOut`, tick. Minimal reduction of
// `examples/basejump/sipo_block`.
//
// Deliberately uses a plain `Bits` local, NOT a `[Logic; N]` array: the
// divergence this pins is independent of array locals (measured — the array
// form fails on exactly the same cycles with exactly the same values).
#[hardware(sequential)]
pub async fn regout_multiphase(
    clk: Clock<MainClk>,
    data_i: In<Bits<4>, MainClk>,
    word_o: RegOut<Bits<8>, MainClk>,
) {
    loop {
        let lo = data_i.read();
        clk.tick().await;
        let hi = data_i.read();
        let mut bits: Bits<8> = Bits::zero();
        for k in 0..4 {
            bits[k] = lo[k];
            bits[4 + k] = hi[k];
        }
        word_o.write(bits);
        clk.tick().await;
    }
}
