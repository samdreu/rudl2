// Read-during-write mode DUTs.
//
// All three share one body shape and differ only in the mode and the write-port
// count, so a cycle where they disagree isolates exactly that difference.

/// WriteFirst: a read sees a same-cycle write to the same address.
#[hardware(sequential)]
async fn ram_write_first(
    clk: Clock<MainClk>,
    waddr: In<Bits<4>, MainClk>,
    wdata: In<Bits<8>, MainClk>,
    we: In<Logic, MainClk>,
    raddr: In<Bits<4>, MainClk>,
    data: Out<Bits<8>, MainClk>,
) {
    let mem = Memory::<Bits<8>, 1, 1, MainClk, 1, 1>::new(clk.clone(), 16).write_first();
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

/// ReadFirst: byte-identical to the above except for the builder call. Exists so
/// the two can be run on the same stimulus — a WriteFirst that silently behaved
/// like ReadFirst would otherwise pass every check WriteFirst has.
#[hardware(sequential)]
async fn ram_read_first(
    clk: Clock<MainClk>,
    waddr: In<Bits<4>, MainClk>,
    wdata: In<Bits<8>, MainClk>,
    we: In<Logic, MainClk>,
    raddr: In<Bits<4>, MainClk>,
    data: Out<Bits<8>, MainClk>,
) {
    let mem = Memory::<Bits<8>, 1, 1, MainClk, 1, 1>::new(clk.clone(), 16).read_first();
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

/// Two write ports, both aimed at one address, forwarded to a WriteFirst read.
/// This is what pins the ORDER of the forwarding mux: the simulator commits
/// writes in ascending port order, so port 1 overwrites port 0 and the read must
/// see port 1's data.
#[hardware(sequential)]
async fn ram_wf_priority(
    clk: Clock<MainClk>,
    waddr: In<Bits<4>, MainClk>,
    d0: In<Bits<8>, MainClk>,
    d1: In<Bits<8>, MainClk>,
    e0: In<Logic, MainClk>,
    e1: In<Logic, MainClk>,
    raddr: In<Bits<4>, MainClk>,
    data: Out<Bits<8>, MainClk>,
) {
    let mem = Memory::<Bits<8>, 1, 2, MainClk, 1, 1>::new(clk.clone(), 16).write_first();
    let mut q: Bits<8> = Bits::zero();

    loop {
        if e0.read() == Logic::One {
            mem.write_port::<0>().write(waddr.read().as_usize(), d0.read());
        }
        if e1.read() == Logic::One {
            mem.write_port::<1>().write(waddr.read().as_usize(), d1.read());
        }
        mem.read_port::<0>().read(raddr.read().as_usize());
        clk.tick().await;
        if mem.read_port::<0>().is_ready() {
            q = mem.read_port::<0>().data();
        }
        data.write(q);
    }
}
