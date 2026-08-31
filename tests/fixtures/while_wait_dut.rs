// The `while` spelling of a repeating wait. Semantically identical to `waiter`
// in `wait_loop_dut.rs` — `while c { … tick }` is `loop { if !c { break; } … }`,
// test first and tick last, which is the supported ordering — but written the
// way one would reach for first.
//
// The transpiler rewrites it before control extraction
// (`control_extract::desugar_tick_waits`), so it lands on the same verified
// machinery. `copper-codegen/tests/while_loops.rs` asserts the two spellings
// emit byte-identical SystemVerilog; this fixture checks the SIMULATOR agrees
// too, since the macro handles `while` natively and never sees the rewrite.

/// Hold while `go` is low, then advance a counter. Unbounded: the module waits
/// for as many cycles as `go` stays low.
#[hardware(sequential)]
async fn while_waiter(clk: Clock<MainClk>, go: In<Logic, MainClk>, count: RegOut<Bits<8>, MainClk>) {
    let mut n: Bits<8> = Bits::zero();

    loop {
        count.write(n);
        while go.read() == Logic::Zero {
            clk.tick().await;
        }
        n = n + Bits::from_lit::<1>();
        clk.tick().await;
    }
}
