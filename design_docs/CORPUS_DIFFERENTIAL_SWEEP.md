# Corpus differential sweep — scope

**Status:** phases 1–4 BUILT and green. `build.rs` generates a case for every
`#[hardware]` module in `tests/fixtures/` and `examples/` — **95 running, 12
ignored-with-reason** as of 2026-08-25 — and `tools/regression.sh`'s **G-D** asserts
the sweep covered the corpus and ran. Every skip is a recorded transpiler cause, a
structural module with no simulatable body, a deliberate divergence witness, or a
documented startup transient. **None is "the generator cannot do this."**

The counts move as the corpus does; `G-D` prints the current ones on every full run,
which is the number to trust over any written here.

## 0. What phase 1 found, on its first run

Two defects, in modules that had been in the tree for weeks:

* **A measured sim ≠ synth divergence.** `branch_merge_explicit` and its async twin
  transpile to byte-identical SystemVerilog; the twin agrees with it for 200 random
  cycles and it leads by one. D1's constant-write exemption is unsound for a
  conditionally-written `Out` — `PRETICK_ALIGNMENT_GUARDRAIL.md` 5.5, pinned in
  `sequential_forwarding_divergence.rs`. The simulator disagrees with *itself*
  depending on how the same hardware is spelled, which is a witness for the timing
  model having two implementations (§6).
* **Invalid emitted SystemVerilog.** `event_source` has a port named `event`, a
  SystemVerilog keyword missing from the legalizer's reserved list, so what it
  transpiles to would not parse. Fixed; both keyword lists are now complete.

Neither was reachable from the existing tests: that example is checked against
hand-written Verilog, and nothing had ever Verilated what it transpiles to.

**A lesson for the generator (phase 2):** the testbench addresses the Verilated
model by the **emitted** port name, not the Rust one, so the generator needs the
rust→SV name mapping — a reserved name is renamed (`event` → `event_sig`).

## 0.1 What phase 2 found

**Nothing — and that is the result.** All 59 generated cases passed on the first
run: every memory fixture, the read-timing and `RegOut`-forwarding families, the
control-extraction pairs, `uart_rx`, the ROM and RAM variants. Under 200 cycles of
random stimulus each, the fixture corpus and its emitted SystemVerilog agree.

That is worth having precisely because it was not knowable before. The 11 skips are
all explained in `build.rs`'s `SKIP` table or by being generic, and each is a
`#[ignore]` with its reason rather than an omission.

**The guard is the load-bearing part.** `every_fixture_module_has_a_generated_case`
re-scans `tests/fixtures/` at test time and asserts the generated manifest matches —
so a generator that quietly stops covering something fails loudly instead of
shrinking the sweep. Verified by negative control: dropping one module from the
generator makes it fail naming that module.

Wall clock: **53s** for 59 Verilator builds and runs, inside `cargo test`'s existing
parallelism. That is the whole budget question answered — it belongs in the default
regression path.

**What phase 2 cost, in the end:** one build script, `syn` + `quote` as
build-dependencies, and the deletion of the hand-written fixture cases phase 1 had
written. The generator emits the same wiring, which is what phase 1 was for.

**One sentence.** Every `#[hardware]` module the transpiler accepts should be run in
the simulator against its own emitted SystemVerilog under seeded random stimulus,
automatically, without anybody writing a harness or a reference model for it.

## 0.2 What phase 3 found

**Nothing new, again** — every example module the generator reaches agrees with its
emitted SystemVerilog under 200 random cycles, including `dual_port_ram`, `sipo_block`,
`uart/rx`, both `fast_counter`s, and the BaseJump family. The two defects phase 1
found by hand were the corpus's whole harvest.

Three things the extension needed, all of them consequences of examples being
standalone programs rather than fixtures:

