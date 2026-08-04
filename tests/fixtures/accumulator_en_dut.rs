// Single source of truth for the accumulator_en equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
// A sequential accumulator whose register is updated only when `en` is asserted —
// the enabled-register idiom (the register holds its value across a tick when the
// update branch is not taken). Exercises a conditionally-updated register and a
// deferred data read.
#[hardware(sequential)]
async fn accumulator_en(
    clk: Clock<MainClk>,
    en: In<Logic, MainClk>,
    data: In<Bits<8>, MainClk>,
    out: Out<Bits<8>, MainClk>,
) {
    let mut acc: Bits<8> = Bits::zero();
    loop {
        out.write(acc);
        clk.tick().await;
        if en.read() == Logic::One {
            acc = acc + data.read();
        }
    }
}
