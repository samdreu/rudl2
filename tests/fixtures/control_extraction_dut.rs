// Fixtures for the increment-A control-extraction STRUCTURAL equivalence test.
//
// Each async branch-tick module is paired with a hand-written explicit single-tick
// `match pc` FSM — the FSM a human would write for it. The test transpiles both
// and asserts the generated SystemVerilog is identical (module name aside): proof
// that control extraction produces exactly the intended FSM, independent of the
// simulator (and so immune to the still-open output-timing reconciliation — see
// EXECUTION_MODEL_RECONCILIATION.md). This is the det_010_awaits→det_010 method,
// applied to the simplest straight-line + if/else cases.

// ── Case 1: if_tick — asymmetric tick counts (then: 1 tick, else: 2). ──────────

#[hardware(sequential)]
async fn if_tick(
    clk: Clock<MainClk>,
    sel: In<Logic, MainClk>,
    out_o: Out<Logic, MainClk>,
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
    out_o: Out<Logic, MainClk>,
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