* **Per-file import and domain rules, keyed on content rather than directory.** A
  fixture declares no `use` and no `ClockDomain`, so the wrapper supplies both; an
  example brings its own and the wrapper must supply neither. `impl ClockDomain for X`
  appears in three spellings in the corpus (with and without a space before `{}`, and
  fully qualified), which cost a build failure to discover.
* **Everything the generated body uses is imported under `__` aliases**, so it cannot
  collide with whatever the included file already imported.
* **`copper_codegen::legalized_port_name`, called at run time**, rather than the
  generator reimplementing the rule. Two copies of a naming rule that must agree is
  the drift bug this repo keeps recording, and it is why the function is now public.

One skip was too strict and was relaxed: the BaseJump modules write their widths as
file-scope `const`s (`Bits<WIDTH>`), which is *concrete* — the const comes with the
included file — so four more modules sweep. Array ports (`bsg_mux_one_hot`) stay
skipped: `RandStim` has no array impl and the bit layout the testbench would assume is
the array-port ABI, a decision to make deliberately rather than let a generator guess.

**Wall clock: 81s** for 83 Verilator builds and runs.

## 0.3 What phase 4 found

**A design's undefined region, which is the interesting part.** `shift_register`
initialises its register to `Bits::x()` — deliberately, because an unreset flip-flop
is X in hardware — and the sweep drove it before any reset. The simulator carries X;
Verilator's 2-state model reads 0. They *legitimately* disagree, and comparing
undefined behaviour against undefined behaviour is a test of nothing.

The honest fix was not to skip it and not to paper over it, but to give the design
the reset it requires: a `RESET` table entry drives the reset port asserted on cycle 0
and randomly after that, so the reset path keeps being exercised rather than visited
once. One entry covers the corpus — `Bits::x()` appears in exactly two files, which
are the two copies of that module.

The rest of phase 4 was mechanical and worked first time:

* **Generic modules sweep.** A generic module transpiles to a *parametric*
  SystemVerilog module, so it needs only `-G` widths — the earlier skip reason
  ("`copper-transpile` cannot emit it") was simply wrong. What cannot be inferred is
  *which* widths: parameters are often constrained (`N_LOG == clog2(N)`, asserted
  inside the module), so a guess is a compile error. `PARAMS` records the same
  monomorphization the hand-written test uses, so the sweep and the vector test
  exercise the same shape.
* **Array ports sweep**, via a `RandStim` impl for `[T; N]` whose bit layout is the
  one the hand-written array tests already record (element-major) — the array-port
  ABI, taken from an existing test rather than invented.
* **Type rewriting is token-wise, not textual.** `Bits<N_LOG>` must not be rewritten
  by a rule for `N`, and a substring replace does exactly that.

One skip was added rather than removed: the *example* copy of `ripple_carry_adder`
does not transpile (cause J-b, a tuple-returning helper) while the fixture copy,
written without the helper, does. Same module name, two files, one blocked — which is
why `SKIP` accepts a `<wrapper>::<module>` key.

---

## 1. Why this and not the individual bugs

The repo `TODO` records "acceptance is not correctness" repeatedly, and the reason
it keeps having to is structural, not a lapse of discipline:

* `tools/transpile_coverage.sh` measures **acceptance**. It says so in its own
  header. A module that transpiles into wrong SystemVerilog counts as covered.
* **Correctness** is measured only where somebody hand-wrote a harness *and* a
  reference model. That is real work per module, so it lags — and the lag is
  invisible, because the coverage number does not move when it grows.
* Stimulus is hand-written vectors, typically 8–12 cycles. Only **three** DUTs get
  randomized streams (`tests/randomized_sequential_equivalence.rs`), because each
  needed an independent Rust model to compare against.

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
| `WIDTHTRUNC` on a literal memory address (2026-08-25, still open) | yes | Verilator `-Wall` build |
| `uart/rx` wrong twice after it started transpiling | yes | sim vs SV |
| D1 pre-tick alignment instances | yes | sim vs SV |
| `RegOut` sequential forwarding ([TODO:1283](../TODO)) | likely | needs the write-after-update shape to exist in a swept module |

