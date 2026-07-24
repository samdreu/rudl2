// Single source of truth for the shift_register equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
// A const-generic (`N`, `N_1 = N-1`) bidirectional shift register — exercises
// symbolic width, for-loops, LHS bit-assign, dynamic index, and the auto-hoist
// of a block-local combinational temp. Verilated at N=8, N_1=7 (`.with_params`).
#[hardware(sequential)]
async fn shift_register<const N: usize, const N_1: usize>(
    d: In<Logic, MainClk>,
    clk: Clock<MainClk>,
    en: In<Logic, MainClk>,
    dir: In<Logic, MainClk>,
    rstn: In<Logic, MainClk>,
    out: Out<Bits<N>, MainClk>,
) {
    const { assert!(N - 1 == N_1, "N_1 must equal N-1") };
    let mut out_n = Bits::x();

    loop {
        if !rstn.read().as_bool() {
            out_n = Bits::zero();
        } else if en.read().as_bool() {
            let mut shifted = Bits::zero();
            if dir.read() == Logic::Zero {
                // shift left, d enters LSB
                shifted[0] = d.read();
                for i in 1..N {
                    shifted[i] = out_n[i - 1];
                }
            } else {
                // shift right, d enters MSB
                shifted[N_1] = d.read();
                for i in 0..N_1 {
                    shifted[i] = out_n[i + 1];
                }
            }
            out_n = shifted;
        }

        out.write(out_n);
        clk.tick().await;
    }
}
