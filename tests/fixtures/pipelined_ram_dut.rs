// Memories with latency deeper than one cycle. One body shape, three latency
// configurations, so a failure isolates the latency and nothing else.

/// READ_LAT = 2, WRITE_LAT = 2, ReadFirst.
#[hardware(sequential)]
async fn ram_r2w2(
    clk: Clock<MainClk>,
    raddr: In<Bits<4>, MainClk>,
    waddr: In<Bits<4>, MainClk>,
    wdata: In<Bits<8>, MainClk>,
    we: In<Logic, MainClk>,
    data: Out<Bits<8>, MainClk>,
) {
    let mem = Memory::<Bits<8>, 1, 1, MainClk, 2, 2>::new(clk.clone(), 16);
    let mut q: Bits<8> = Bits::zero();

    loop {
        if we.read() == Logic::One {
            mem.write_port::<0>().write(waddr.read().as_usize(), wdata.read());
        }
        mem.read_port::<0>().read(raddr.read().as_usize());
        clk.tick().await;
        if mem.read_port::<0>().is_ready() {
            q = mem.read_port::<0>().data();
        }
        data.write(q);
    }
}

/// READ_LAT = 3, WRITE_LAT = 1, ReadFirst — a chain deeper than two, so the
/// lowering cannot be right by accident on a hard-coded second stage.
#[hardware(sequential)]
async fn ram_r3w1(
    clk: Clock<MainClk>,
    raddr: In<Bits<4>, MainClk>,
    waddr: In<Bits<4>, MainClk>,
    wdata: In<Bits<8>, MainClk>,
    we: In<Logic, MainClk>,
    data: Out<Bits<8>, MainClk>,
) {
    let mem = Memory::<Bits<8>, 1, 1, MainClk, 3, 1>::new(clk.clone(), 16);
    let mut q: Bits<8> = Bits::zero();

    loop {
        if we.read() == Logic::One {
            mem.write_port::<0>().write(waddr.read().as_usize(), wdata.read());
        }
        mem.read_port::<0>().read(raddr.read().as_usize());
        clk.tick().await;
        if mem.read_port::<0>().is_ready() {
            q = mem.read_port::<0>().data();
        }
        data.write(q);
    }
}

/// READ_LAT = 1, WRITE_LAT = 2, WriteFirst. The forwarding mux must take the
/// write that COMMITS at this edge — the last pipeline stage — not the one just
/// staged, which will not reach the array for another cycle.
#[hardware(sequential)]
async fn ram_r1w2_wf(
    clk: Clock<MainClk>,
    raddr: In<Bits<4>, MainClk>,
    waddr: In<Bits<4>, MainClk>,
    wdata: In<Bits<8>, MainClk>,
    we: In<Logic, MainClk>,
    data: Out<Bits<8>, MainClk>,
) {
    let mem = Memory::<Bits<8>, 1, 1, MainClk, 1, 2>::new(clk.clone(), 16).write_first();
    let mut q: Bits<8> = Bits::zero();

    loop {
        if we.read() == Logic::One {
            mem.write_port::<0>().write(waddr.read().as_usize(), wdata.read());
        }
        mem.read_port::<0>().read(raddr.read().as_usize());
        clk.tick().await;
        if mem.read_port::<0>().is_ready() {
            q = mem.read_port::<0>().data();
        }
        data.write(q);
    }
}
