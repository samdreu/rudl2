# Guiding Figures (draft)

> Paper-ready code snippets, each trimmed to ~column length and mapped to a contribution
> (see `intro_contributions.md`). Snippets are faithful to the source with elisions marked
> `// ...`. Source file:line ranges given so the full listing is recoverable.
> `[VERIFY]` = confirm the elision didn't drop something load-bearing before final typesetting.

## Figure plan (where each goes)

| Fig | Example | Frames contribution | Section |
|---|---|---|---|
| 1 (teaser) | `simple_counter` (paired: source + generated SV) | C1 async-FSM + C2 same-source, on page 1 | Intro |
| 2 | UART RX (`examples/uart/rx.rs`) | **C1**: control-flow *is* the state machine | §Overview / motivating example |
| 3 | UART system (`examples/uart/system.rs`) | **C3**: hierarchical composition + move-only single-driver | §Composition & ownership |
| 4 | RV32I pipeline (`examples/cpu/rv32i_cpu_pipelined.rs`) | **C1 at scale**: pipeline regs = live-across-await vars; commit = clock edge | §Case study |
| 5 (not code) | equivalence harness diagram | **C2**: same source → rustc sim + transpiler → Verilator | §Sim/synth correspondence |

---

## Figure 1 — Teaser: same source, both sides (counter)

**Decision (settled):** the teaser is `simple_counter`, shown as a **paired figure** — Copper
source on the left, generated SystemVerilog on the right — because the teaser's job is to show
the signature move *and its payoff* (C1 async-FSM + C2 same-source) on page 1. The counter is
the only example small enough to fit both halves in one figure.

### 1a — Copper source (real; `tests/fixtures/counter_dut.rs`)

```rust
async fn counter(
    clk: Clock<MainClk>,              // phantom-typed clock domain
    step: In<Bits<8>, MainClk>,       // input port, domain-tagged
    out: Out<Bits<8>, MainClk>,       // output port: move-only ⇒ single driver
) {
    let mut count: Bits<8> = Bits::zero();   // lives across .await ⇒ a register
    loop {
        out.write(count);             // drive output from current state
        clk.tick().await;             // ── clock edge ──
        count = count + step.read();  // next-state update
    }
}
```

### 1b — Generated SystemVerilog (right half)

> **TODO — paste real transpiler output here once the transpiler is fixed.**
> Blocked on the parser/lowering desync on this branch (parser emits `ExprType::Path` for a
> bare-identifier `port.write()` receiver; `chir_lower.rs` still matches only `ExprType::Lit`
> at 3 sites — `:696`, `:744`, `:804`). Owner: transpiler fix planned.
>
> Once green, generate the real listing with:
> ```
> cargo run -p copper-codegen --bin copper-transpile -- tests/fixtures/counter_dut.rs
> ```
> Do NOT hand-write or mock this half — the whole point of the figure is that it is the
> transpiler's actual output. `[TODO: paste + verify Verilator-lint-clean]`

**Caption (draft):** *The Copper source (left) is a step-adjustable counter; the SystemVerilog
(right) is the transpiler's output for that exact source. `count` is an ordinary Rust local,
but because it is live across `clk.tick().await`, Rust's `async` lowering makes it state — a
register in both the cycle-accurate simulation and the generated hardware. The two are checked
behaviorally equivalent under Verilator (`tests/m1_counter_equivalence.rs`), so page 1 shows
both the signature construct and the guarantee that ties simulation to synthesis.*

> Status note: the counter/lfsr equivalence tests are new and currently failing on this branch
> due to the transpiler desync above; caption's "checked equivalent" wording is accurate once
> the fix lands. See `00_claims_audit.md` for the C2 status caveat.

---

## Figure 2 — Control flow *is* the state machine (UART RX)

*Purpose:* the strongest illustration of C1. A textbook UART receiver is a 5-state FSM
(IDLE/START/DATA/STOP/CLEANUP) with an explicit state register, a bit counter, and a
clocks-per-bit counter. In Copper **none of those are declared**: the state is the position
in the async control flow, and the counters are the loop induction variables.
Source: `examples/uart/rx.rs:45-88` (elided).