### What it would NOT catch, and why the existing checks stay

* A divergence that needs a **specific long input sequence** (a deep FSM state
  reached only by an exact pattern). Random stimulus is weak here; hand vectors and
  the golden traces stay.
* A **shared misunderstanding** — both sides wrong the same way. Only an independent
  reference catches that, which is exactly what `examples/basejump/` is for. The
  BaseJump anchoring is not replaced by this and must not be traded against it.

---

## 2. The corpus, measured

107 `#[hardware]` modules across `examples/` and `tests/fixtures/`, as of 2026-08-25:

| | count |
|---|---|
| sequential | 85 |
| combinational | 19 |
| synchronizer / structural | 3 |
| generic (const-generic params) | 10 — all swept, at the widths in `build.rs`'s `PARAMS` |
| more than one clock PORT | 1 (`two_domain_top`, structural) |
| uses `RegOut` | 37 |
| array-typed ports | 2 — one swept, one skipped (`bsg_mux_one_hot`) |

Port payload types are near-uniform, which is what makes generation tractable:
`Bits<N>` and `Logic` account for nearly all of them, with 2 array ports and 2
`Vec<Bits<N>>` (the CPUs, which do not transpile). Port *heads* are only `In` / `Out`
/ `RegOut` / `Clock` — there is no fifth shape to discover. A width written as a
file-scope `const` or a const-generic parameter is still `Bits<N>`; the generator
handles both.

### The backlog it closed (historical — phase 1's starting point)

Concrete example modules with **no** sim ≡ transpiled-SV check today: 10 —
`det_010_awaits`, `event_sink`, `event_source`, `fast_counter`, `flag_sync`,
`slow_consumer`, `two_domain_top`, `rv32i_cpu_pipelined`, `uart_tx`, `uart_rx`.
Of those, the last three do not transpile and `two_domain_top` is structural, so
**six** are transpilable-but-unchecked. Fixture modules unchecked: **five**
(`branch_merge`, `branch_merge_explicit`, `match_tick`, `match_tick_explicit`,
`sync_2ff_concrete`).

Note the examples are *not* unverified — they are checked sim ≡ **hand-written**
Verilog, which is the stronger anchor. What is missing is sim ≡ **emitted** SV,
which is the direction transpiler bugs live in.

Eleven modules is a small backlog; it is not the point. The point is that the
backlog **regrows with every module added**, silently, and this removes that.

---

## 3. Design

### 3.1 Generation, not invocation

A `build.rs` (with `syn` as a build-dependency) scans `examples/` and
`tests/fixtures/`, parses every `#[hardware]` signature, and emits one `#[test]` per
eligible module into `OUT_DIR`, which a single `tests/corpus_equivalence.rs`
`include!`s. `cargo:rerun-if-changed` on both directories keeps it honest.

Each generated case wraps the source file in its own module and `include!`s it, so
per-file `struct MainClk` / imports do not collide:

```rust
mod dut_examples_basejump_lfsr {
    include!("…/examples/basejump/lfsr.rs");   // its own `use`s come with it
    #[test] fn lfsr_differential() { … }
}
```

**Measured in phase 1, correcting this section's first draft.** The example files
need *no* change for their `fn main` demos: a module-level
`#[allow(unused_imports, dead_code)]` on the wrapper is enough, and the main compiles
unused. What does need a change is the file's **header comment style** — `include!`
cannot carry `//!` inner doc comments (they may not follow an item, and a macro
expansion cannot produce them), so an `include!`-able file must use `//`. Every
fixture and every previously-`include!`d example already did; the two CDC examples
were converted. The generator should refuse a file with a `//!` header and say why,
rather than emitting a case that will not compile.

**Why build.rs and not a proc macro:** a proc macro reading files off disk has no
rebuild tracking, so a changed module would silently keep its stale generated test.
That is this repo's recurring bug class, not a hypothetical.

