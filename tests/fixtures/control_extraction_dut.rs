// Fixtures for the increment-A control-extraction STRUCTURAL equivalence test.
//
// Each async branch-tick module is paired with a hand-written explicit single-tick
// `match pc` FSM — the FSM a human would write for it. The test transpiles both
// and asserts the generated SystemVerilog is identical (module name aside): proof
// that control extraction produces exactly the intended FSM, independent of the
// simulator (and so immune to the still-open output-timing reconciliation — see
// EXECUTION_MODEL_RECONCILIATION.md). This is the det_010_awaits→det_010 method,
// applied to the simplest straight-line + if/else cases.
//
// `if_tick`/`match_tick` write their output on both sides of a bare tick after a
// leading input read — the multi-write-around-a-tick pattern the macro guardrail
// (`copper_analysis::multi_write_collapse`) rejects for a plain (combinational)
// `Out`, because the simulator would collapse it. Their outputs are therefore
// `RegOut` (registered / non-blocking), which both satisfies the guardrail and
// makes sim ≡ transpiled SV (see tests/nested_tick_equivalence.rs). Their explicit
// twins mirror the `RegOut` so the structural SV comparison stays apples-to-apples.

// FSM state enum for the match-nested-tick case (Case 3).
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Single,
    Double,
}

// ── Case 1: if_tick — asymmetric tick counts (then: 1 tick, else: 2). ──────────

#[hardware(sequential)]
async fn if_tick(
    clk: Clock<MainClk>,
    sel: In<Logic, MainClk>,
    out_o: RegOut<Logic, MainClk>,
) {
    loop {
        if sel.read() == Logic::One {
            out_o.write(Logic::One);
            clk.tick().await;
        } else {
            out_o.write(Logic::Zero);
            clk.tick().await;
            out_o.write(Logic::One);
            clk.tick().await;
        }
    }
}

#[hardware(sequential)]
async fn if_tick_explicit(
    clk: Clock<MainClk>,
    sel: In<Logic, MainClk>,
    out_o: RegOut<Logic, MainClk>,
) {
    let mut pc: u8 = 0;
    loop {
        match pc {
            0u8 => {
                if sel.read() == Logic::One {
                    out_o.write(Logic::One);
                    pc = 0;
                } else {
                    out_o.write(Logic::Zero);
                    pc = 1;
                }
            }
            1u8 => {
                out_o.write(Logic::One);
                pc = 0;
            }
            _ => {}
        }
        clk.tick().await;
    }
}

// ── Case 2: branch_merge — continuation (tail_o) after the if, duplicated. ─────

#[hardware(sequential)]
async fn branch_merge(
    clk: Clock<MainClk>,
    sel: In<Logic, MainClk>,
    head_o: Out<Logic, MainClk>,
    mid_o: Out<Logic, MainClk>,
    tail_o: Out<Logic, MainClk>,
) {
    loop {
        head_o.write(Logic::One);
        if sel.read() == Logic::One {
            clk.tick().await;
        } else {
            mid_o.write(Logic::One);
        }
        tail_o.write(Logic::One);
        clk.tick().await;
    }
}

#[hardware(sequential)]
async fn branch_merge_explicit(
    clk: Clock<MainClk>,
    sel: In<Logic, MainClk>,
    head_o: Out<Logic, MainClk>,
    mid_o: Out<Logic, MainClk>,
    tail_o: Out<Logic, MainClk>,
) {
    let mut pc: u8 = 0;
    loop {
        match pc {
            0u8 => {
                head_o.write(Logic::One);
                if sel.read() == Logic::One {
                    pc = 1;
                } else {
                    mid_o.write(Logic::One);
                    tail_o.write(Logic::One);
                    pc = 0;
                }
            }
            1u8 => {
                tail_o.write(Logic::One);
                pc = 0;
            }
            _ => {}
        }
        clk.tick().await;
    }
}

// ── Case 3: match_tick — ticks INSIDE `match` arms, one arm with a mid-arm tick. ─
// Exercises the match-arm generalization of `lower_into`: descending into arms (not
// just `if`) and allocating a fresh `pc` state for the `Double` arm's second cycle.

#[hardware(sequential)]
async fn match_tick(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    b: In<Bits<8>, MainClk>,
    out: RegOut<Bits<8>, MainClk>,
) {
    let mut mode = Mode::Single;
    loop {
        match mode {
            Mode::Single => {
                out.write(a.read());
                mode = Mode::Double;
                clk.tick().await;
            }
            Mode::Double => {
                out.write(a.read());
                clk.tick().await;
                out.write(b.read());
                mode = Mode::Single;
                clk.tick().await;
            }
        }
    }
}

#[hardware(sequential)]
async fn match_tick_explicit(
    clk: Clock<MainClk>,
    a: In<Bits<8>, MainClk>,
    b: In<Bits<8>, MainClk>,
    out: RegOut<Bits<8>, MainClk>,
) {
    let mut mode = Mode::Single;
    let mut pc: u8 = 0;
    loop {
        match pc {
            0u8 => match mode {
                Mode::Single => {
                    out.write(a.read());
                    mode = Mode::Double;
                    pc = 0;
                }
                Mode::Double => {
                    out.write(a.read());
                    pc = 1;
                }
            },
            1u8 => {
                out.write(b.read());
                mode = Mode::Single;
                pc = 0;
            }
            _ => {}
        }
        clk.tick().await;
    }
}
