# Corpus differential sweep — scope and as-built design

**Status:** phases 1–4 BUILT and green (2026-08-25). `build.rs` generates one
differential case for every `#[hardware]` module in `tests/fixtures/` and
`examples/` — those two directories only; `src/sync.rs::sync_2ff` and modules
declared inline in test files are outside it (a `sync_2ff_concrete` twin is swept)
— and `tools/regression.sh`'s **G-D** asserts the sweep covered the corpus and ran.

The counts move as the corpus does, so trust the tool over any number written here:
`build.rs` prints `corpus sweep: N generated, M ignored-with-reason` on every build,
and G-D prints `corpus sweep ran N differential cases (M ignored-with-reason)` on
every full run. Verified 2026-09-01: **133 running, 10 ignored-with-reason**, out of
143 modules. Every one of the ten is a row in `build.rs`'s `SKIP` table — a recorded
transpiler refusal, a structural module with no simulatable body, a deliberate
divergence witness, a documented startup transient, or the pipelined CPU, whose
`Memory` parameter the harness cannot supply. **None is "the generator cannot do
this."**

## 0. What the sweep found — the 2026-08-25 log, compressed

Phases 1–4 were built and run in one day; this is what each found, kept because
the findings are the argument for the sweep.

* **Phase 1 (hand-written cases for the unchecked backlog): two defects** in modules
  that had been in the tree for weeks. A measured sim ≠ synth divergence —
  `branch_merge_explicit` and its async twin transpile to byte-identical
  SystemVerilog, the twin agrees with it for 200 random cycles and it leads by one
  (D1's constant-write exemption is unsound for a conditionally-written `Out`;
  `PRETICK_ALIGNMENT_GUARDRAIL.md` §5.5, pinned in
  `tests/sequential_forwarding_divergence.rs`). And invalid emitted SystemVerilog —
  `event_source` has a port named `event`, a keyword missing from the legalizer's
  reserved list. Neither was reachable from the existing tests: that example is
  checked against hand-written Verilog, and nothing had ever Verilated what it
  transpiles to. Lesson carried into the generator: the testbench addresses the
  Verilated model by the **emitted** port name (`event` → `event_sig`), so the
  generated case calls `copper_codegen::legalized_port_name` at run time rather than
  reimplementing the rule.
* **Phase 2 (`build.rs` over `tests/fixtures/`): nothing** — every fixture agreed
  with its emitted SystemVerilog on the first run. The load-bearing part is the
  guard, `every_corpus_module_has_a_generated_case` in `tests/corpus_generated.rs`,
  which re-scans the corpus at test time and asserts the generated manifest
  (`COVERED`) matches in both directions; verified by negative control (dropping a
  module from the generator fails naming it). The runtime fit inside `cargo test`'s
  existing parallelism, which settled the "does it belong in the default path"
  question.
* **Phase 3 (extend to `examples/`): nothing new.** What it needed: per-file import
  and clock-domain rules keyed on file *content* (a fixture declares no `use` and no
  `ClockDomain`, an example brings both), `__`-aliased imports in the generated body,
  and the public `legalized_port_name`. One skip was too strict and was relaxed —
  a width written as a file-scope `const` is concrete, so the BaseJump modules sweep.
* **Phase 4 (generics, array ports, reset): a design's undefined region.**
  `shift_register` initialises its register to `Bits::x()` — deliberately, an unreset
  flip-flop is X — and the simulator carries X where Verilator's 2-state model reads
  0. Comparing undefined against undefined is a test of nothing, so the `RESET` table
  drives the reset port asserted on cycle 0 and randomly after. Generic modules
  sweep at the widths in `PARAMS` (the earlier "cannot emit it" reason was simply
  wrong — a generic module emits a *parametric* SystemVerilog module and needs only
  `-G`). Array ports sweep via `RandStim for [T; N]` (`tests/common/mod.rs`) whose bit
  layout is the one the hand-written array tests already record. Type rewriting in
  the generator is token-wise (`alias_ty`): `Bits<N_LOG>` must not be rewritten by a
  rule for `N`.
* **After the log:** the `SKIP` table gained `<wrapper>::<module>` keys because the
  *example* copy of `ripple_carry_adder` refused (cause J-b) while the fixture copy
  swept; J-b closed 2026-08-27 and both copies sweep, but the key form stays. On
  2026-09-01 an audit found two `SKIP` rows whose reasons had silently stopped being
  true (`bit_not_bits`, `lit_width_in_ternary`); both were deleted, both sweep, and
  `build.rs` now validates every `SKIP` key against the corpus at build time so a
  dead row fails the build. A stale *reason* still cannot be detected mechanically —
  see §3.4.

