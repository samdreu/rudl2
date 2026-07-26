// Single source of truth for the mac_fsm equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
// The single-tick explicit-state-machine coding of the 3-cycle MAC. It produces
// IDENTICAL simulator output to mac_pipeline, but being single-tick it is in the
// category that transpiles correctly — so it is the adjudicating reference for
// the sim-vs-phase-FSM timing debate. `out` is written only in the Out state and
// held between writes, so it is a genuine registered output — declared `RegOut`
// (an enabled flip-flop) so both the simulator and the transpiler drive it from
// `always_ff` and agree on its +1 timing.
#[derive(Clone, Copy)]
enum Stage { Load, Mul, Out }

#[hardware(sequential)]
async fn mac_fsm(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    b: In<Bits<8>, MainClk>,
    c: In<Bits<8>, MainClk>,
    out: RegOut<Bits<8>, MainClk>,
) {
    let mut stage = Stage::Load;
    let mut product: Bits<8> = Bits::from_lit::<0>();
    let mut c_latch: Bits<8> = Bits::from_lit::<0>();
    let mut result:  Bits<8> = Bits::from_lit::<0>();

    loop {
        match stage {
            Stage::Load => {
                product = a.read() * b.read();
                c_latch = c.read();
                stage   = Stage::Mul;
            }
            Stage::Mul => {
                result = product.clone() + c_latch.clone();
                stage  = Stage::Out;
            }
            Stage::Out => {
                out.write(result.clone());
                stage = Stage::Load;
            }
        }
        clk.tick().await;
    }
}
