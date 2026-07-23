async fn traffic_light(
    clk: Clock<MainClk>,              // phantom-typed clock domain
    request: In<Logic, MainClk>,      // input port, domain-tagged
    red: Out<Logic, MainClk>,         // output port: move-only ⇒ single driver
    yellow: Out<Logic, MainClk>,
    green_out: Out<Logic, MainClk>,
) {
    let mut phase = Phase::Green;     // lives across .await ⇒ a register
    let mut timer: u8 = 0;            //   "        "        ⇒ a register
    loop {
        match phase {                 // Moore outputs: f(current state)
            Phase::Green => { red.write(Logic::Zero); /* ... */ green_out.write(Logic::One); }
            /* ... */
        }
        clk.tick().await;             // ── clock edge ──
        (phase, timer) = match (phase, timer, request.read()) {
            (Phase::Green, _, Logic::One) => (Phase::Yellow, 0),
            /* ... next-state logic ... */
        };
    }
}

#[hardware(sequential)]
async fn rv32i_cpu_pipelined(
    clk: Clock<MainClk>,
    program: In<Vec<Bits<32>>, MainClk>,
    program_counter: Out<Bits<32>, MainClk>,
    halted: Out<Logic, MainClk>,
    a0_out: Out<Bits<32>, MainClk>,
) {
    // All state below is live across .await ⇒ pipeline registers / register file / memory
    let mut memory = { let mut v = program.read(); v.resize(1024, Bits::zero()); v };
    let mut regs   = [Bits::<32>::zero(); 32];
    let mut pc: Bits<32> = Bits::zero();
    let mut if_id  = IFIDReg::bubble();   // IF→ID latch
    let mut id_ex  = IDEXReg::bubble();   // ID→EX latch
    let mut ex_mem = EXMEMReg::bubble();  // EX→MEM latch
    let mut mem_wb = MEMWBReg::bubble();  // MEM→WB latch

    loop {
        clk.tick().await;                 // ── one iteration = one clock cycle ──

        // Stages computed in reverse (WB→MEM→EX→ID→IF) so each reads last cycle's latch.
        // ... WB: retire mem_wb into regs; detect ecall/halt ...
        // ... MEM: load/store against `memory` ...
        // ... forwarding unit: EX/MEM > MEM/WB priority ...
        // ... EX: ALU / branch / jump, produce (new_ex_mem, flush, branch_target) ...
        // ... load-use hazard detection ⇒ load_use_stall ...
        // ... ID: decode if_id.instr ⇒ new_id_ex ...
        // ... IF: fetch at pc (or hold/flush) ⇒ new_if_id ...
        let new_pc = if flush { branch_target }
                     else if load_use_stall { pc }
                     else { pc + Bits::<32>::from_lit::<4>() };

        // ── Commit: the clock-edge register update ──
        pc = new_pc;
        if_id = new_if_id; id_ex = new_id_ex; ex_mem = new_ex_mem; mem_wb = new_mem_wb;
        program_counter.write(pc);
    }
}
