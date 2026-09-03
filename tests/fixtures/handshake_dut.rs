// A request/acknowledge handshake counter, in the `while` spelling: wait for
// `req`, take one cycle, wait for `ack`, count the completed handshake. Each
// `while` is a state with a self-loop; the loop top, where `done` is published,
// is the third. The same machine in the `loop { if … { break } tick }` spelling is
// `handshake` in wait_loop_dut.rs; tests/handshake_equivalence.rs checks the two
// agree cycle for cycle. Single source for the tests and the corpus sweep; the
// runnable twin is examples/sequential/handshake.rs.

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
        while req.read() == Logic::Zero {
            clk.tick().await;
        }
        clk.tick().await;
        while ack.read() == Logic::Zero {
            clk.tick().await;
        }
        n = n + Bits::from_lit::<1>();
        clk.tick().await;
    }
}
