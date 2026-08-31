// Single source of truth for the plain-`Out` multi-phase hold divergence.
// `include!`d for simulation and `include_str!`d for transpilation.
//
// A plain (combinational) `Out` written UNCONDITIONALLY, but in only ONE phase of
// a multi-phase loop. Reduced from the `wN_dbg` debug ports of
// `examples/basejump/sipo_block.rs`.
#[hardware(sequential)]
pub async fn plain_out_multiphase(
    clk: Clock<MainClk>,
    data_i: In<Bits<4>, MainClk>,
    seen_o: Out<Bits<4>, MainClk>,
) {
    loop {
        let v = data_i.read();
        seen_o.write(v);
        clk.tick().await;
        // Second phase: seen_o is not written at all. The simulator HOLDS it.
        clk.tick().await;
    }
}
