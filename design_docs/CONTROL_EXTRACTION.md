# Control Extraction: async control flow → explicit FSM

**Status:** design + in-progress (2026-07-24). The milestone behind the README's
"write FSMs naturally with async/await" claim. Blocks `uart/rx` and
`det_010_awaits`.

## Problem

The current phase model (`split_at_ticks` in `shir_lower.rs`) only handles
`clk.tick().await` at the **top level** of the loop body, as a *linear* sequence:
`n_ticks = segments.len() - 1`, one phase per tick, `phase_r` counting
`0 → 1 → … → 0`. Ticks inside `if`/`while`/`for` are rejected (`TickInsideBranch`,
"while loops not supported", "tick inside a for-loop"). So it cannot express:

- **Branching ticks** — different control paths take different numbers of ticks
  (`det_010_awaits`).
- **Data-dependent waits** — `while cond { clk.tick().await; }`.
- **Counted delays** — `for _ in 0..K { clk.tick().await; }` (uart: 434-cycle bit
  periods).

## Key insight

An async loop body with ticks **is** a state machine, and the hand-written
explicit-FSM form already exists and is verified: `det_010` (single-tick, explicit
`State` enum) is the hand-written equivalent of `det_010_awaits` (multi-tick
async), and the two produce **identical simulator output** (proven by
`pattern_detector_2`'s test). So control extraction = *automatically producing the
explicit single-tick FSM from the async body.*

## Architecture: a FIR→FIR pass that reuses the single-tick pipeline

Do control extraction as a **source-level (FIR) transformation** that rewrites an
async control-flow loop body into an **explicit single-tick FSM**:

```
loop { <async control flow with ticks anywhere> }
        ↓  control extraction
<pc + counters init>
loop {
    match pc {
        S0 => { <actions>; pc = <next>; }
        S1 => { … }
        …
    }
    clk.tick().await;          // exactly one tick
}
```

The output is a **single-tick** loop with a `match` on a program-counter register —
exactly the shape the existing pipeline already lowers correctly (see `det_010`,
`mac_fsm`). So we reuse all of CHIR/SHIR/VLIR/emit unchanged; the new code is
confined to the FIR pass.

### Algorithm (CFG → states)

1. **Enumerate suspend points.** Each `clk.tick().await` is a state; the loop entry
   is the start state `S0`. A state = "we are suspended at this tick; next cycle we
   resume just after it."
2. **Per-state segment.** For each state, the *combinational* work executed on
   resume, up to the next tick(s), plus the **next-state** assignment. Straight-line
   code chains states; branches make the next-state conditional.
3. **Control-flow lowering:**
   - **Sequence** `a; tick; b` → state for `a`'s tail (ends by ticking → next
     state runs `b`).
   - **`if c { …ticks… } else { …ticks… }`** → the segment ends with
     `pc = c ? <first state of then> : <first state of else>`; branch tails
     re-merge to the continuation state.
   - **`while c { …tick… }`** → a **self-loop** state: `pc = c ? <self> : <after>`.
   - **`for _ in 0..K { tick }`** → a **counter** register + self-loop:
     `cnt = cnt + 1; pc = (cnt == K-1) ? (cnt=0, <after>) : <self>`.
   - **`continue`/`break`** → `pc = <loop head>` / `<after loop>`.
4. **State live values** (variables live across a tick) are handled *after*
   flattening: the resulting single-tick FSM has them cross its one `.await`, so
   the existing register-promotion turns them into registers automatically.

### Increments

- **(A) straight-line + `if`/`else` branching** — the core CFG→FSM, no counters or
  loops. Reproduces today's linear behavior via the new mechanism, then adds
  branches. **← starting here.**
- **(B) `while` self-loops** — data-dependent waits.
- **(C) counted `for` delays** — a counter register (uart bit periods).
- **(D) `continue`/`break`, `if let`, early paths** — as needed by uart.

### Validation

`det_010_awaits` is the adjudicator: once it transpiles, check its Verilog against
`det_010`'s already-verified Verilog (both proven equal in sim). If they match, the
async-FSM story is proven end-to-end — and it simultaneously resolves the timing
debate (a known-good reference instead of first-principles argument). `uart/rx` is
the stretch goal (needs A–D + memory-free datapath).

## Relationship to the timing reconciliation

Control extraction produces a **single-tick** FSM, which is the category where sim
and hardware already agree. So if the flattening is faithful, the multi-tick timing
discrepancy (EXECUTION_MODEL_RECONCILIATION.md) *dissolves* for extracted FSMs —
the extraction itself defines the cycle-accurate semantics, matching the explicit
hand-written FSM. This is a strong reason to prefer extraction over patching the
multi-phase lowering.
