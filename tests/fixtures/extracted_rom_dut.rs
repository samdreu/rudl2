// Memory in a module that needs CONTROL EXTRACTION — the shape that was
// unwritable until 2026-08-25, when the memory staging rules moved from
// `chir_lower`'s tick-delimited segments to `copper-analysis`'s source CFG.
//
// Both modules stage a read, cross the edge, and observe the result, which is the
// ordinary single-cycle ROM idiom. The only thing they add is a SECOND clock
// boundary that does not sit at the top level of the loop body — a counted `for`
// and a data-dependent wait — so `control_extract` rewrites them into a `match pc`
// FSM and the segments the old check counted collapse into one.
//
// `RegOut` deliberately: an extracted FSM drives its outputs from `always_ff`, so a
// plain `Out` written in a `pc` state lands a cycle later in SystemVerilog than in
// the simulator. That is the pre-tick alignment family, not a memory question.

#[hardware(sequential)]
async fn rom_paced(clk: Clock<MainClk>, addr: In<Bits<4>, MainClk>, data: RegOut<Bits<16>, MainClk>) {
    let rom = Memory::<Bits<16>, 1, 0, MainClk, 1, 1>::from_fn(clk.clone(), 16, |i| {
        Bits::from_usize(i * 3 + 7)
    });

    loop {
        rom.read_port::<0>().read(addr.read().as_usize());
        clk.tick().await;
        data.write(rom.read_port::<0>().data());
        for _ in 0..2 {
            clk.tick().await;
        }
    }
}

#[hardware(sequential)]
async fn rom_gated(
    clk: Clock<MainClk>,
    go: In<Logic, MainClk>,
    addr: In<Bits<4>, MainClk>,
    data: RegOut<Bits<16>, MainClk>,
) {
    let rom = Memory::<Bits<16>, 1, 0, MainClk, 1, 1>::from_fn(clk.clone(), 16, |i| {
        Bits::from_usize(i * 3 + 7)
    });

    loop {
        while go.read() == Logic::Zero {
            clk.tick().await;
        }
        rom.read_port::<0>().read(addr.read().as_usize());
        clk.tick().await;
        data.write(rom.read_port::<0>().data());
    }
}

// Two writes to ONE port on mutually exclusive branches — a multiplexer on the
// write bus, not a conflict, since no cycle reaches both. Refused until the
// one-access-per-bus rule became a per-path question instead of a per-phase count
// (`rv32i_cpu`'s seven regfile writebacks are the real instance of this shape).
//
// `Memory::new` is ReadFirst, so the read below sees the word the same cycle's
// write is about to replace.
#[hardware(sequential)]
async fn ram_arms(
    clk: Clock<MainClk>,
    sel: In<Logic, MainClk>,
    a: In<Bits<4>, MainClk>,
    b: In<Bits<4>, MainClk>,
    d: In<Bits<8>, MainClk>,
    o: RegOut<Bits<8>, MainClk>,
) {
    let mem = Memory::<Bits<8>, 1, 1, MainClk, 1, 1>::new(clk.clone(), 16);

    loop {
        if sel.read() == Logic::One {
            mem.write_port::<0>().write(a.read().as_usize(), d.read());
        } else {
            mem.write_port::<0>().write(b.read().as_usize(), d.read() + Bits::from_u8(1));
        }
        mem.read_port::<0>().read(a.read().as_usize());
        clk.tick().await;
        o.write(mem.read_port::<0>().data());
    }
}
