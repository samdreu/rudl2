// Single source of truth for the M1 equivalence test.
// Included as code (`include!`) to run in the Copper simulator, and read as
// text (`include_str!`) to feed the transpiler — so the simulated DUT and the
// transpiled DUT are byte-identical by construction.
#[hardware(sequential)]
async fn counter(clk: Clock<MainClk>, step: In<Bits<8>, MainClk>, out: Out<Bits<8>, MainClk>) {
    let mut count: Bits<8> = Bits::zero();
    loop {
        out.write(count);
        clk.tick().await;
        count = count + step.read();
    }
}
