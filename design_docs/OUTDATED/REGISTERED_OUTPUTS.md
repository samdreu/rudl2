# Explicit Registered Outputs (`RegOut`)

**Status:** ADOPTED for the macro + simulator (2026-07-25); transpiler lowering
pending. An earlier revision of this note marked `RegOut` "superseded" under the
interim atomic model (which registered *every* held output by construction). The
dual-convention experiment (EXECUTOR_CONVENTION_EXPERIMENT.md) reversed that: under
the post-edge convention no single global scheduling choice is correct for both
write-before-tick and write-after-tick outputs — the register/combinational
distinction is irreducible — so `RegOut` is the **minimal, provably-necessary**
explicit marker, not a redundant one.

Current state:
- `RegOut<T,D>` is a first-class output port type (`copper-core/src/port.rs`).
- The `#[hardware]` macro accepts it (`copper-macros`); `mac_fsm`'s held output
  uses it (`examples/sequential/pipeline_mac.rs`).
- **Not yet done:** the transpiler does not lower `RegOut` to a registered output
  (`always_ff … out <= v`). Until it does, `mac_fsm_equivalence` stays ignored
  (the sim registers via `RegOut`, the transpiler still emits combinational). This
  is the remaining transpiler-alignment step. Original design below.

## Summary

Add an explicit registered-output port type, `RegOut<T, D>`, alongside the
existing combinational `Out<T, D>`. The programmer chooses register vs
combinational **explicitly** — as in every mainstream HDL — instead of Copper
inferring it from control flow. This removes the sim-vs-transpiler divergence on
held outputs at its source.

- `Out<T, D>` — **combinational** output. Must be driven on every path through the
  loop body (an undriven path is a latch → a hard error, "use `RegOut`").
- `RegOut<T, D>` — **registered** output (an enabled flip-flop). May be written
  conditionally / held; commits at the clock edge (+1 cycle latency).

## Why (the investigation, in one page)

The multi-tick reconciliation fixed **input** timing (reads sample at the
registering edge; see EXECUTION_MODEL_RECONCILIATION.md). Outputs turned out to be
a different, deeper problem:

1. **Output register-vs-combinational is not physics-forced** (unlike reads).
   Both are valid hardware; they differ by one cycle. A held output *must* be a
   register (a conditional combinational drive is a **latch** — proven: a
   conditional `out.write(a+b)` Verilates to `%Warning-LATCH`). An
   always-driven output is combinational.
2. **The distinction is irreducible.** Empirically, the two output shapes need
   *opposite* treatment:
   - post-tick Moore output (`out = state`): combinational — registering it adds a
     wrong cycle (`[0,1,2,…]` instead of `[1,2,3,…]`).
   - pre-tick held output (`mac_fsm`): registered — leaving it combinational reads
     one cycle early (cycle 1 instead of cycle 2).
   No single executor rule or convention fixes both. A prototype "resolve ticks at
   the pre-edge" executor confirmed this — it fixed `mac_fsm` but broke `det_010`.
3. **Every real HDL makes the distinction explicit:** Verilog `<=`/`always_ff` vs
   `=`/`always_comb`; Chisel `Reg` vs wire; Esterel's synchronous instants with
   explicit state. None infer it. (Sources in EXECUTION_MODEL_RECONCILIATION.md.)
4. **Copper *infers* it**, and the sim and transpiler infer *differently* — the sim
   treats every output as combinational/immediate, the transpiler infers per output
   (`conditional_output_ports`). They agree where the inferences coincide and
   diverge on held outputs (`mac_fsm`: sim cycle 1, transpiler cycle 2). That
   divergence is every output bug we found.

Conclusion: stop inferring; make it explicit. `RegOut` is that marker.

## Semantics

| | `Out<T, D>` | `RegOut<T, D>` |
|---|---|---|
| hardware | combinational (`assign` / `always_comb`) | registered (`always_ff … out <= v`) |
| latency | same cycle | +1 cycle (commits at the edge) |
| may be conditional? | **no** — undriven path is a latch → error | yes — held between writes |
| Verilog analogue | `=` in `always_comb` | `<=` in `always_ff` |

## Sim behavior

`RegOut` already implements the enabled-flip-flop model (`copper-core/src/port.rs`,
`registered_wire` + `RegOutShared: ClockEdgeListener`): `write` buffers a value;
`on_posedge` (fired by `clk.advance()`, between the pre- and post-edge settle)
commits it to the observed cell, else holds. Validated: a `mac_fsm` whose output is
a `RegOut` produces the hardware-accurate `[0,0,10,10,10,…]` (cycle 2). `Out` is
unchanged (immediate write).

## Transpiler behavior

Drive register-vs-combinational from the **port type**, not from
`conditional_output_ports`:

- `RegOut<T,D>` param → the output's drives move to `always_ff` as guarded
  non-blocking assigns (`if (guard) out <= v;`), holding otherwise. This is what
  `vlir_lower::{conditional_output_ports, split_output_regs}` already does — but
  keyed on the explicit type instead of on path analysis.
- `Out<T,D>` param → combinational (`always_comb` / `assign`).
- **Latch check:** an `Out` port not driven on all paths is a hard error with a
  fix-it ("this output is held on some paths; declare it `RegOut`"). This replaces
  silent implicit-hold-register inference with an explicit diagnostic.

## Migration

Modules with a genuinely-held (conditionally-written) output switch `Out` →
`RegOut`. Known case: `mac_fsm` (`out` written only in the `Out` arm). Combinational
outputs (`det_010`, `counter`, `probe`, `mac_pipeline` if kept combinational,
unconditional writers) stay `Out`. The equivalence harness / tests create `RegOut`
outputs via `registered_wire(&clk, init)` for `RegOut` ports (the caller knows the
signature).

## Implementation plan (incremental)

1. **Macro** recognizes `RegOut` params (parallels `Out`): include in
   hardware-signature detection, `In`/`Out` handling, and any port bookkeeping.
2. **Transpiler** parses `RegOut` in the port list; VLIR emits registered output
   for `RegOut` ports (reuse `split_output_regs`, keyed on type); `Out` stays
   combinational. Add the conditional-`Out` latch error.
3. **Migrate** `mac_fsm` to `RegOut`; un-`#[ignore]` `mac_fsm_equivalence`. Confirm
   sim == transpiler at cycle 2.
4. **Decide** (follow-up) whether to *remove* the `conditional_output_ports`
   inference entirely (full explicit-only) or keep it as a compatibility fallback.
   Deferred until the explicit path is proven end-to-end.

## Open questions

- **API form:** a distinct type `RegOut<T,D>` (chosen — mirrors `Out`, and the
  primitive already exists) vs a method (`out.write_reg`) vs an attribute. Type is
  cleanest and makes the signature self-documenting.
- **`if_tick`/control-extraction outputs:** the extracted single-tick FSM writes
  outputs conditionally (per `pc` state). Those become `RegOut` (registered) under
  this model — which also resolves the `if_tick` multi-write collapse, since a
  registered output commits once per edge. To confirm during implementation.
- **Reset value:** `RegOut` initial value comes from `registered_wire(init)`; the
  transpiler needs a matching reset/init. Currently registers have no explicit
  reset — revisit if it matters for equivalence.

## Relationship to the reconciliation

This is the resolution of the **output** half of
EXECUTION_MODEL_RECONCILIATION.md. The **read** half is already fixed
(`synced_read`). With explicit `RegOut`, sim and transpiler agree on outputs by
construction, and the remaining documented sim gaps (mid-phase reads `accum_2`;
`if_tick` collapse) are expected to be subsumed (the collapse) or remain a separate
read-side item (mid-phase reads).
