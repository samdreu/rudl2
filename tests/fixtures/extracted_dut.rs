// CONTROL-EXTRACTED variants of the shapes the pipelined CPU exercises — the
// `match pc` FSM places the whole body PRE-tick (head segment), where the
// trailing-form fixtures' measured timings do not automatically carry over.
// Each module here is its trailing-form sibling plus a branch-nested halt wait
// (`if halt { loop { tick } }`), which is what forces extraction.
//
// Ports are `RegOut`: with `In`-fed writes in an extracted body the pretick
// guard refuses a plain `Out` (path-dependent boundary via the halt path), and
// registration is its prescribed remedy — the differential still measures the
// array/forwarding timing, one registered cycle later.
//
//   - `ex_regfile`        — regfile_dut's write-through module, extracted:
//                           array-register commit timing and the staging-net
//                           mux measured in the head form.
//   - `ex_reg_then_wire`  — a comb wire reading a register ASSIGNED EARLIER in
//                           the same segment (`r = r + x; let w = r + 1`): the
//                           wire must see the assigned value, as the
//                           simulator's statement order does.

/// The CPU's regfile shape under extraction: constant read before the write,
/// dynamic write, dynamic read after it (write-through).
#[hardware(sequential)]
pub async fn ex_regfile(
    clk: Clock<MainClk>,
    halt: In<Logic, MainClk>,
    wen: In<Logic, MainClk>,
    waddr: In<Bits<3>, MainClk>,
    wdata: In<Bits<8>, MainClk>,
    raddr: In<Bits<3>, MainClk>,
    o: RegOut<Bits<8>, MainClk>,
    o3: RegOut<Bits<8>, MainClk>,
) {
    let mut regs = [Bits::<8>::zero(); 8];
    loop {
        clk.tick().await;
        // All input reads up front, on every path — the uniform boundary the
        // pretick guard requires of a plain `Out` in this shape.
        let h = halt.read();
        let we = wen.read();
        let wa = waddr.read();
        let wd = wdata.read();
        let ra = raddr.read();
        if h == Logic::One { loop { clk.tick().await; } }
        o3.write(regs[3]);
        if we == Logic::One {
            regs[wa.as_usize()] = wd;
        }
        o.write(regs[ra.as_usize()]);
    }
}

/// A wire computed from a register assigned earlier in the same segment.
#[hardware(sequential)]
pub async fn ex_reg_then_wire(
    clk: Clock<MainClk>,
    halt: In<Logic, MainClk>,
    x: In<Bits<8>, MainClk>,
    o: RegOut<Bits<8>, MainClk>,
) {
    let mut r: Bits<8> = Bits::zero();
    loop {
        clk.tick().await;
        let h = halt.read();
        let xv = x.read();
        if h == Logic::One { loop { clk.tick().await; } }
        r = r + xv;
        let w = r + Bits::<8>::from_lit::<1>();
        o.write(w);
    }
}
