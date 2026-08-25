# Copper

**A Rust-embedded hardware description language.** You write hardware as
`#[hardware]`-annotated async Rust functions. Copper both **simulates** them —
with a custom async executor that models clock edges and delta cycles — and
**transpiles** them to synthesizable SystemVerilog.

```rust
#[hardware(combinational)]
fn one_bit_comparator(i0: In<Logic, ()>, i1: In<Logic, ()>, eq: Out<Logic, ()>) {
    let p0 = !i0.read() & !i1.read();
    let p1 = i0.read() & i1.read();
    eq.write(p0 | p1);
}
```

```systemverilog
module one_bit_comparator (
    input  logic i0,
    input  logic i1,
    output logic eq
);
    logic p0;
    logic p1;

    always_comb begin
        p0 = ((!i0) & (!i1));
        p1 = (i0 & i1);
    end

    assign eq = (p0 | p1);
endmodule
```

The project's correctness bar is that **the two agree**. Most examples run the
Copper simulator, emit SystemVerilog plus a Verilator C++ testbench, compile it,
and assert the two match cycle-by-cycle — and the sharpest of them check Copper
against *independent, third-party* Verilog rather than against Copper's own
output.

> **Status: research prototype (v0.1.0).** The semantics are anchored and
> regression-tested, but the language surface is deliberately narrow. See
> [Limitations](#limitations) for the measured list of what does not transpile
> yet — it is kept honest rather than aspirational.

---

## Why async Rust?

The central idea is that `clk.tick().await` **is** a clock-cycle boundary:

- every suspension point becomes a state in the generated FSM;
- every value **live across a tick** becomes a **register** (inferred by
  backward liveness analysis, not by an annotation you write);
- everything between ticks is combinational logic.

So an ordinary Rust `loop` with an `enum` state variable is a state machine, and
you get to keep Rust's type system, generics, `match`, and tooling while writing
it. Here is a real example — `examples/sequential/pattern_detector.rs`, a
"110101" sequence detector — and the SystemVerilog Copper emits for it:

```rust
#[hardware(sequential)]
async fn det_110101(
    clk: Clock<MainClk>,
    rstn: In<Logic, MainClk>,
    in_i: In<Logic, MainClk>,
    out_o: Out<Logic, MainClk>,
) {
    let mut state = State::IDLE;
    loop {
        if rstn.read() == Logic::Zero {
            state = State::IDLE;
        } else {
            state = match (state, in_i.read()) {
                (State::IDLE,   Logic::One)  => State::S1,
                (State::S1,     Logic::One)  => State::S11,
                (State::S11,    Logic::Zero) => State::S110,
                (State::S110,   Logic::One)  => State::S1101,
                (State::S1101,  Logic::Zero) => State::S11010,
                (State::S11010, Logic::One)  => State::S110101,
                _ => State::IDLE,
            };
        }
        if state == State::S110101 {
            out_o.write(Logic::One);
        } else {
            out_o.write(Logic::Zero);
        }

        clk.tick().await;
    }
}
```

```systemverilog
module det_110101 (
    input  logic clk,
    input  logic rstn,
    input  logic in_i,
    output logic out_o
);
    logic [2:0] state;          // inferred: `state` is live across the tick

    always_comb begin
        if ((state == 3'd6)) out_o = 1'b1;
        else                 out_o = 1'b0;
    end

    always_ff @(posedge clk) begin
        state <= /* the match, lowered to a nested ternary on {state, in_i} */ ;
    end
endmodule
```

Nobody declared a register. `state` became one because it is assigned in the
loop and read after the tick.

---

## Requirements

| | |
|---|---|
| **Rust** | edition 2024 (rustc ≥ 1.85; developed on 1.92) |
| **Verilator** | required for examples and equivalence tests — `brew install verilator` (developed on 5.044) |

Verilator is not optional for a full regression run. A genuinely *absent*
binary is skipped with an explicit marker; an installed-but-broken one fails
loudly, by design — "the check silently didn't run" is a bug class this project
takes seriously.

## Quick start

```bash
cargo build --workspace
```

Run one example end-to-end (simulate → transpile → Verilate → compare):

```bash
cargo run --example pattern_detector
```

Run the full regression — build, CLI, `cargo test --workspace`, and **every**
registered example:

```bash
tools/regression.sh
```

A clean full run ends with `REGRESSION OK`. Any subset ends with `PARTIAL OK`,
so a partial run can never be mistaken for a full one:

```bash
tools/regression.sh --quick          # fast loop: build + CLI + a few examples
tools/regression.sh --no-examples    # build + CLI + tests, no Verilator
tools/regression.sh --example lfsr   # one named example
```

The driver also enforces three wiring guards, because "the check silently didn't
run" has been a recurring bug class here: every `examples/**.rs` is registered as
a `[[example]]` (**G-A**), every registered example actually ran (**G-B**), and
every `tests/*.rs` produced a test binary that ran (**G-C**). It prints every
`#[ignore]`d test on each run so a deliberately-skipped check stays visible.

### Transpiling from the command line

```bash
cargo run -q -p copper-codegen --bin copper-transpile -- examples/sequential/pattern_detector.rs
```

| Flag | Meaning |
|---|---|
| `-o <path>` | write to a file (default: stdout) |
| `--module <name>` | which module to emit (required when a file has more than one) |
| `--profile <p>` | `verilator` (default) \| `generic` \| `yosys` |
| `--hierarchy` | also emit every submodule the target instantiates, deepest-first |
| `--list` | list the hardware modules in the file, then exit |

The CLI handles **concrete** (non-generic) modules only. Generic modules
(`const WIDTH_P: usize`, `Bits<W>`, `Clock<D>`) are monomorphized at
example-run time by the `#[hardware]` macro, so to see their SystemVerilog, run
the example.

---

## Writing hardware

### Module kinds

| Attribute | Shape | Use for |
|---|---|---|
| `#[hardware(combinational)]` | plain `fn`, no ticks | pure logic; every output must be assigned on every path |
| `#[hardware(sequential)]` | `async fn` with one top-level `loop` containing `clk.tick().await` | registers, FSMs, datapaths |
| `#[hardware(synchronizer)]` | `async fn` | the **only** place a clock-domain crossing is legal |
| `#[hardware(structural)]` | no ticks, instantiates children | hierarchy, including multi-clock parents (transpile-only today) |

Every path through the loop body must reach a `clk.tick().await`. A loop that
can spin without ticking is a spanned **compile error**, in both the simulator
front end and the transpiler — a combinational loop is not a thing you can
accidentally write.

### `Out` vs `RegOut`

`Out<T, D>` is a plain wire. `RegOut<T, D>` is a **registered** output driven
from `always_ff`. Use `RegOut` for write-before-tick Moore outputs; plain `Out`
otherwise. A sequential `Out` **holds** its value when a path does not write it
(the enabled-register idiom — verified against BaseJump's `bsg_dff_en`), so
"assign on all paths" is a rule for combinational modules only.

### Clock domains are types

Signals and clocks carry a phantom domain type (`Clock<D>`, `In<T, D>`,
`Out<T, D>`, `D: ClockDomain`). Wiring one domain into another domain's port is
a nominal type mismatch — it fails at `cargo build`, not in a lint or a review.
The only legal crossing is inside a `#[hardware(synchronizer)]` module, such as
the standard library's `sync_2ff`. A module may receive or clone a clock, never
construct one.

`copper-core/src/cdc.rs` contains no runtime code: its `compile_fail` doctests
**are** the executable specification, so `cargo test` fails if a CDC guarantee
regresses.

### Memory

`Memory<T, R, W, D, READ_LAT, WRITE_LAT>` is a first-class construct — `R` read
ports and `W` write ports on domain `D`, with configurable latencies and
read-during-write mode (`ReadFirst` / `WriteFirst`), preloadable via `from_fn` /
`from_contents`. It simulates and transpiles — see
`examples/memory/dual_port_ram.rs`, checked against an independent hand-written
RAM template.

---

## Repository layout

Five crates, plus the root crate as the hardware standard library:

| Crate | Role |
|---|---|
| **`copper-core`** | The type system and every IR in the pipeline: `Logic`, `Bits<N>`, `Clock<D>`, ports, `Memory`, the phantom clock-domain machinery, and the `frontend_ir` / `chir` / `shir` / `vlir` structs. A leaf crate — which is what lets the proc-macro and the transpiler share analysis without a dependency cycle. |
| **`copper-analysis`** | The shared compile-time control/liveness analysis: a real CFG over `syn::ItemFn`, backward-liveness register inference, and the reachability check. Keyed off the representation *both* front ends already hold, so the simulator and the transpiler consume **one** authoritative analysis rather than two that must agree. |
| **`copper-macros`** | The `#[hardware(...)]` proc macro — signature validation, CDC enforcement, per-read freshness guards, and making a module a `HardwareModule` so a bare `async fn` won't simulate. |
| **`copper-codegen`** | The transpiler and the `copper-transpile` CLI. |
| **`copper-sim`** | The async simulation executor and the Verilator equivalence verifier. |
| **`copper`** (root `src/`) | The hardware standard library (e.g. `sync_2ff`), which must live downstream of `copper-sim` because the macro expands to `::copper_sim::…` paths. |

### The transpilation pipeline

```
Rust #[hardware] fn
  → FIR    source-shaped frontend IR
  → control_extract    flatten branch-nested ticks into an explicit `match pc` FSM
  → CHIR
  → SHIR
  → VLIR   1:1 with legal SystemVerilog — legalized names, concrete widths
  → SystemVerilog text
```

### The timing model

A tick has four phases: **pre-edge settle → clock edge → post-edge settle →
post-edge observation**. Simulation is **independent of Rust's async poll
order** — a well-formed design simulates identically under any task ordering,
which a dedicated poll-order fuzzer checks. The production scheduler is
levelized (a topologically-ordered combinational DAG with Tarjan SCC
condensation); a convergent combinational loop is iterated, a non-convergent one
is a structural error rather than a hang.

The full semantics live in
[`design_docs/SYNCHRONOUS_SEMANTICS.md`](design_docs/SYNCHRONOUS_SEMANTICS.md).

---

## Verification

Copper's central discipline is that **the transpiler is never treated as a
correctness oracle for the simulator's semantics**. Three kinds of check, in
descending order of authority:

1. **Sim vs independent third-party Verilog** under Verilator — the only kind
   that can adjudicate a timing question. `examples/basejump/` checks Copper
   modules against [BaseJump STL](https://github.com/bespoke-silicon-group/basejump_stl)
   Verilog that Copper had no hand in writing.
2. **Sim vs Copper-generated Verilog** under Verilator, plus a Rust reference
   model — good for datapath equivalence, circular for timing.
3. **Sim vs itself** — self-consistency only.

Register inference is additionally checked *structurally*: the registers the
analysis infers are matched reg-for-reg against the independent reference
Verilog, not merely against matching behavior.

Current state, as of the last full regression run:

- **767 tests pass, 0 fail, 4 ignored** across 94 test binaries
- **26 of 26 examples pass**, Verilator equivalence included
- all three wiring guards (G-A / G-B / G-C) clean

---

## Limitations

Measured, not guessed — **24 of 34** `#[hardware]` modules in `examples/`
currently transpile. The 10 that do not, grouped by root cause:

| Cause | Blocks |
|---|---|
| array-typed ports (`In<[Bits<W>; ELS]>`) | `mux`, `bsg_mux_one_hot` |
| `Vec` ports | `rv32i_cpu`, `rv32i_cpu_pipelined` |
| bit-width inference on a bare integer local | `bsg_gray_to_binary`, `ripple_carry_adder` |
| no structural parent in simulation | `uart/system` (`uart_tx`, `uart_rx`) |
| `while` loops unsupported | `uart/rx` |
| a tick inside a conditional branch | `det_010_awaits` |

Note that transpiling and *linting* are different bars: the equivalence harness
runs Verilator under `-Wall`, and a module can emit SystemVerilog that the CLI
accepts but the linter rejects (a `usize` local becomes a 64-bit signal, so
assigning it to a narrow port is a width-truncation error).

Also unsupported, each pinned with its own diagnostic and a regression test:
signed arithmetic / arithmetic shift right, division, `continue` inside a nested
loop, and several memory shapes that would lower to something subtly wrong
(two accesses to one port in a cycle, run-time-computed preloads, zero-latency
memories).

Two further notes on what "unsupported" means here. Some of these are
**decisions, not gaps** — a construct that cannot be given the same meaning in
simulation and in silicon is refused with a spanned error and a recorded
counterexample rather than lowered to something plausible. And X-propagation
cannot be checked against Verilator even in principle (Verilator is 2-state), so
Copper's 4-state behavior is pinned by its own tests instead.

The everything-still-open list lives in [`TODO`](TODO).

---

## Documentation

| | |
|---|---|
| [`design_docs/SYNCHRONOUS_SEMANTICS.md`](design_docs/SYNCHRONOUS_SEMANTICS.md) | the timing model — start here |
| [`design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md`](design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md) | the executor/analysis architecture and its sequenced items |
| [`design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md`](design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md) | the pre-tick alignment hazard, its compile-time guard, and three rejected fixes with evidence |
| [`design_docs/LEVELIZED_SCHEDULING_SCOPE.md`](design_docs/LEVELIZED_SCHEDULING_SCOPE.md) | the levelized scheduler |
| [`design_docs/TIMING_COVERAGE_MATRIX.md`](design_docs/TIMING_COVERAGE_MATRIX.md) | which timing patterns have an independent hardware anchor |
| [`CLAUDE.md`](CLAUDE.md) | orientation for contributors (and for Claude Code) |

`design_docs/OUTDATED/` is history, not authority.

---

## License

The Copper source does not yet carry a license declaration.

Third-party Verilog vendored as **independent references** under
`examples/*/sv/` is adapted from
[BaseJump STL](https://github.com/bespoke-silicon-group/basejump_stl) and is
covered by the **Solderpad Hardware License v0.51** (Apache-2.0-based),
Copyright 2016 Michael B. Taylor / BaseJump STL contributors. Each such file
carries its own attribution header naming the upstream module and the
adaptations made.
