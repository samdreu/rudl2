// Cause L: a `RegOut` write whose value is a register ALREADY ASSIGNED earlier in
// the same segment must see the assigned value, not the register's pre-edge one.
//
// The two modules below are the same three statements in the two possible orders.
// They are different programs and must not lower to the same SystemVerilog — which
// is exactly what they did until 2026-08-24, both emitting `acc <= n; n <= n + step;`.
// `assign_then_write` was the one silently wrong, by a full cycle, every cycle.
//
// The asymmetry is NOT about the port type, it is about where the drive is finally
// evaluated: `acc <= <expr>` inside `always_ff` reads PRE-edge register values and so
// must be given the assigned value explicitly, while a continuous `assign` is read
// after the edge when the flop already holds it and must NOT be. `lfsr` pins that
// second direction. `conditional_plain_out` below is the case that makes the
// distinction impossible to draw from the port type alone.

/// The shape that was wrong: assign, then publish what was assigned.
#[hardware(sequential)]
async fn assign_then_write(
    clk: Clock<MainClk>,
    step: In<Bits<8>, MainClk>,
    acc: RegOut<Bits<8>, MainClk>,
) {
    let mut n: Bits<8> = Bits::zero();

    loop {
        n = n + step.read();
        acc.write(n);
        clk.tick().await;
    }
}

/// The write-before-tick Moore ordering, which was always correct and must stay so:
/// publish the value the register currently holds, then advance it.
#[hardware(sequential)]
async fn write_then_assign(
    clk: Clock<MainClk>,
    step: In<Bits<8>, MainClk>,
    acc: RegOut<Bits<8>, MainClk>,
) {
    let mut n: Bits<8> = Bits::zero();

    loop {
        acc.write(n);
        n = n + step.read();
        clk.tick().await;
    }
}

/// **L-1.** A plain `Out` written CONDITIONALLY. `vlir_lower` turns it into an
/// implicit-hold register in `always_ff`, so it is sampled at the edge exactly like a
/// `RegOut` — even though its port type says otherwise. This is why the choice cannot
/// be made from the port declaration, and is made instead at `split_output_reg`,
/// where the registration decision actually exists.
#[hardware(sequential)]
async fn conditional_plain_out(
    clk: Clock<MainClk>,
    step: In<Bits<8>, MainClk>,
    enable: In<Logic, MainClk>,
    o: Out<Bits<8>, MainClk>,
) {
    let mut n: Bits<8> = Bits::zero();

    loop {
        n = n + step.read();
        if enable.read() == Logic::One {
            o.write(n);
        }
        clk.tick().await;
    }
}

/// **L-2.** A `let` wire read after the register assignment and then published. The
/// wire lowers to a continuous `assign` over the flop, so a drive that samples it at
/// the edge sees the pre-edge value and the `+ step` never arrives. Forwarding has to
/// see THROUGH the wire, which also leaves the wire itself dead — `-Wall` rejects an
/// assigned-but-unread signal, so eliminating it is part of the fix, not tidying.
#[hardware(sequential)]
async fn wire_into_regout(
    clk: Clock<MainClk>,
    step: In<Bits<8>, MainClk>,
    acc: RegOut<Bits<8>, MainClk>,
) {
    let mut n: Bits<8> = Bits::zero();

    loop {
        n = n + step.read();
        let bumped = n + Bits::from_lit::<1>();
        acc.write(bumped);
        clk.tick().await;
    }
}
