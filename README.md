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
> regression-tested, but the language surface is deliberately narrow. Every
> example module transpiles (34/34); see [Limitations](#limitations) for what
> is refused **by design** and what is still a gap — the list is measured
> rather than aspirational.

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

The largest design in the tree is a 5-stage pipelined RV32I CPU
(`examples/cpu/rv32i_cpu_pipelined.rs` — forwarding, load-use stalls, branch
flush, a word-indexed register file, and a unified instruction/data memory
received as a parameter). It runs 13 architectural programs on the simulator
**and** on its own transpiled SystemVerilog under Verilator, matching
cycle-for-cycle:

```bash
cargo test --test rv32i_pipelined_verilator
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

The driver also enforces four wiring guards, because "the check silently didn't
run" has been a recurring bug class here: every `examples/**.rs` is registered as
a `[[example]]` (**G-A**), every registered example actually ran (**G-B**), every
`tests/*.rs` produced a test binary that ran (**G-C**), and the corpus differential
sweep covered every `#[hardware]` module and ran (**G-D**). It prints every
`#[ignore]`d test on each run so a deliberately-skipped check stays visible.

### The corpus differential sweep

`build.rs` generates one equivalence case per `#[hardware]` module in `examples/`
and `tests/fixtures/`: seeded random stimulus into the simulator, and the
SystemVerilog that module transpiles to Verilated against the same trace, 200
cycles each. No reference model is needed — the simulator and the emitted SV are
two independent implementations of one source, so comparing them is already an
oracle — which is what makes a case cheap enough to have for *every* module
rather than for whichever ones someone wrote a harness for.

A module the sweep cannot run gets an `#[ignore]` with its reason, never an
omission, and G-D fails if the generator quietly stops covering something. Widths
for generic modules, resets for designs whose state starts undefined, and the
reason for each skip live in three tables in `build.rs`. See
`design_docs/CORPUS_DIFFERENTIAL_SWEEP.md`.

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

- **1006 tests pass, 0 fail, 23 ignored** across 122 test binaries
- **26 of 26 examples pass**, Verilator equivalence included
- **131 corpus differential cases pass**, 12 ignored with a recorded reason
- the pipelined RV32I CPU matches its Verilated self **cycle-for-cycle on all
  13 architectural programs** (`tests/rv32i_pipelined_verilator.rs`)
- all four wiring guards (G-A / G-B / G-C / G-D) clean

Every ignored test prints its reason on each run. They are divergence witnesses,
shapes refused by design, modules blocked on a recorded transpiler cause, and one
documented startup transient — not silent skips.

---

## Limitations

Measured, not guessed — **34 of 34** `#[hardware]` modules in `examples/`
transpile (`tools/transpile_coverage.sh` prints the current number). The cause
ledger that used to sit here — `Vec` ports, tuple-returning helpers, struct
pipeline latches, const match patterns, the word-indexed register file, the
whole-file `spawn_uart` rejection — is empty as of 2026-08-27; its full history
lives in [`TODO`](TODO).

Transpiling is not the same as being *checked*: coverage counts acceptance, and
the corpus sweep above is what says the emitted SystemVerilog agrees with the
simulator. The equivalence harness runs Verilator under `-Wall`, and warnings
are failures.

What remains unsupported falls into two kinds.

**Decisions, not gaps** — constructs that cannot be given the same meaning in
simulation and in silicon are refused with a spanned error and a recorded
counterexample rather than lowered to something plausible:

- **The mid-phase read seam**: an `In` read placed *after* a delay
  (`for _ in 0..N { clk.tick().await; } let x = input.read();`) samples
  post-edge in the simulator but pre-edge in a flip-flop, a cycle apart under
  any testbench that moves inputs between edges. Write the read first and the
  delay last — `examples/uart/rx.rs` shows the anchored spelling.
- **The pre-tick alignment family**: five compile-time rules refuse plain-`Out`
  drive shapes whose phase would silently differ between the simulator and the
  synthesized FSM (a path-dependent read boundary, a port written in more than
  one clock phase, and their trailing-segment variants). The remedy each error
  names is usually `RegOut`. Five *rejected* fixes are recorded with measured
  evidence in
  [`design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md`](design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md)
  so they are not re-tried.
- A **wait loop that ticks before testing** its condition, and `continue`
  inside such a wait — refused orderings, each with its own diagnostic.
- **Zero-latency memories**, two accesses to one memory port in a cycle, and
  run-time-computed memory preloads — memory shapes that would lower to
  something subtly wrong.
- **Two writes to one array register in a cycle**: `let mut regs = [Bits<W>; N]`
  lowers to storage with one write port; a second write statement in the same
  cycle is refused with the muxed-address rewrite suggested.

**Gaps** — expressible in the simulator, not yet in the transpiler:

- the division operator `/` (`%` works);
- a behavioral module is **single-clock**; multi-clock designs compose clock
  domains through `#[hardware(structural)]` parents (transpile-only today);
- the CLI transpiles **concrete** modules only — generic ones are
  monomorphized at example-run time;
- a module that *receives* a `Memory` parameter emits a bus the corpus sweep
  cannot drive by itself; anchoring one behaviorally takes a Verilated parent
  that owns the array (`tests/rv32i_pipelined_verilator.rs` is the pattern).

One correctness rule is worth knowing when reading the generated Verilog: an
output drive that is *sampled at the clock edge* is emitted from the values the
registers hold **before** that edge, while one emitted as a continuous `assign` is
read **after** it. Copper carries both forms and picks at the point where the
choice actually exists, because "is this drive edge-sampled?" is not answerable
from the port type — a plain `Out` written conditionally becomes a register too.
Getting this wrong is a silent one-cycle error, so it is pinned by
`tests/regout_forwarding_equivalence.rs` rather than left to inspection.

And X-propagation cannot be checked against Verilator even in principle
(Verilator is 2-state), so Copper's 4-state behavior is pinned by its own tests
instead.

The everything-still-open list lives in [`TODO`](TODO).

---

## Documentation

| | |
|---|---|
| [`design_docs/SYNCHRONOUS_SEMANTICS.md`](design_docs/SYNCHRONOUS_SEMANTICS.md) | the timing model — start here |
| [`design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md`](design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md) | the executor/analysis architecture and its sequenced items |
| [`design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md`](design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md) | the pre-tick alignment family — its five compile-time rules, and the rejected fixes with measured evidence |
| [`design_docs/CORPUS_DIFFERENTIAL_SWEEP.md`](design_docs/CORPUS_DIFFERENTIAL_SWEEP.md) | why every module gets a generated sim-vs-emitted-SV case, and how |
| [`design_docs/TIMING_MODEL_UNIFICATION.md`](design_docs/TIMING_MODEL_UNIFICATION.md) | how far the simulator's and the transpiler's timing derivations actually diverge — measured |
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
