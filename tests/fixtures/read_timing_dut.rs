// Read-timing regression fixtures: multi-tick loops with CROSS-TICK input reads —
// the exact case the synced_read fix targets (EXECUTION_MODEL_RECONCILIATION.md).
// Each output is a held register aliased to `assign out = <reg>` (combinational),
// so these isolate the READ-timing axis from the still-open output-timing axis.

// 2-tick sample-and-hold: read `inp` before the tick, drive it one edge later.
#[hardware(sequential)]
async fn sample_hold_2(
    clk: Clock<MainClk>,
    inp: In<Bits<8>, MainClk>,
    out: Out<Bits<8>, MainClk>,
) {
    loop {
        let x = inp.read();
        clk.tick().await;
        out.write(x);
        clk.tick().await;
    }
}

// 2-tick accumulator: reads `step` AFTER a tick (`acc + step.read()`), so the
// read settles at post-edge and must NOT be deferred — the counterpart case to
// the loop-top reads above. Output is the accumulator register (combinational).
#[hardware(sequential)]
async fn accum_2(
    clk: Clock<MainClk>,
    step: In<Bits<8>, MainClk>,
    out: Out<Bits<8>, MainClk>,
) {
    let mut acc: Bits<8> = Bits::from_lit::<0>();
    loop {
        out.write(acc.clone());
        clk.tick().await;
        acc = acc + step.read();
        clk.tick().await;
    }
}

// 2-tick with TWO loop-top cross-tick reads combined before the tick.
#[hardware(sequential)]
async fn sum_hold_2(
    clk: Clock<MainClk>,
    in_a: In<Bits<8>, MainClk>,
    in_b: In<Bits<8>, MainClk>,
    out: Out<Bits<8>, MainClk>,
) {
    loop {
        let s = in_a.read() + in_b.read();
        clk.tick().await;
        out.write(s);
        clk.tick().await;
    }
}
