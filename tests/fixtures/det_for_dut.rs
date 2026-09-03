// Variable-length pattern detector written as a counted `for` over the pattern.
//
// The pattern arrives on a port (`Bits<N>`) and the loop walks it one bit per
// cycle; the loop counter `i` is live across the tick and becomes a register,
// and `pattern.read()[i]` is a dynamic bit select. A mismatch `break`s out of the
// `for` in the same cycle and the outer loop starts a new attempt. A full match
// drives `out_o` (a `RegOut`, write-before-tick Moore) high for one cycle.
//
// Experiment for the paper (2026-09-02): does a `for` over a variable-length
// pattern simulate, transpile, and pass the sweep? It does now; it found two bugs
// on the way, both FIXED the same day:
//   1. the first version had no tick on the mismatch path (`break` then straight
//      back to the loop top) and HUNG the simulator: `check_reachability` assumed
//      a `for` always runs to its tick. Now rejected at compile time —
//      copper-macros/tests/ui/fail/for_break_before_tick.rs pins the message;
//   2. with the trailing tick below, the transpiled FSM charged every `break` the
//      hoisted last-iteration tick (pc0→pc2→pc0 where the source takes one edge).
//      `expand_counted_for` now guards that tick on the counter having reached the
//      end when the body breaks. tests/det_for_probe.rs pins sim == hand model ==
//      RTL on a fixed stream; the sweep covers the module with random stimulus.
#[hardware(sequential)]
async fn det_for<const N: usize>(
    clk: Clock<MainClk>,
    rstn: In<Logic, MainClk>,
    pattern: In<Bits<N>, MainClk>,
    in_i: In<Logic, MainClk>,
    out_o: RegOut<Logic, MainClk>,
) {
    loop {
        out_o.write(Logic::Zero);
        if rstn.read() == Logic::Zero {
            clk.tick().await;
        } else {
            let mut ok = true;
            for i in 0..N {
                if in_i.read() != pattern.read()[i] {
                    ok = false;
                    break;
                }
                clk.tick().await;
            }
            if ok {
                out_o.write(Logic::One);
            }
            // Unconditional: a mismatch that `break`s out of the `for` before its
            // tick must still cost a cycle, or the outer loop would spin with no
            // clock boundary (the first version did exactly that and hung the sim).
            clk.tick().await;
        }
    }
}
