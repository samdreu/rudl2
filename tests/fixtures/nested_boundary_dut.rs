// Cause K: a loop whose body ENDS in another tick-bearing loop, so the segment's
// clock boundary comes from the inner loop rather than from a tick of its own.
//
// ## Why the inner loop never breaks
//
// Not an oversight — it is forced. A nested loop's tick must be its LAST statement
// (the ordering rule; see `wait_loop_equivalence.rs`), so any inner loop that *can*
// exit tests before it ticks and can therefore exit having ticked zero times. An
// outer loop whose only boundary is such a loop then returns to its top in zero
// time: a livelock, which `copper_analysis::check_reachability` rejects. The two
// rules together mean the *back edge* of a cause-K loop is not writable with `loop`
// alone; it needs a counted `for _ in 0..N { … }`, which always ticks and needs no
// exit test. Until that lands, what is checkable here is the **entry** path.
//
// ## Why every port write precedes the register update
//
// Deliberate, and NOT incidental to the shape. This fixture was written while
// `RegOut` forwarding was still broken (`TODO` cause L: `acc.write(n)` after
// `n = n + step` emitted the pre-edge `acc <= n`), and it keeps the
// write-before-mutate ordering now that cause L is fixed, so that what it checks
// stays cause K alone. A failure here means the nested-loop lowering regressed,
// not that the forwarding did — `tests/regout_forwarding_equivalence.rs` owns that.

/// `busy` is the outer body's prefix and `acc` the inner body's, and both must land
/// in the SAME cycle: entering a nested loop must not cost a clock tick. If it did,
/// cycle 0 would set `busy` alone and every `acc` value would be shifted by one.
///
/// The inner `loop` never exits, so the outer body runs exactly once — that IS the
/// shape under test (a nested tick-bearing loop with no escape), not an accident, so
/// clippy's deny-by-default `never_loop` is allowed on this item alone.
#[allow(clippy::never_loop)]
#[hardware(sequential)]
async fn nested_boundary(
    clk: Clock<MainClk>,
    step: In<Bits<8>, MainClk>,
    acc: RegOut<Bits<8>, MainClk>,
    busy: RegOut<Logic, MainClk>,
) {
    let mut n: Bits<8> = Bits::zero();

    loop {
        busy.write(Logic::One);
        loop {
            acc.write(n);
            n = n + step.read();
            clk.tick().await;
        }
    }
}