---

## 1. Why this and not the individual bugs

The repo `TODO` records "acceptance is not correctness" repeatedly, and the reason
it keeps having to is structural, not a lapse of discipline:

* `tools/transpile_coverage.sh` measures **acceptance**. It says so in its own
  header. A module that transpiles into wrong SystemVerilog counts as covered.
* **Correctness** was measured only where somebody hand-wrote a harness *and* a
  reference model. That is real work per module, so it lags — and the lag is
  invisible, because the coverage number does not move when it grows.
* Stimulus was hand-written vectors, typically 8–12 cycles. Before the sweep only
  **three** DUTs got randomized streams
  (`tests/randomized_sequential_equivalence.rs`), because each needed an independent
  Rust model to compare against.

The load-bearing observation is that **the reference model is not required**.
Simulator vs Verilated-emitted-SV is already a differential oracle: two independent
implementations of the same source. A model is a valuable third opinion — it catches
the case where both sides are wrong the same way — but it is not what makes the
comparison meaningful. Dropping the model requirement is what makes the sweep
mechanical, and mechanical is what makes it complete.

### What this would have caught, as evidence rather than claim

| Defect | Caught? | How |
|---|---|---|
| Memory result → plain `Out` in an extracted module, one cycle late (2026-08-25) | yes | sim vs SV, any stimulus |
| `CASEINCOMPLETE` on an extracted comb `case` (2026-08-25) | yes | Verilator `-Wall` build |
| `WIDTHTRUNC` on a literal memory address (2026-08-25; FIXED — addresses carry an explicit width cast, `4'(64'd0)`) | yes | Verilator `-Wall` build |
| `uart/rx` wrong twice after it started transpiling | yes | sim vs SV |
| D1 pre-tick alignment instances | yes | sim vs SV |
| `RegOut` sequential forwarding | yes | the shape exists and sweeps: `tests/fixtures/regout_forwarding_dut.rs` (write_then_assign / assign_then_write) |

### What it would NOT catch, and why the existing checks stay

* A divergence that needs a **specific long input sequence** (a deep FSM state
  reached only by an exact pattern). Random stimulus is weak here; hand vectors and
  the golden traces stay.
* A **shared misunderstanding** — both sides wrong the same way. Only an independent
  reference catches that, which is what `examples/basejump/` and the `REFERENCE`
  table are for (`ANCHORING_A_MODULE.md`). The anchoring is not replaced by this and
  must not be traded against it.

---

## 2. The corpus, measured

143 `#[hardware]` modules across `examples/` and `tests/fixtures/`, counted
2026-09-01 from the generated manifest (`COVERED` in `$OUT_DIR/corpus_generated.rs`)
cross-checked against a scan of the attributes. The mode split is by `#[hardware(…)]`
attribute; the rest by signature.

| | count |
|---|---|
| sequential | 121 |
| combinational | 19 |
| synchronizer | 2 |
| structural | 1 (`two_domain_top`, the only module with more than one clock port) |
| generic (const-generic params) | 10 instances of 6 names — all swept, at the widths in `build.rs`'s `PARAMS` |
| uses `RegOut` | 40 |
| array-typed ports | 2 (`mux`, `bsg_mux_one_hot`) — both swept |
| receives a `Memory<…>` parameter | 1 (`rv32i_cpu_pipelined`) — `SKIP`, anchored outside the sweep |

