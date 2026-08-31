# Anchoring a module to an independent Verilog reference

The corpus sweep's ordinary check is **simulator vs the SystemVerilog the
transpiler emitted**. Those are two implementations of *one source*, so agreement
proves consistency and nothing more — a misconception shared between the executor
and the lowering is invisible to it, and this repo has shipped exactly that kind of
bug before (`as i32` dropped: the emitted SV lints perfectly clean and computes the
wrong number, and sim and transpiler agreed all the way down).

Anchoring adds a **third implementation nobody derived from the other two**. A
module with a reference is checked sim ≡ transpiled *and* sim ≡ reference, so
reference ≡ transpiled transitively, and a failure names which leg broke.

Guard **G-E** in `tools/regression.sh` prints how many modules are anchored and how
many are not, every run. It does not fail — most of the corpus is unanchored and
blocking on that would help nobody — but the gap stays visible instead of being
rediscovered by an audit.

## Adding one

Three steps.

**1. Write the Copper module** as a fixture in `tests/fixtures/<topic>_dut.rs`. No
`use` statements and no `struct MainClk` — `build.rs`'s wrapper supplies both. Any
`#[hardware]` module in that directory is swept automatically; there is no harness
to write.

**2. Write the Verilog** in `tests/fixtures/reference_sv/<module>.sv`.

**3. Add one row** to `REFERENCE` in `build.rs`:

```rust
const REFERENCE: &[(&str, &str)] = &[
    ("ram_read_first", "tests/fixtures/reference_sv/ram_read_first.sv"),
];
```

`build.rs` validates the row at build time — the module must exist in the corpus,
the file must exist, and it must declare a module of that name. A mistyped module
name would otherwise anchor nothing while every test kept passing, which is the
failure mode the whole guard family exists to prevent.

## The contract the reference has to meet

| requirement | why |
|---|---|
| SV module named **exactly** the Copper module | Verilator runs `--top-module <name>` |
| clock port named **`clk`** | the generated testbench drives `top->clk` literally |
| port names match the Copper module's **emitted** names | the testbench addresses them by name; a name colliding with an SV/C++ keyword is legalized (`event` → `event_sig`) via `copper_codegen::legalized_port_name` |
| must pass `verilator -Wall -Wno-DECLFILENAME` | warnings are **fatal** in `verification.rs`; a WIDTHTRUNC or UNUSEDSIGNAL fails the test |
| same parameter names if the module is generic | `with_params` is mirrored onto the reference leg, so it Verilates at the same widths |
| must not be the transpiler's own output | a reference derived from the transpiler cannot anchor the transpiler; `build.rs` rejects a file carrying the `// @generated` marker |

**A combinational reference still needs `clk`** — the harness always drives one —
and an *unused* input is UNUSEDSIGNAL under `-Wall`, which is fatal. Consume it with
an empty `always_ff @(posedge clk) begin end`, which is exactly what the transpiler
emits for the same reason. `reference_sv/bit_not_bits.sv` shows it.

Two things that are *not* required, because both sides already agree on them:

- **No `initial` block for memories.** Verilator zero-fills by default
  (`--x-initial 0`) and the transpiler emits no `initial` either, so the two start
  from the same state. A *preloaded* memory is a different fixture.
- **No reset**, unless the design genuinely has X state before one — in which case
  add a row to `RESET` in `build.rs` naming the reset port and whether it is
  active-low.

## Provenance belongs in the file

State where the reference came from, in its header, because it changes what
agreement means.

- **Third-party** (BaseJump STL and the like) is the strong form: independent of
  everyone here, so agreement is evidence about the semantics themselves. See
  `examples/basejump/sv/bsg_dff_en.sv` for the header format — source URL, licence,
  copyright, and exactly what was adapted (renamed ports, concrete parameter
  defaults) and why. Vendored code must keep its licence header; BaseJump STL is
  Solderpad v0.51 and requires attribution.
- **Hand-written** is the weaker form, and say so. It is evidence that the emitted
  SystemVerilog does what a hardware engineer would write by hand for this
  behaviour — real, and different from "sim and transpiler agree" — but a
  misconception shared between the reference and the Copper module survives it.
  `tests/fixtures/reference_sv/ram_read_first.sv` is the worked example.

Prefer third-party where one exists. For much of what makes Copper Copper —
`RegOut` phases, trailing-segment semantics, `match pc` control extraction — there
is no external counterpart, and hand-written with the weaker claim stated is the
honest option.

## Check that it has teeth

A reference that passes tells you nothing until you have seen it fail. Break it in
the way the module is *about* and confirm the sweep notices:

```bash
# ram_read_first: flip the reference to write-first semantics
#   data <= mem[raddr];  ->  if (we && waddr == raddr) data <= wdata; else ...
cargo test --test corpus_generated ::ram_read_first_differential
```

That mutation should fail on the exact semantic under test (it reports
`FAIL: Cycle 14 data expected 0 got 166`). If it still passes, the reference is not
exercising what you think — usually because the random stimulus never reaches the
case, the way a guarded 32-bit address is essentially never in range for a
1024-word memory.

## Reading a failure

Both legs always run before either is allowed to fail, and the panic names which
combination you are in — because that *is* the diagnosis:

| what failed | what it means |
|---|---|
| transpiled leg only | the transpiler disagrees with the simulator |
| **reference leg only** | the simulator and the transpiler agree with each other and are **both wrong** — the case no amount of differential testing can see, and the reason anchoring exists |
| both | the transpiler disagrees with the simulator, and both disagree with the reference |

A reference-only failure is also how you localise a bug you already know about.
`bit_not_bits` is the worked example: `assign o = ~a` agrees with the simulator and
disagrees with the emitted SystemVerilog, which is what says the `!` lowering is
wrong rather than the executor.

Confirm the leg actually ran, too:

```bash
cargo test --test corpus_generated ::<module>_differential -- --nocapture | grep "INDEPENDENT reference"
```

A silently-skipped check is indistinguishable from a passing one.