```rust
async fn rx(
    clk: Clock<MainClk>,
    rx_serial: In<Logic, MainClk>,
    rx_dv:   Out<Logic, MainClk>,     // "data valid" strobe
    rx_byte: Out<Bits<8>, MainClk>,
) {
    loop {
        // IDLE: wait for the falling edge of the start bit
        while rx_serial.read() == Logic::One { clk.tick().await; }

        // START: wait to the center of the start bit, then verify
        for _ in 0..CLKS_PER_BIT / 2 { clk.tick().await; }
        if rx_serial.read() != Logic::Zero { continue; }  // false start ⇒ back to IDLE

        // DATA: sample 8 bits, one bit-period apart
        let mut byte_val = 0;
        for i in 0..8 {
            for _ in 0..CLKS_PER_BIT { clk.tick().await; }
            if rx_serial.read() == Logic::One { byte_val |= 1 << i; }
        }

        // STOP + one-cycle data-valid pulse
        for _ in 0..CLKS_PER_BIT { clk.tick().await; }
        rx_dv.write(Logic::One);
        rx_byte.write(Bits::from_u8(byte_val));
        clk.tick().await;
        rx_dv.write(Logic::Zero);
    }
}
```

**Caption (draft):** *A 115200-baud 8N1 UART receiver. The conventional five-state FSM
(IDLE→START→DATA→STOP→CLEANUP) is expressed entirely through async control flow: `while`
is the IDLE wait, nested `for` loops are the oversampling counters, and `continue` is the
false-start transition back to IDLE. The state register and both counters that a Verilog
implementation declares explicitly are here the program counter and induction variables of
the generated state machine.*

> **Decision (settled):** the figure does **not** show any state enum. Make the
> control-flow-as-FSM point in prose: "a conventional receiver would declare a five-value
> state register and two counters; the Copper version declares none — the state is the
> position in the async control flow and the counters are loop induction variables."
> Do not surface the unused `enum State` from `rx.rs`. (Optional cleanup: delete that unused
> enum from the source so the shipped artifact has no dead code — ask before editing examples.)

---

## Figure 3 — Hierarchical composition + single-driver ownership (UART system)

*Purpose:* C3. Module composition uses ordinary Rust functions — no `module`/port-map
syntax. The internal TX→RX serial wire never escapes `spawn_uart`. Output ports are
move-only, so the compiler guarantees exactly one driver per wire, and two `spawn_uart`
calls yield provably disjoint port sets. Source: `examples/uart/system.rs:108-143` (elided).

```rust
struct UartPorts {                    // caller-visible interface only
    tx_byte:  Out<Bits<8>, MainClk>,  // caller drives  (owns the Out)
    tx_start: Out<Logic,   MainClk>,
    tx_busy:  In<Logic,    MainClk>,  // caller observes (holds a cloned In)
    rx_dv:    In<Logic,    MainClk>,
    rx_byte:  In<Bits<8>,  MainClk>,
}

fn spawn_uart(exec: &mut HardwareExecutor, clk: Clock<MainClk>) -> UartPorts {
    // Internal wire: TX serial out → RX serial in. Created, used, and hidden here.
    let (serial_out, serial_in) = wire::<Logic, MainClk>(Logic::One);
    let (tx_byte_port, tx_byte_in) = wire::<Bits<8>, MainClk>(Bits::zero());
    /* ... other caller-side wires ... */

    exec.spawn_wired(uart_tx(clk.clone(), tx_byte_in, /* ... */ serial_out, tx_busy_out), /* ... */);
    exec.spawn_wired(uart_rx(clk.clone(), serial_in, rx_dv_out, rx_byte_out), /* ... */);

    UartPorts { tx_byte: tx_byte_port, /* ... */ }   // serial wire is NOT in the interface
}

// Two independent channels; the type system guarantees their ports are disjoint.
let uart0 = spawn_uart(&mut exec, clk.clone());
let uart1 = spawn_uart(&mut exec, clk.clone());
```

**Caption (draft):** *Hierarchical composition in Copper is an ordinary Rust function.
`spawn_uart` instantiates a TX and an RX submodule, wires them with an internal `serial`
wire that never appears in the returned `UartPorts`, and exposes only the five caller-facing
ports. Because `Out<T,D>` is non-`Clone` (move-only), each wire has exactly one owner and
therefore one driver — the single-driver rule is discharged by the borrow checker. Two calls
to `spawn_uart` produce two structurally disjoint instances with no shared state.*

---

