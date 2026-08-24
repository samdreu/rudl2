// Memory across phases, and memory consumed combinationally after the tick.

/// The shape that is REJECTED, kept here as the counterexample: a plain `Out`
/// driven straight from a read result in a phase that does not stage it. Neither
/// emitted form is right (see `reject_memory_driven_comb_outputs`), so the
/// transpiler refuses it and points at `RegOut` — which `mp_regout` below uses.
#[hardware(sequential)]
async fn mp_rom(clk: Clock<MainClk>, addr: In<Bits<4>, MainClk>, data: Out<Bits<16>, MainClk>) {
    let rom = Memory::<Bits<16>, 1, 0, MainClk, 1, 1>::from_fn(clk.clone(), 16, |i| {
        Bits::from_usize(i * 3 + 7)
    });
    loop {
        rom.read_port::<0>().read(addr.read().as_usize());
        clk.tick().await;
        data.write(rom.read_port::<0>().data());
        clk.tick().await;
    }
}

/// One tick, and the read result drives an output COMBINATIONALLY — no mediating
/// register. The post-tick segment runs after the capture edge, so the output
/// tracks what that edge produced; a continuous read of the array would instead
/// track the address being staged for the NEXT edge, a cycle early.
#[hardware(sequential)]
async fn rom_direct(clk: Clock<MainClk>, addr: In<Bits<4>, MainClk>, data: Out<Bits<16>, MainClk>) {
    let rom = Memory::<Bits<16>, 1, 0, MainClk, 1, 1>::from_fn(clk.clone(), 16, |i| {
        Bits::from_usize(i * 3 + 7)
    });

    loop {
        rom.read_port::<0>().read(addr.read().as_usize());
        clk.tick().await;
        data.write(rom.read_port::<0>().data());
    }
}


/// S1: consume the read result into a register in the phase AFTER the staging,
/// and drive the output from that register in the staging phase of the next
/// iteration.
#[hardware(sequential)]
async fn mp_reg(clk: Clock<MainClk>, addr: In<Bits<4>, MainClk>, data: Out<Bits<16>, MainClk>) {
    let rom = Memory::<Bits<16>, 1, 0, MainClk, 1, 1>::from_fn(clk.clone(), 16, |i| {
        Bits::from_usize(i * 3 + 7)
    });
    let mut q: Bits<16> = Bits::zero();

    loop {
        data.write(q);
        rom.read_port::<0>().read(addr.read().as_usize());
        clk.tick().await;
        if rom.read_port::<0>().is_ready() {
            q = rom.read_port::<0>().data();
        }
        clk.tick().await;
    }
}

/// S4: same, but the output is a `RegOut` written in the consuming phase.
#[hardware(sequential)]
async fn mp_regout(clk: Clock<MainClk>, addr: In<Bits<4>, MainClk>, data: RegOut<Bits<16>, MainClk>) {
    let rom = Memory::<Bits<16>, 1, 0, MainClk, 1, 1>::from_fn(clk.clone(), 16, |i| {
        Bits::from_usize(i * 3 + 7)
    });

    loop {
        rom.read_port::<0>().read(addr.read().as_usize());
        clk.tick().await;
        data.write(rom.read_port::<0>().data());
        clk.tick().await;
    }
}
