# Array-typed ports — the ABI decision

**Status:** DECIDED AND LANDED 2026-08-24 (`2618426`) — option B (§4B below,
packed 2-D `[ELS-1:0][WIDTH-1:0]` with direct element select). `copper-transpile
examples/combinational/mux.rs` emits `input logic [ELS_P-1:0][WIDTH_P-1:0] data_i`
and `data_o = data_i[sel_i]`; `bsg_mux_one_hot` emits the same shape. Positive
coverage is `copper-codegen/tests/array_ports.rs`, and both modules run in the
corpus differential sweep. §7 records what landed; the rest of this document is
the option analysis that led to B, kept as the rationale, in the tense it was
written.

**Was blocking (closed):** cause D-b — `examples/combinational/mux.rs` and
`examples/basejump/bsg_mux_one_hot.rs`, at the time the last two modules in the
corpus that failed for a shared reason.
**Measured:** 2026-08-24, against Verilator 5.044.

```
mux              error: 26:12: cannot resolve type '[Bits<WIDTH_P>;ELS_P]' to a hardware type
bsg_mux_one_hot  error: 22:12: cannot resolve type '[Bits<WIDTH>;ELS]' to a hardware type
```

Both simulated at the time and both checked against independent BaseJump Verilog.
What they lacked was any transpiled path at all, so the *generated* SystemVerilog
for the mux family had never existed, let alone been verified.

---

## 1. Why this needed a decision rather than a default

Every other gap closed at the time was a missing lowering with one obviously
correct answer. This one is a **choice about the shape of the emitted module
interface**, visible to anything that instantiates a Copper module, and changing
it later is a breaking change to generated hardware rather than an internal
refactor.

---

## 2. What the independent references already do

Both BaseJump modules this corpus is anchored to declare the port as a **packed
2-D vector**:

```systemverilog
// examples/combinational/sv/mux.sv
input  [els_p-1:0][width_p-1:0] data_i

// examples/basejump/sv/bsg_mux_one_hot.sv
input  [els_p-1:0][width_p-1:0] data_i
```

That is the idiom the reference hardware uses, and Copper's stated discipline is
to settle "which behaviour is correct" against an independent reference rather
than by argument. It is evidence, not proof — the references answer *what is
idiomatic*, not *what is cheapest to build*.

## 3. What the simulator and the harness already do

The simulator does not care: `wire::<[Bits<W>; ELS], D>` is a Rust array and
never becomes bits. The ABI is purely an emission concern.

The **testbench harness does care**, and it had already picked a convention.
`examples/combinational/mux.rs` flattens for the Verilator port (it still does):

```rust
// Flatten [Bits<WIDTH>; ELS] → Vec<Logic> (element 0 at LSBs) for the Verilator port.
```

and `verification.rs::logic_vec_to_int` packs index 0 into bit 0 and drives the
port with a single scalar assignment, `top->data_i = <u64>`.

**Measured, and this is the load-bearing fact:** Verilator gives packed 2-D and
flat 1-D the *identical* C++ interface.

```systemverilog
input logic [ELS-1:0][W-1:0] packed2d,   // →  VL_IN(&packed2d,31,0);
input logic [ELS*W-1:0]      packed1d,   // →  VL_IN(&packed1d,31,0);
```

So options A and B below are **bit-identical and harness-identical**. The
existing testbench generator needs no change for either, and the flattening
convention the mux example already uses stays correct. They differ only in how
the emitted SystemVerilog *source* declares and indexes the port.

This also rules out a third shape: an **unpacked** array (`input [W-1:0] d [ELS-1:0]`)
appears in C++ as a member array, so `top->d = value` would not compile. Adopting
it means rewriting the testbench generator. Nothing recommends it — the references
do not use it, and unpacked ports cannot be driven as a whole vector.

**Harness ceiling, unrelated to arrays but relevant to sizing:** `logic_vec_to_int`
returns a `u64`, so any port wider than 64 bits is already undrivable by the
generated testbench. `mux` (8×4 = 32) and `bsg_mux_one_hot` (4×3 = 12) fit. A
wider array port would hit that ceiling regardless of which option is chosen.

---

## 4. The options

### A — flat packed vector

```systemverilog
input logic [ELS_P*WIDTH_P-1:0] data_i
assign data_o = data_i[sel_i*WIDTH_P +: WIDTH_P];
```

*Body:* element indexing becomes an indexed part-select.
*IR:* `CHIRType` needs **no** new variant — the port resolves to a flat
`UInt { width: ELS*W }` and the array exists only in the front end.
*Cost:* `Width` must express a **product**. Today it is
`Concrete(usize) | Param(String)` with no arithmetic, and both blocked modules
are symbolic (`mux` is generic over `WIDTH_P`/`ELS_P`; `bsg_mux_one_hot`'s
`WIDTH`/`ELS` are file consts, which now lower to `localparam`s — also symbolic).
A new `Width` variant is ~11 match sites plus every exhaustiveness error it
surfaces.