Port payload types are near-uniform, which is what makes generation tractable:
`Bits<N>` and `Logic` account for all but the two array ports, and there are no
`Vec` ports left (the CPUs' `Vec<Bits<32>>` ports went with the subset restriction
on 2026-08-26; both CPUs transpile — `rv32i_cpu_transpilable` sweeps under a `RESET`
row, `rv32i_cpu_pipelined` is gated by `tests/rv32i_pipelined_verilator.rs`). Port
*heads* are only `In` / `Out` / `RegOut` / `Clock`, plus a `Memory` parameter, which
is not a port (`build.rs`'s `Kind::Memory`). A width written as a file-scope `const`
or a const-generic parameter is still `Bits<N>`; the generator handles both.

### The backlog it closed (historical — phase 1's starting point, 2026-08-25)

Concrete example modules with **no** sim ≡ transpiled-SV check at the time: 10 —
`det_010_awaits`, `event_sink`, `event_source`, `fast_counter`, `flag_sync`,
`slow_consumer`, `two_domain_top`, `rv32i_cpu_pipelined`, `uart_tx`, `uart_rx`.
Of those, the last three did not transpile then and `two_domain_top` is structural,
so **six** were transpilable-but-unchecked. Fixture modules unchecked: **five**
(`branch_merge`, `branch_merge_explicit`, `match_tick`, `match_tick_explicit`,
`sync_2ff_concrete`).

Note the examples were *not* unverified — they are checked sim ≡ **hand-written**
Verilog, which is the stronger anchor. What was missing is sim ≡ **emitted** SV,
which is the direction transpiler bugs live in.

Eleven modules is a small backlog; it is not the point. The point is that the
backlog **regrows with every module added**, silently, and this removes that.

---

## 3. Design, as built

### 3.1 Generation, not invocation

`build.rs` (with `syn` and `quote` as build-dependencies) walks `examples/` and
`tests/fixtures/` (`collect_rs`, which prunes `old/` directories), parses every
`#[hardware]` signature (`Module::from_item`, `classify`), and emits one `#[test]`
per module into `$OUT_DIR/corpus_generated.rs`, which `tests/corpus_generated.rs`
`include!`s. `cargo:rerun-if-changed` on both directories and on `build.rs` itself
keeps it honest.

Each generated case wraps the source file in its own module and `include!`s it, so
per-file `struct MainClk` / imports do not collide. The wrapper name is
path-derived (`wrapper_name`: `examples/cdc/flag_crossing.rs` →
`ex_cdc_flag_crossing`), not stem-derived, because two different modules can share
a name (`fast_counter` exists in both `two_domain_counter.rs` and
`two_domain_hierarchy.rs`):

```rust
#[allow(unused_imports, dead_code)]
mod ex_basejump_lfsr {
    include!("…/examples/basejump/lfsr.rs");   // its own `use`s come with it
    #[test] fn lfsr_differential() { … }
}
```

The example files need *no* change for their `fn main` demos: the module-level
`#[allow(unused_imports, dead_code)]` is enough. What a file must not have is a
`//!` header — `include!` cannot carry inner doc comments — so `emit_file` turns
such a file's modules into `#[ignore]`d cases whose reason says so, rather than
emitting a case that will not compile. Every fixture and example uses `//` headers.

**Why build.rs and not a proc macro:** a proc macro reading files off disk has no
rebuild tracking, so a changed module would silently keep its stale generated test.
That is this repo's recurring bug class, not a hypothetical.

### 3.2 Stimulus

A seeded `Rng` (SplitMix64, `tests/common/mod.rs`) produces a fresh random value for
every `In` port on every cycle, by payload type through the `RandStim` trait: `Logic`
a random bit, `Bits<N>` N random bits, `[T; N]` element-wise. The seed is a per-module
FNV-1a hash of the module name (`build.rs`'s `seed_of`), so two modules do not walk
the same bit pattern and a failure is reproducible from the name alone. `SEQ_CYCLES`
(200) ticks per sequential module; `COMB_VECTORS` (64) vectors per combinational
module, settled with `poll_tasks`. Outputs are wired with `wire` or `registered_wire`
according to whether the port is `Out` or `RegOut`, and read after every tick. A
module with a `RESET` row has that port driven to its asserted value on cycle 0 and
randomly thereafter, so the reset path keeps being exercised rather than visited
once.

### 3.3 Comparison

`EquivalenceTest::differential_only` + `record_differential` (`tests/common/mod.rs`)
record the simulator's outputs as both actual and expected, so the trace comparison
is trivially satisfied and the Verilator leg carries the weight. The mode is
**explicit** and mutually exclusive with `record` by assertion: passing the sim's own
values as the "reference" is right here and a silent disaster anywhere else, so it
cannot be reached by accident. A module with a `REFERENCE` row additionally gets
`with_hand_written_reference`, which replays the same stimulus against the
independent Verilog (`ANCHORING_A_MODULE.md`).

### 3.4 The tables

There is no separate constraint file and no stimulus masking; a module whose
random inputs would hit a precondition is either fine under the harness's own rules
(a guarded 32-bit address that is simply never in range) or has a `SKIP` row saying
why. What cannot be inferred from a signature lives in four reviewed tables at the
top of `build.rs`, each row a sentence with an author:

| table | carries | verified at build time |
|---|---|---|
| `SKIP` | modules the sweep must not run, with the reason; keys are a bare name or `<wrapper>::<module>` for one copy of a duplicated name | every key names a module in the corpus (a dead row fails the build, since 2026-09-01) |
| `PARAMS` | the monomorphization for a generic module — the same widths its hand-written test uses | a generic module with no complete row is ignored with a reason naming the table |
| `RESET` | `(module, port, active_low)` for a design whose state is X until reset | — |
| `REFERENCE` | an independent Verilog reference, by module | module exists, file exists, declares `module <name>`, is not `// @generated` |

The generator adds its own ignore reasons where no table applies (`skip_reason`): a
`Memory` parameter (the sweep would have to invent its size and contents), more than
one clock port (the ratio is a design decision), a payload `RandStim` cannot produce,
or a `//!` header.

One limit is worth knowing. `emit_test(…, body = false)` replaces an ignored case's
body with a `panic!`, so `cargo test -- --ignored` **cannot** tell you a `SKIP` reason
has gone stale — it panics either way. Two rows sat in the table after their
lowerings were fixed (found 2026-09-01). Re-transpile a module before trusting a
reason that names emitted text.

### 3.5 The guards (this is the part that matters)

Two layers. In the test binary, `every_corpus_module_has_a_generated_case`
re-scans both directories with the same pruning as `collect_rs` and asserts the
`COVERED` manifest matches in **both** directions — a module with no case, or a case
for a module that no longer exists, fails naming it.

In `tools/regression.sh`, **G-D** requires that guard's `ok` line in the test log,
counts the `_differential ... ok` and `... ignored` lines, fails if none ran, and —
because a case without Verilator compares the simulator to itself — prints
`G-D NOT SATISFIED` and forces `PARTIAL` when `verilator` is not on the path. **G-E**
prints the anchoring ledger `build.rs` writes to the build log (`anchoring: N
module(s) checked against an independent reference, M cross-checked against the
transpiler only`); it does not fail.

This repo already had G-A, G-B and G-C because "the check silently did not run" is
its most-repeated bug class. A sweep without G-D would be one more instance of it.

---

## 4. Phasing (all done 2026-08-25)

| Phase | Work | Outcome |
|---|---|---|
| **1** ✔ | `differential_only` + a hand-written `tests/corpus_equivalence.rs` covering the 11-module backlog with random stimulus | found two defects on the first run (§0) |
| **2** ✔ | `build.rs` generator over `tests/fixtures/` | all generated cases green; the hand-written fixture cases deleted (§0) |
| **3** ✔ | extend to `examples/`, add `SKIP` and G-D | G-D in the driver; phase 1's hand-written file deleted, `tests/corpus_generated.rs` is the includer |
| **4** ✔ | generic modules via `with_params` monomorphization (`PARAMS`); array ports; reset sequencing (`RESET`) | the last generator-shaped skip gone (§0) |

`REFERENCE` and G-E were added on 2026-08-26 (`ANCHORING_A_MODULE.md`).

## 5. Costs and risks

* **Runtime.** One Verilator build per module, a few seconds each, absorbed by
  `cargo test`'s thread pool. It belongs in the full driver run, not `--quick`;
  `tools/regression.sh` prints the elapsed time of every run.
* **Noise from undefined inputs.** A module that needs more than the harness's
  rules is usually a finding about the module's contract rather than about the
  sweep; `RESET` and `SKIP` are the two places that finding is recorded.
* **False confidence.** A green sweep says the two implementations agree, not that
  either is right. §1 states what it cannot see; the independent anchoring stays.
* **Generated-test staleness.** Handled by `rerun-if-changed`, the two-directional
  guard, and the build-time validation of `SKIP` and `REFERENCE` keys. A stale
  *reason* is the residual (§3.4).

## 6. Relationship to the bigger refactor

This was deliberately sequenced **before** unifying the timing model (one
authoritative register inference in `copper-analysis`, consumed by both front-ends
— `SYNCHRONOUS_SEMANTICS.md`). That refactor touches the lowering's spine, and the
standing rule for migrations here is to build against a differential oracle and
keep the old path rather than debug it afterwards. This *is* that oracle, and it was.
