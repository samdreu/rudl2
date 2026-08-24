// Repeating waits: `loop { … clk.tick().await; … }` with `break`, nested inside
// the module's own loop. The idiom for "stall until something is ready".

/// Wait for `go`, then advance a counter. The wait is unbounded: the module holds
/// for as many cycles as `go` stays low, which is the whole point — a fixed number
/// of ticks could be written without a nested loop at all.
#[hardware(sequential)]
async fn waiter(clk: Clock<MainClk>, go: In<Logic, MainClk>, count: RegOut<Bits<8>, MainClk>) {
    let mut n: Bits<8> = Bits::zero();

    loop {
        count.write(n);
        loop {
            if go.read() == Logic::One {
                break;
            }
            clk.tick().await;
        }
        n = n + Bits::from_lit::<1>();
        clk.tick().await;
    }
}

/// Two waits in sequence, on different conditions, with a tick between them —
/// a handshake. Sequential nested loops must not share states or interfere.
#[hardware(sequential)]
async fn handshake(
    clk: Clock<MainClk>,
    req: In<Logic, MainClk>,
    ack: In<Logic, MainClk>,
    done: RegOut<Bits<8>, MainClk>,
) {
    let mut n: Bits<8> = Bits::zero();

    loop {
        done.write(n);
        loop {
            if req.read() == Logic::One {
                break;
            }
            clk.tick().await;
        }
        clk.tick().await;
        loop {
            if ack.read() == Logic::One {
                break;
            }
            clk.tick().await;
        }
        n = n + Bits::from_lit::<1>();
        clk.tick().await;
    }
}

/// The REFUSED ordering, kept as the counterexample: the body ticks BEFORE
/// testing. The test then lands in the window where the simulator and a testbench
/// disagree about which input value is current, and the transpiled module reacts a
/// full cycle earlier than the simulator — measured, not argued.
#[hardware(sequential)]
async fn tick_first_waiter(
    clk: Clock<MainClk>,
    go: In<Logic, MainClk>,
    count: RegOut<Bits<8>, MainClk>,
) {
    let mut n: Bits<8> = Bits::zero();

    loop {
        count.write(n);
        loop {
            clk.tick().await;
            if go.read() == Logic::One {
                break;
            }
        }
        n = n + Bits::from_lit::<1>();
        clk.tick().await;
    }
}
