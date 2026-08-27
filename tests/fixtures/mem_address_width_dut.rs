// Memory address width — pins the cast that put a CPU in the differential sweep.
//
// An address net is `clog2(depth)` wide; an index is almost always derived from a
// `usize`, which is 32. `vlir_lower` casts the address to the net's width
// (`10'(idx)`), so the narrowing is explicit and Verilator's `-Wall` — fatal in
// `verification.rs` — accepts it. Without the cast these modules transpile and
// then fail to Verilate, which is what blocked `rv32i_cpu_transpilable`.
//
// Both modules stage before the tick and harvest after it, the `dual_port_ram`
// idiom. That is deliberate: written the other way round they also trip the
// single-tick output-phase divergence in `out_phase_dut.rs`, and a fixture that
// fails for two reasons pins neither. The address is also MASKED rather than
// range-checked, so every cycle really issues an access — a guarded wide address
// is almost never in range under random stimulus, and the module would pass by
// never touching the memory.

/// A 32-bit-derived index into a 1024-word memory: 32 bits into a 10-bit net.
/// This is the shape every address in `rv32i_cpu_transpilable` has.
#[hardware(sequential)]
pub async fn wide_index_into_narrow_addr(
    clk: Clock<MainClk>,
    addr: In<Bits<32>, MainClk>,
    d: In<Bits<32>, MainClk>,
    we: In<Logic, MainClk>,
    q: Out<Bits<32>, MainClk>,
) {
    let mem = Memory::<Bits<32>, 1, 1, MainClk, 1, 1>::new(clk.clone(), 1024);
    let mut held: Bits<32> = Bits::zero();
    loop {
        // Shifted down so the value is always in range, and RANGE-CHECKED so the
        // index has a second consumer that reads all 32 bits — see
        // `wide_index_sole_consumer` below for what happens without one. This is
        // exactly the shape of every address in `rv32i_cpu_transpilable`.
        let i: usize = (addr.read() >> 22).as_usize();
        if i < 1024 {
            if we.read() == Logic::One {
                mem.write_port::<0>().write(i, d.read());
            } else {
                mem.read_port::<0>().read(i);
            }
        }

        clk.tick().await;

        if mem.read_port::<0>().is_ready() {
            held = mem.read_port::<0>().data();
        }
        q.write(held);
    }
}

/// The control: an index whose width already matches the address net, so the cast
/// is a no-op. If a future change made the cast conditional and got the condition
/// wrong, this is the module that would not notice — which is why it sits next to
/// the one that would.
#[hardware(sequential)]
pub async fn narrow_index_into_narrow_addr(
    clk: Clock<MainClk>,
    addr: In<Bits<10>, MainClk>,
    d: In<Bits<32>, MainClk>,
    we: In<Logic, MainClk>,
    q: Out<Bits<32>, MainClk>,
) {
    let mem = Memory::<Bits<32>, 1, 1, MainClk, 1, 1>::new(clk.clone(), 1024);
    let mut held: Bits<32> = Bits::zero();
    loop {
        if we.read() == Logic::One {
            mem.write_port::<0>().write(addr.read().as_usize(), d.read());
        } else {
            mem.read_port::<0>().read(addr.read().as_usize());
        }

        clk.tick().await;

        if mem.read_port::<0>().is_ready() {
            held = mem.read_port::<0>().data();
        }
        q.write(held);
    }
}

/// **BROKEN, and found by writing the module above.** The same wide index, but the
/// memory address is its ONLY consumer. `10'(i)` reads `i[9:0]`, so the upper 22
/// bits of the `usize` local are driven and never read — UNUSEDSIGNAL, which
/// `-Wall` makes fatal. Nothing to do with the cast being wrong; `usize` is 32 bits
/// unconditionally and a memory address is narrower, so an index local that feeds
/// nothing else always has a dead half. `rv32i_cpu_transpilable` escapes it because
/// its indices are range-checked, which reads the whole word.
///
/// DECIDED AND FIXED 2026-08-27 ("emit the index at the address width"):
/// `vlir_lower::narrow_sole_resize_wires` declares a wire at the one narrower
/// width every use resizes it to, truncating its assignments explicitly —
/// `logic [9:0] i; i = 10'((addr >> 22));` — the same bits the consumers always
/// read. Any other use disqualifies (`wide_index_into_narrow_addr` keeps its 32
/// bits). Sweeps green under `-Wall`.
#[hardware(sequential)]
pub async fn wide_index_sole_consumer(
    clk: Clock<MainClk>,
    addr: In<Bits<32>, MainClk>,
    d: In<Bits<32>, MainClk>,
    we: In<Logic, MainClk>,
    q: Out<Bits<32>, MainClk>,
) {
    let mem = Memory::<Bits<32>, 1, 1, MainClk, 1, 1>::new(clk.clone(), 1024);
    let mut held: Bits<32> = Bits::zero();
    loop {
        let i: usize = (addr.read() >> 22).as_usize();
        if we.read() == Logic::One {
            mem.write_port::<0>().write(i, d.read());
        } else {
            mem.read_port::<0>().read(i);
        }

        clk.tick().await;

        if mem.read_port::<0>().is_ready() {
            held = mem.read_port::<0>().data();
        }
        q.write(held);
    }
}
