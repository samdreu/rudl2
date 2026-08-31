// Single source of truth for the lfsr equivalence test (see counter_dut.rs).
// `include!`d for simulation and `include_str!`d for transpilation.
#[hardware(sequential)]
async fn lfsr(
    clk: Clock<MainClk>,
    reset_i: In<Logic, MainClk>,
    yumi_i: In<Logic, MainClk>,
    o: Out<Bits<32>, MainClk>,
) {
    let xor_mask = Bits::from_u32((1 << 31) | (1 << 29) | (1 << 26) | (1 << 25));
    let mut state = Bits::from_u32(1);
    loop {
        if reset_i.read().as_bool() {
            state = Bits::from_u32(1);
        } else if yumi_i.read().as_bool() {
            state = if state[0] == Logic::One {
                (state >> 1) ^ xor_mask
            } else {
                state >> 1
            };
        }
        o.write(state);
        clk.tick().await;
    }
}
