// Word-indexed ARRAY REGISTERS — the register file, the pipelined CPU's last
// transpile blocker (landed 2026-08-27).
//
// `let mut regs = [Bits::<W>::zero(); N]` at pre-loop position is a register
// per the shared inference; with a multi-bit element it lowers as an INTERNAL
// MEMORY (an unpacked array with the write staged and committed at the edge by
// the existing memory machinery) plus COMBINATIONAL word reads (`regs[idx]`
// emits an array select, not a staged read port).
//
// The semantics are the simulator's statement order, per the cycle-dataflow
// model:
//
//   - a read BEFORE the write statement sees the COMMITTED array (the value as
//     of the opening edge) — `regfile_read_first` and the constant-index read
//     in `regfile_write_through` pin this;
//   - a read AFTER the write statement sees the written value in the SAME
//     cycle (write-through — the classic WB→ID regfile forwarding half-cycle).
//     Lowered as a mux against `<regs>_fwd_{en,addr,data}` wires assigned at
//     the write site; blocking-assign order makes the mux position-aware, so
//     the same wires serve both cases.
//
// Next-cycle visibility (write in iteration k, read in iteration k+1) rides
// the memory commit: staged during cycle k, committed at the closing edge,
// read combinationally in k+1.

/// Reads only BEFORE the write statement: both outputs must lag the write by a
/// full cycle (the committed array), like a classic read-first RAM.
#[hardware(sequential)]
pub async fn regfile_read_first(
    clk: Clock<MainClk>,
    wen: In<Logic, MainClk>,
    waddr: In<Bits<3>, MainClk>,
    wdata: In<Bits<8>, MainClk>,
    raddr: In<Bits<3>, MainClk>,
    o: Out<Bits<8>, MainClk>,
    o3: Out<Bits<8>, MainClk>,
) {
    let mut regs = [Bits::<8>::zero(); 8];
    loop {
        clk.tick().await;
        o.write(regs[raddr.read().as_usize()]);
        o3.write(regs[3]);
        if wen.read() == Logic::One {
            regs[waddr.read().as_usize()] = wdata.read();
        }
    }
}

/// The CPU's shape: a constant-index read BEFORE the write (committed value),
/// then the write, then a read AFTER it (same-cycle write-through).
#[hardware(sequential)]
pub async fn regfile_write_through(
    clk: Clock<MainClk>,
    wen: In<Logic, MainClk>,
    waddr: In<Bits<3>, MainClk>,
    wdata: In<Bits<8>, MainClk>,
    raddr: In<Bits<3>, MainClk>,
    o: Out<Bits<8>, MainClk>,
    o3: Out<Bits<8>, MainClk>,
) {
    let mut regs = [Bits::<8>::zero(); 8];
    loop {
        clk.tick().await;
        o3.write(regs[3]);
        if wen.read() == Logic::One {
            regs[waddr.read().as_usize()] = wdata.read();
        }
        o.write(regs[raddr.read().as_usize()]);
    }
}
