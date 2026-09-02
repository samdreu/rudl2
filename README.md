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
> `#[hardware]` module under `examples/` transpiles (34 of 34 as of 2026-09-01;
> `tools/transpile_coverage.sh` re-measures it); see [Limitations](#limitations)
> for what is refused **by design** and what is still a gap — the list is
> measured rather than aspirational.

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

    logic [2:0] state;

    always_comb begin
        if ((state == 3'd6)) begin
            out_o = 1'b1;
        end else begin
            out_o = 1'b0;
        end
    end

    always_ff @(posedge clk) begin
        state <= ((rstn == 1'b0) ? 3'd0 : (({state, in_i} == 4'd1) ? 3'd1 : (({state, in_i} == 4'd3) ? 3'd2 : (({state, in_i} == 4'd4) ? 3'd3 : (({state, in_i} == 4'd7) ? 3'd4 : (({state, in_i} == 4'd8) ? 3'd5 : (({state, in_i} == 4'd11) ? 3'd6 : 3'd0)))))));
    end

endmodule
```

That block is the CLI's output verbatim (`cargo run -q -p copper-codegen --bin
copper-transpile -- examples/sequential/pattern_detector.rs`); the one long line
is the `match` on `(state, in_i)` lowered to a nested ternary over the
concatenation `{state, in_i}`. Nobody declared a register. `state` became one
because it is assigned in the loop and read after the tick.

---

## Requirements

| | |
|---|---|
| **Rust** | every crate is `edition = "2024"`, which needs rustc ≥ 1.85 (no `rust-version` field pins it; developed on 1.92) |
| **Verilator** | required for examples and equivalence tests — `brew install verilator` (developed on 5.044) |
| **Icarus Verilog** | *optional* — `brew install icarus-verilog` (developed on 12.0). Used only by the simulator-throughput benchmark `tests/sim_throughput.rs` and by `tools/stats/simperf.py`; when `iverilog` is absent that test skips its Icarus leg (`iverilog_available`) and the stats script refuses to write a CSV |

Verilator is not optional for a full regression run. A genuinely *absent*
binary is skipped with an explicit marker (`VERILATOR_NOT_INSTALLED` in
`copper-sim/src/verification.rs`); an installed-but-broken one fails loudly, by
design — "the check silently didn't run" is a bug class this project takes
seriously. The same three-way split (absent / present / present-but-broken)
applies to Icarus.

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

The driver also enforces five wiring guards, because "the check silently didn't
run" has been a recurring bug class here: every `examples/**.rs` is registered as
a `[[example]]` (**G-A**), every registered example actually ran (**G-B**), every
`tests/*.rs` (root and per-crate) produced a test binary that ran (**G-C**), the
corpus differential sweep covered every module in `examples/` and
`tests/fixtures/` and ran with Verilator present (**G-D**), and — printed rather
than enforced — how many of those swept modules are additionally anchored to an
independent hand-written Verilog reference (**G-E**). It prints every
`#[ignore]`d test on each run so a deliberately-skipped check stays visible.

### The corpus differential sweep

`build.rs` generates one equivalence case per `#[hardware]` module in `examples/`
and `tests/fixtures/` (not `src/`, and not modules defined inline in a
`tests/*.rs` file): seeded random stimulus into the simulator, and the
SystemVerilog that module transpiles to Verilated against the same trace — 200
cycles for a sequential module, 64 vectors for a combinational one (`SEQ_CYCLES`
/ `COMB_VECTORS` in `build.rs`). No reference model is needed — the simulator
and the emitted SV are two independent implementations of one source, so
comparing them is already an oracle — which is what makes a case cheap enough to
have for *every* module rather than for whichever ones someone wrote a harness
for.

A module the sweep cannot run gets an `#[ignore]` with its reason, never an
omission, and G-D fails if the generator quietly stops covering something. Widths
for generic modules (`PARAMS`), resets for designs whose state starts undefined
(`RESET`), the reason for each skip (`SKIP`), and an optional independent
Verilog reference to check the module against as well (`REFERENCE`) live in four
tables in `build.rs`. See `design_docs/CORPUS_DIFFERENTIAL_SWEEP.md` and
`design_docs/ANCHORING_A_MODULE.md`.

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

A **generic** module (`const N: usize`, `Bits<N>`) transpiles to a *parametric*
SystemVerilog module — `copper-transpile examples/combinational/rotate_right.rs`
emits `module rotate_right #(parameter int N = 1, parameter int N_LOG = 1)`. The
CLI cannot choose the widths for you: parameters are often constrained
(`N_LOG == clog2(N)`, asserted inside the module), so concrete values are
supplied only when the module is *Verilated* — `HardwareTest::with_params` in
`copper-sim/src/testing.rs`, or the `PARAMS` table in `build.rs` for the sweep.

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
`examples/memory/dual_port_ram.rs`, checked against an independently written
RAM reference (`examples/memory/sv/dual_port_ram.sv`, a textbook simple-dual-port
block-RAM template that Copper had no hand in).

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
post-edge observation**. The production scheduler is `SchedulerMode::Levelized`
(`copper-sim/src/executor.rs`): a topologically-ordered combinational DAG with
Tarjan SCC condensation, evaluated in one canonical pass, so Rust's async poll
order cannot influence the result. The older iterate-to-fixpoint delta-cycle
scheduler is kept permanently as the differential oracle
(`COPPER_SCHEDULER=fixpoint`), and *that* is what the poll-order fuzzer
(`tests/poll_order_fuzz.rs`) exercises — a well-formed design must simulate
identically under any `PollOrder` there, or the levelized-vs-fixpoint comparison
loses its footing. A convergent combinational loop is iterated to a fixed point
(`iterate_scc`); one that is still changing after `OSCILLATION_THRESHOLD` passes
is a **panic** naming the offending SCC, not a hang and not a silent
approximation.

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

- **1010 tests pass, 0 fail, 21 ignored** across 123 test binaries
- **26 of 26 examples pass**, Verilator equivalence included
- **133 corpus differential cases pass**, 10 ignored with a recorded reason
- the pipelined RV32I CPU matches its Verilated self **cycle-for-cycle on all
  13 architectural programs** (`tests/rv32i_pipelined_verilator.rs`)
- all five wiring guards (G-A / G-B / G-C / G-D / G-E) clean; G-E reports 2
  swept modules anchored to an independent hand-written reference on top of the
  sim-vs-emitted-SV check (`ram_read_first`, `bit_not_bits` — the `REFERENCE`
  table in `build.rs`)

Every ignored test prints its reason on each run. They are pinned one-cycle
divergence witnesses that keep `allow_pretick_alignment` on purpose, shapes
refused by design, a structural parent with no simulatable body, the pipelined
CPU (anchored in its own lane because the sweep cannot supply its `Memory`
parameter), diagnostic printouts with no assertions, and one documented startup
transient — not silent skips. `tools/regression.sh` prints the full list with
reasons at the end of every run (the sweep's come from the `SKIP` table in
`build.rs`; three more are hand-written under `tests/`).

---

## Limitations

Measured, not guessed — **34 of 34** `#[hardware]` modules in `examples/`
transpile as of 2026-09-01 (`tools/transpile_coverage.sh` prints the current
number). The cause ledger that used to sit here — `Vec` ports, tuple-returning
helpers, struct pipeline latches, const match patterns, the word-indexed
register file, the whole-file `spawn_uart` rejection — is empty as of
2026-08-27; its full history lives in [`TODO`](TODO), the engineering journal
and cause ledger (dated entries, not a curated open list).

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
- A **wait loop that ticks before testing** its condition (a repeating wait must
  be `loop { <test>; clk.tick().await; }`), and `continue` inside a nested loop
  — refused orderings, each with its own diagnostic
  (`copper-codegen/src/control_extract.rs`).
- **Zero-latency memories** (`READ_LAT` and `WRITE_LAT` must both be at least
  1), a memory port accessed more than once in one cycle
  (`check_memory_staging`), a plain `Out` driven from a memory read result
  across clock phases (`memory_result_drives_plain_out`), and memory preloads
  that are not a closure written at the `from_fn` call site — memory shapes
  that would lower to something subtly wrong.
- **Two writes to one array register in a cycle**: `let mut regs = [Bits<W>; N]`
  lowers to storage with one write port; a second write statement in the same
  cycle is refused with the muxed-address rewrite suggested.
- `while` loops (write `loop { …; clk.tick().await; }`), `..rest` struct
  update, and or-patterns that bind a name — each a spanned "not supported"
  error from `copper-codegen/src/chir_lower.rs`.

**Gaps** — expressible in the simulator, not yet in the transpiler:

- the division operator `/` (`%` works — see `lower_binop` in
  `copper-codegen/src/chir_lower.rs`);
- a behavioral module is **single-clock** (the macro requires exactly one
  `Clock<D>` parameter); multi-clock designs compose clock domains through
  `#[hardware(structural)]` parents, which transpile but have no simulatable
  body — the simulator wires the hierarchy by hand;
- a module that *receives* a `Memory` parameter emits a bus the corpus sweep
  cannot drive by itself; anchoring one behaviorally takes a Verilated parent
  that owns the array (`tests/rv32i_pipelined_verilator.rs` is the pattern).

**Pinned divergences** — shapes the simulator and the transpiler still disagree
on by one cycle, kept as `#[ignore]`d sweep cases with the reason attached
rather than papered over (`SKIP` in `build.rs`):

- a `RegOut` written *after* the tick in a **single-tick** loop: the transpiler
  folds the write into this edge, the simulator commits it on the next
  (`regout_trailing_single_tick`) — reaching for `RegOut` does not fix the
  trailing plain-`Out` shape the pre-tick rules refuse;
- a phase-gated cross-tick read (`probe_fsm`, the W4 path-dependent boundary
  from the guardrail doc);
- an `Out` first written in the trailing segment drives from cycle 1 in the
  simulator but from time 0 as a continuous `assign` — a **startup transient**,
  identical from cycle 1 on (`trailing_constant`).

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

[`TODO`](TODO) is the engineering journal and cause ledger — dated entries
recording what was measured, decided and closed, not a curated list of what is
still open.

---

## Documentation

| | |
|---|---|
| [`design_docs/SYNCHRONOUS_SEMANTICS.md`](design_docs/SYNCHRONOUS_SEMANTICS.md) | the timing model — start here |
| [`design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md`](design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md) | the pre-tick alignment family — its five compile-time rules, and the rejected fixes with measured evidence |
| [`design_docs/CORPUS_DIFFERENTIAL_SWEEP.md`](design_docs/CORPUS_DIFFERENTIAL_SWEEP.md) | why every module gets a generated sim-vs-emitted-SV case, and how |
| [`design_docs/ANCHORING_A_MODULE.md`](design_docs/ANCHORING_A_MODULE.md) | how to add an independent Verilog reference to a swept module (the `REFERENCE` table) |
| [`design_docs/LEVELIZED_SCHEDULING_SCOPE.md`](design_docs/LEVELIZED_SCHEDULING_SCOPE.md) | the levelized scheduler |
| [`design_docs/ARRAY_PORT_ABI.md`](design_docs/ARRAY_PORT_ABI.md), [`design_docs/RECEIVED_MEMORY_ABI.md`](design_docs/RECEIVED_MEMORY_ABI.md) | the emitted port shapes for array-typed ports and for a `Memory` received as a parameter |
| [`design_docs/CYCLE_DATAFLOW_SEMANTICS.md`](design_docs/CYCLE_DATAFLOW_SEMANTICS.md), [`design_docs/DERIVATION_TABLE.md`](design_docs/DERIVATION_TABLE.md), [`design_docs/PAIRED_IMPLEMENTATION_SCOPE.md`](design_docs/PAIRED_IMPLEMENTATION_SCOPE.md) | the cycle-dataflow denotation behind the pre-tick rules, and its derivation across the corpus |
| [`design_docs/TIMING_MODEL_UNIFICATION.md`](design_docs/TIMING_MODEL_UNIFICATION.md) | how far the simulator's and the transpiler's timing derivations actually diverge — measured |
| [`design_docs/TIMING_COVERAGE_MATRIX.md`](design_docs/TIMING_COVERAGE_MATRIX.md) | which timing patterns have an independent hardware anchor |
| [`design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md`](design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md) | the historical implementation plan for the executor/analysis architecture — its status notes are dated; `SYNCHRONOUS_SEMANTICS.md` is normative where they disagree |
| [`CLAUDE.md`](CLAUDE.md) | orientation for contributors (and for Claude Code) |

`design_docs/OUTDATED/` is history, not authority.

---

## License

The Copper source does not yet carry a license declaration.

The Verilog vendored as **independent references** under `examples/*/sv/` and
`tests/fixtures/reference_sv/` comes from three sources, and each file's header
is the record:

- **BaseJump STL, under the Solderpad Hardware License v0.51**
  (Apache-2.0-based), Copyright 2016 Michael B. Taylor / BaseJump STL
  contributors — the seven files in `examples/basejump/sv/`, plus
  `examples/combinational/sv/mux.sv`, `ripple_carry_adder.sv`,
  `rotate_right.sv` and `examples/sequential/sv/lfsr.sv`. Each carries a header
  naming the upstream module, the license, and the adaptations made
  (BaseJump macros stripped, widths pinned, `clk_i` renamed).
- **Hand-written for this repository**, and saying so in the header:
  `examples/cdc/sv/sync_2ff_ref.sv`, `examples/cdc/sv/two_domain_hierarchy.sv`,
  `examples/sequential/sv/pattern_detector_010.sv`,
  `tests/fixtures/reference_sv/ram_read_first.sv`; the other files under
  `tests/fixtures/reference_sv/` are test-specific reference models written for
  the tests that name them.
- **Adapted from third-party tutorial or template sources**:
  `examples/sequential/sv/shift_register.sv` (chipverify.com, cited in its
  header), `examples/combinational/sv/priority_encode.sv` (Greg Stitt, University
  of Florida, cited in its header), and three whose headers record no provenance
  — `examples/memory/sv/dual_port_ram.sv` (a textbook simple-dual-port block-RAM
  template), `examples/sequential/sv/pattern_detector.sv`, and
  `examples/combinational/sv/one_bit_comparator.sv`. No license is recorded for
  any of these five.

`examples/counter_enable.sv` is tracked but referenced by nothing in the tree.
