// Preloaded-memory DUTs — one per `Memory` constructor that states contents.
//
// Both are ROMs (1 read port, 0 write ports): the ONLY thing that can make the
// output correct is the preload, so a missing or mis-ordered `initial` block
// shows up immediately as zeros rather than hiding behind a write that happens to
// put the right value there.

/// `from_fn` — contents described by an expression in the index. Emitted as a
/// fill loop, not as evaluated constants: the transpiler does not run Rust.
#[hardware(sequential)]
async fn rom_from_fn(
    clk: Clock<MainClk>,
    addr: In<Bits<4>, MainClk>,
    data: Out<Bits<16>, MainClk>,
) {
    let rom = Memory::<Bits<16>, 1, 0, MainClk, 1, 1>::from_fn(clk.clone(), 16, |i| {
        Bits::from_usize(i * 3 + 7)
    });
    let mut q: Bits<16> = Bits::zero();

    loop {
        rom.read_port::<0>().read(addr.read().as_usize());
        clk.tick().await;
        if rom.read_port::<0>().is_ready() {
            q = rom.read_port::<0>().data();
        }
        data.write(q);
    }
}

/// `from_contents` — contents listed word by word. The values are deliberately
/// NOT monotonic, so an off-by-one or reversed fill cannot pass.
#[hardware(sequential)]
async fn rom_from_contents(
    clk: Clock<MainClk>,
    addr: In<Bits<2>, MainClk>,
    data: Out<Bits<8>, MainClk>,
) {
    let rom = Memory::<Bits<8>, 1, 0, MainClk, 1, 1>::from_contents(
        clk.clone(),
        vec![
            Bits::from_u8(0xAB),
            Bits::from_u8(0x12),
            Bits::from_u8(0xF0),
            Bits::from_u8(0x34),
        ],
    );
    let mut q: Bits<8> = Bits::zero();

    loop {
        rom.read_port::<0>().read(addr.read().as_usize());
        clk.tick().await;
        if rom.read_port::<0>().is_ready() {
            q = rom.read_port::<0>().data();
        }
        data.write(q);
    }
}