### 3.2 Stimulus

Seeded `Rng` (already in `tests/common/mod.rs`, SplitMix64) → per-cycle random value
for every `In` port, by payload type: `Logic` a random bit, `Bits<N>` N random bits,
`[Bits<W>; N]` element-wise. 200 cycles per module, seed fixed and printed on
failure. Outputs are wired with `wire` or `registered_wire` according to whether the
port is `Out` or `RegOut` — both readable from the signature.

### 3.3 Comparison

Add `EquivalenceTest::differential_only(…)`, which records the simulator's outputs
as both actual and expected. That mode has to be **explicit**: passing the sim's own
values as the "reference" is the right thing here and a silent disaster anywhere
else, so it must not be reachable by accident.

### 3.4 The constraint table

Some modules have input preconditions — a one-hot select (`bsg_mux_one_hot`,
`bsg_encode_one_hot`), an address that must stay inside a memory smaller than its
address width (the out-of-range access **panics** the simulator while SystemVerilog
reads X). Random stimulus will hit them.

A checked-in table (`tests/corpus_constraints.toml`) carries per-module masks, ranges
and skips **with a reason string**, rather than annotating the hardware source — the
source should not carry test scaffolding, and a reason in a reviewed file is what
stops a skip from becoming permanent by accident. Expect 3–6 entries.

### 3.5 The guard (this is the part that matters)

`tools/regression.sh` gains **G-D**: every module the transpiler accepts either has a
generated differential case that **ran**, or an entry in the constraint table with a
reason. Both counts are printed on every run.

This repo already has G-A, G-B and G-C because "the check silently did not run" is
its most-repeated bug class. A sweep without G-D would be one more instance of it.

---

## 4. Phasing

| Phase | Work | Value |
|---|---|---|
| **1** ✔ | `differential_only` + a hand-written `tests/corpus_equivalence.rs` covering the 11-module backlog with random stimulus | DONE 2026-08-25 — found two defects on the first run (§0) |
| **2** ✔ | `build.rs` generator over `tests/fixtures/` (70 modules, uniform and already `include!`-friendly) | DONE 2026-08-25 — 59 generated + 11 ignored-with-reason, all green in 53s (§0.1) |
| **3** ✔ | extend to `examples/`, add the constraint table and G-D | DONE 2026-08-25 — 83 cases, G-D in the driver, phase 1's hand-written file deleted (§0.2) |
| **4** ✔ | generic modules via `with_params` monomorphization; array ports; reset sequencing | DONE 2026-08-25 — 10 more modules, and the last generator-shaped skip is gone (§0.3) |

Rough cost: phase 1 half a day, phase 2 a day, phase 3 a day plus constraint triage,
G-D a couple of hours. **~3 focused days** to the point where the gap cannot regrow.

## 5. Costs and risks

* **Runtime.** One Verilator build per module, ~3s measured, ~90 modules ≈ 4–5
  minutes wall clock before parallelism; `cargo test` threads absorb most of it. It
  belongs in the full driver run, not `--quick`. The full run is 364s today.
* **Noise from undefined inputs.** Mitigated by the constraint table; if a module
  needs more than a mask, that is usually a finding about the module's contract
  rather than about the sweep.
* **False confidence.** A green sweep says the two implementations agree, not that
  either is right. §1 states what it cannot see; the BaseJump anchoring stays.
* **Generated-test staleness.** Handled by `rerun-if-changed` + G-D's count assertion.

## 6. Relationship to the bigger refactor

This is deliberately sequenced **before** unifying the timing model (one authoritative
schedule in `copper-analysis`, consumed by both front-ends, replacing
`shir_lower::split_at_ticks`). That refactor touches the lowering's spine, and the
standing rule for migrations here is to build against a differential oracle and keep
the old path rather than debug it afterwards. This *is* that oracle.