> There is a shortcut worth naming so nobody reaches for it silently:
> `Width::Param("ELS_P*WIDTH_P".to_string())` renders as `[ELS_P*WIDTH_P-1:0]`,
> which is legal SystemVerilog, and costs zero new variants. It is a **hack** —
> it puts an expression where every other consumer expects an identifier, and
> `Width::Param` names are matched by name elsewhere. Do not adopt it without
> auditing those consumers.

### B — packed 2-D vector  *(matches both references)*

```systemverilog
input logic [ELS_P-1:0][WIDTH_P-1:0] data_i
assign data_o = data_i[sel_i];
```

*Body:* element indexing is a direct index — no arithmetic anywhere.
*IR:* needs a **second dimension on the port**, and the lowering must know an
indexed operand is an array port rather than a bit-vector (a bit-select and an
element-select emit the same syntax but mean different things).
*Cost:* no `Width` arithmetic at all — each dimension is independently `Concrete`
or `Param`, which the existing `Width` already expresses. The emitter needs an
outer-dimension case. The wart is that a value read from an array port has a
shape the IR cannot otherwise name.

### C — N separate scalar ports

```systemverilog
input logic [WIDTH_P-1:0] data_i_0, data_i_1, data_i_2, data_i_3
```

*Fails the generic case outright:* a parameterised `ELS_P` cannot produce a
variable number of ports. It also breaks drop-in interchangeability with the
BaseJump references and changes the trace signal names the harness records.
Listed for completeness; not viable.

---

## 5. The decision

**Option B, packed 2-D.** Three reasons, in order of weight:

1. It is what both independent references declare, so a Copper module stays
   drop-in interchangeable with the hardware it is checked against.
2. It needs **no width arithmetic** — the one piece of new IR machinery option A
   forces, and the piece most likely to leak into unrelated code paths. Both
   blocked modules are symbolic, so option A cannot avoid it.
3. Indexing stays a direct index, so the emitted body reads like the reference
   rather than like a part-select computation.

The cost is honest: B introduces a port shape the type system cannot otherwise
express. A is cheaper *only* if width arithmetic is wanted for its own sake —
and there is a case for that (`Width` already carries a `// M2 (later): Sub`
note), but it should be a deliberate decision about `Width`, not a side effect of
the mux family.

## 6. What "done" was defined as

- Both modules transpile and lint clean under `-Wall`.
- `copper-codegen/tests/unsupported_constructs.rs::array_typed_port_is_unsupported`
  fails loudly (it was written to demand promotion, not relaxation) and is
  replaced by positive coverage.
- Each module gains sim ≡ transpiled-SV on top of its existing sim ≡ BaseJump
  check, `include!`ing the example rather than copying it — the pattern used by
  `sipo_block`, `bsg_encode_one_hot` and `bsg_counter_up_down`.
- The one-hot module additionally exercises **constant** element indexing
  (`d[i]` over a loop) while `mux` exercises **dynamic** indexing (`d[sel]`);
  both need coverage, since only the second involves a run-time index.

## 7. What landed (2026-08-24, `2618426`)

Every item in §6, and where each lives:

- **IR.** `CHIRType::Array { elem, len }` in `copper-core/src/chir.rs`; the VLIR
  port carries an `outer_dim` (`copper-core/src/vlir.rs`), as does a
  `VLIRStmt::WireAssign` for a local holding an array read, so the wire is
  declared `[ELS-1:0][W-1:0]` rather than at the element width.
- **Emission.** `copper-codegen/src/emit.rs` keeps `range_str` one-dimensional
  and prepends the outer range when a port or wire has an `outer_dim` — outer
  first, both packed. Element indexing is emitted as a direct index.
- **Positive coverage.** `copper-codegen/tests/array_ports.rs`:
  `an_array_port_declares_both_packed_dimensions`,
  `an_array_port_with_symbolic_dimensions_needs_no_width_arithmetic` (the case
  that decided the ABI), `indexing_an_array_port_is_a_direct_element_select`,
  `a_local_holding_an_array_keeps_both_dimensions`, and
  `a_nested_array_port_is_refused` (`[[Bits<4>; 2]; 3]` stays a clean refusal).
  `array_typed_port_is_unsupported` is gone.
- **Behavioural coverage.** `tests/mux_equivalence.rs` and
  `tests/bsg_mux_one_hot_equivalence.rs` `include!` the examples and check
  sim ≡ transpiled SV under Verilator on top of the examples' own sim ≡ BaseJump
  checks; and both modules are in the corpus differential sweep (`mux` at the
  widths in `build.rs`'s `PARAMS`, `bsg_mux_one_hot` at its file-scope consts),
  driven by `RandStim for [T; N]` in `tests/common/mod.rs`, whose element-major
  layout is the flattening the harness already used (§3).