## Figure 4 — The approach at scale: RV32I 5-stage pipeline

*Purpose:* C1 scaled to a real design. The whole datapath is one `async fn`; the pipeline
latches, register file, PC, and memory are `let mut` locals that survive `.await` and so
become registers/memories. Each loop iteration is one cycle; the trailing **commit block** is
the clock-edge register update (all `new_*` values computed combinationally, then latched at
once — the non-blocking-assignment semantics, made explicit).
Source: `examples/cpu/rv32i_cpu_pipelined.rs:198-432` (heavily elided).

```rust
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
```

**Caption (draft):** *A full RV32I five-stage pipeline (IF/ID/EX/MEM/WB) with forwarding,
load-use hazard stalling, and branch flushing — expressed as a single `async fn`. The four
pipeline latches, the 32-entry register file, the program counter, and unified memory are
all ordinary Rust locals; being live across `clk.tick().await`, they become the design's
state. Stages are computed into `new_*` temporaries and installed together in the commit
block, making the register-transfer boundary explicit. Hazard, forwarding, and control logic
are plain `if`/`match` over those values.*

> **Decision (settled):** split into **4a** (skeleton above) and **4b** (one representative
> stage below). The skeleton makes the "state = live-across-await locals, commit = clock edge"
> point; 4b shows that the stage logic really is plain Rust over those values. Full 235-line
> listing goes to the appendix/artifact.

### Figure 4b — one stage in detail: the forwarding unit

*Purpose:* show that a genuinely tricky piece of pipeline control — operand forwarding with
EX/MEM-over-MEM/WB priority — is expressed as ordinary Rust `if`/`else` over the latch
locals, no special HDL construct. Source: `examples/cpu/rv32i_cpu_pipelined.rs:260-281`.

```rust
// Forwarding unit: prefer the newest producer (EX/MEM) over the older (MEM/WB).
// EX/MEM forwarding is suppressed for loads — the value isn't ready until MEM,
// so the hazard unit stalls one cycle instead (see load_use_stall).
let fwd_rs1 = {
    let from_ex_mem = ex_mem.valid && ex_mem.writes_reg && !ex_mem.is_load
                      && ex_mem.rd != 0 && ex_mem.rd == id_ex.rs1;
    let from_mem_wb = mem_wb.valid && mem_wb.writes_reg
                      && mem_wb.rd != 0 && mem_wb.rd == id_ex.rs1;
    if from_ex_mem      { ex_mem.alu_result }
    else if from_mem_wb { mem_wb.result }
    else                { id_ex.rs1_val }
};
// fwd_rs2 is symmetric (matches on id_ex.rs2).
```

**Caption (draft):** *The operand-forwarding logic for the pipeline of Fig. 4a. Detecting
whether the value in `id_ex` should come from the EX/MEM latch, the MEM/WB latch, or the
register file is a plain conditional over the pipeline-register locals; the EX/MEM-priority
policy is just the order of the `if`/`else if`. The load exception (`!ex_mem.is_load`) and
the corresponding one-cycle stall live in the same straight-line Rust.*

---

## Figure 5 — Same-source correspondence (diagram, not code)

*Purpose:* C2. A box diagram, not a listing:

```
                       ┌─ rustc (async lowering) ─► cycle-accurate sim ─► trace_A ─┐
   one .rs source ─────┤                                                            ├─► assert trace_A ≡ trace_B
                       └─ copper-codegen (FIR→CHIR→SHIR→VLIR) ─► .sv ─► Verilator ─► trace_B ─┘
```

Point the reader at `tests/lfsr_equivalence.rs`: the DUT is `include!`d as Rust *and*
`include_str!`d as text for `transpile_source`, so both paths provably consume the identical
source. **Caption should state current scope honestly: demonstrated end-to-end for `counter`
and `lfsr`.** `[VERIFY: update as coverage grows]`

---

## Open decisions for you
- [x] Teaser: **`simple_counter`, paired (source + generated SV).** SV half is a TODO pending transpiler fix.
- [x] Fig 4: **skeleton (4a) + one-stage detail (4b, forwarding unit).**
- [x] UART RX figure: **no state enum shown**; control-flow-as-FSM made in prose. (Optional: delete unused `enum State` from `rx.rs` source — needs go-ahead.)
- [ ] Whether Fig 5 stays a diagram or becomes a small code+diagram combo.
