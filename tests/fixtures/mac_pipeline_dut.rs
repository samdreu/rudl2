// Single source of truth for the mac_pipeline equivalence test.
// `include!`d for simulation and `include_str!`d for transpilation.
// A 3-stage pipeline computing (a*b)+c with 3-cycle latency, written as
// straight-line code with three `clk.tick().await` stage boundaries — the
// compiler infers the inter-stage pipeline registers. Exercises multi-phase
// async/await -> a phase FSM with phase-local combinational temps (the latch
// P0 fix defaults them at the top of always_comb).
#[hardware(sequential)]
async fn mac_pipeline(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    b: In<Bits<8>, MainClk>,
    c: In<Bits<8>, MainClk>,
    out: Out<Bits<8>, MainClk>,
) {
    loop {
        // Stage 1 — sample inputs and multiply (product, c_s live across .await)
        let product = a.read() * b.read();
        let c_s = c.read();
        clk.tick().await;

        // Stage 2 — add c captured in stage 1 (sum lives across .await)
        let sum = product + c_s;
        clk.tick().await;

        // Stage 3 — output
        out.write(sum);
        clk.tick().await;
    }
}
