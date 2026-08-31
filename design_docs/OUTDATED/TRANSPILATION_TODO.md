# Copper TODO / Audit Checklist

Working list as of 2026-07-23. Covers the **transpilation pipeline** (by phase) and
the **language frontend** (macro, port model, simulator API).
Companions: [TRANSPILATION_ROADMAP.md](TRANSPILATION_ROADMAP.md) (status + decisions),
[TRANSPILATION_COVERAGE_MAP.md](TRANSPILATION_COVERAGE_MAP.md) (examples → features).

**P0** = emits wrong hardware, fix first. **P1** = gap with a known shape. **P2** = cleanup.

> **2026-07-24 checkpoint.** Landed this session (all committed, 8 equivalence
> tests green): **parameterized equivalence harness** (`.with_params` → `-GN=8`)
> — locking in `shift_register`/`rotate_right`/`priority_encode`/`det_010` as
> transpiled-vs-sim tests, and it **caught two real bugs**; **const-generic
> combinational** greened (`usize`/`from_usize`/`%`/`Resize`, `rotate_right` +
> `priority_encode`); **forward width-inference** and **auto-hoist** removed the
> source-annotation workarounds; **Phase D P0s** fixed; **conditional output →
> implicit-hold register**; **trailing-segment output drop** (a silent miscompile)
> fixed; **`matches!`** support. Two design docs added:
> [EXECUTION_MODEL_RECONCILIATION.md](EXECUTION_MODEL_RECONCILIATION.md) (the
> multi-cycle sim-vs-hardware 1-cycle-offset finding) and
> [CONTROL_EXTRACTION.md](CONTROL_EXTRACTION.md) (the next milestone, designed).
> **Next: implement control extraction, increment A** (Phase C, below).

> **2026-07-23 status.** This branch now sits on `fix/frontend` (macro/CDC/sim
> hardening — barrier removal, synced-reads, `synchronizer` mode). Two bodies of
> work landed on top: **Phase A FIR capture** (#1–#7a — the FIR now losslessly
> represents the whole example set) and the first **Phase B CHIR consumption**
> increments (free-fn + impl-assoc-fn inlining, struct lowering, match-as-value).
> **Design decision (2026-07-23):** `Option`/`?`/`panic!` are **banned** in
> hardware bodies (rejected at the macro) rather than lowered — use explicit
> `{valid, payload}` structs; the `unwrap_or` increment was reverted and the
> `rv32i_cpu` examples were rewritten to the explicit form. See the Phase A audit
> (§ below), the *CHIR consumption progress* note under Phase B, and
> *Cross-cutting → Banned software idioms*.

---

## Phase D — VLIR legalization

- [x] **Resolved (2026-07-24) — P0: Multiply-driven outputs.** Detect-and-reject: a port driven in >1 phase now errors (`MultiplyDrivenOutput`) in `lower_seq` instead of emitting conflicting `assign`s. The phase-mux/registered-output feature is still the deferred conditional-output decision. Was: A port driven in >1 phase becomes multiple
      *unguarded* `assign`s (`assign busy = 1'b1; assign busy = 1'b0;`); phase guards
      are dropped at `vlir_lower.rs:147` (`output_assigns.append`). Verilator does
      **not** catch this, even with `-Wwarn-MULTIDRIVEN`. Detect and reject until the
      registered-output decision lands.
- [x] **Resolved (2026-07-24) — P0: Latch per-phase hole.** Fixed by emission, not the checker: each phase-local combinational temp now gets a `<w> = '0;` default at the top of the merged multi-phase `always_comb` (emit `seq_body`), so no latch is inferred — mac_pipeline is now Verilator-lint-clean. (Cross-phase-read wires are already promoted to registers, so defaulting the remaining phase-local temps is behavior-preserving.) Was: `check_no_latches` runs on each
      phase's statements, but the emitter merges all phases into one `always_comb`
      with `if (phase_r == K)` guards. A signal assigned in only one phase is a latch
      across the whole block. `mac_pipeline` demonstrates it (`product`, `c_s`);
      Verilator warns, our checker passes. Fix: model the merged block.
- [x] **Resolved (2026-07-24) — P1: Phase-local wire defaults.** Implemented as the
      fix for the latch P0 above — emit `<w> = '0;` at the top of the multi-phase
      `always_comb`. This is exactly the "standard practice" fix anticipated here.

## Phase E — Emission

- [ ] **P1 — `ToolchainProfile` is accepted but never consulted.** It's stored in
      `EmitConfig` and read nowhere in `emit.rs`; Verilator/Generic/Yosys all emit
      identically. Either implement the differences (x/z literals, `logic` vs
      `wire`/`reg`) or drop the option until it means something.
- [ ] **P1 — Submodule output port name is hardcoded `.out`** (`emit.rs:199`). SHIR
      carries only the callee's output *wire*, not its port name. Blocks hierarchy (M3).
- [ ] **P2 —** Port-declaration column alignment and optional source-location
      comments, both specified in `EMISSION_DESIGN.md`, are unimplemented (cosmetic).

## Phase C — SHIR

- [ ] **P1 — Pre-existing validation items never implemented.** `EmitWithoutOutput` /
      `OutputWithoutEmit` don't exist in the codebase; the roadmap has listed them as
      "remaining" since before this work started. Same for submodule output-wire
      visibility across phases.
- [x] **Resolved — `pre_edge_barrier` no longer exists; phase extraction was
      never aware of it and doesn't need to be aware of its replacement
      either.** `pre_edge_barrier`/`LoopDeltaInjector` have been deleted
      entirely (see the frontend section below) in favor of a per-read
      freshness check that only *occasionally* suspends, at runtime, exactly
      when a read would otherwise double-consume a value within one
      `tick_clock()` call. Unlike the old barrier, this introduces no new
      *static* timing the transpiler would need to model — when it blocks,
      it's specifically preventing the simulator from racing ahead of the
      one-phase-per-tick structure `capture_raw_statements` already assumes,
      not adding a latency the transpiler doesn't know about. So this item is
      believed resolved by construction, though not directly verified via a
      transpile + Verilator run — the one example that would exercise it
      (`dual_port_ram`) still doesn't transpile at all (`cannot infer bit
      width` on `Memory`, unrelated — see **P2 — Memory's language
      integration** below). Re-check once Memory transpiles.
- [x] **Resolved (2026-07-24) — P0: `for { tick }` was a silent miscompile.** When
      for-loop lowering landed (for `rotate_right`/`priority_encode`), a
      `clk.tick().await` inside a `for` stopped being unrolled *and* stopped
      erroring — the loop body was treated as combinational, so a counted delay
      (`for _ in 0..4 { tick }`) was **silently dropped** (empty `always_ff`, wrong
      hardware, no error). Now rejected with a clear message via a recursive
      `stmts_contain_tick` guard in the `ForLoop` lowering arm (`if`/`while` were
      already rejected). 1 test. This closes the silent-wrong-hardware hole; the
      *feature* to actually support it is the milestone below.
- [ ] **P1 — No control extraction (own milestone).** The phase FSM extracts a
      **static** count of **top-level** ticks in the module `loop` body
      (`split_at_ticks` doesn't descend into nested bodies; `n_ticks =
      segments.len()-1`). That handles a straight-line pipeline (`mac_pipeline`: 3
      ticks → a 3-phase FSM with `phase_r` + inferred pipeline registers) and
      single-tick FSMs. It **cannot** express, and every form is now rejected (not
      miscompiled):
      - **Counted delays** — `for _ in 0..434 { tick }` — need a **cycle-counter
        register** ("stay in this state until the counter hits N"), *not* 434
        unrolled phases.
      - **Data-dependent waits** — `while cond { tick }` — need a **self-looping
        state** that holds until the condition.
      - Conditional ticks (`if cond { tick }`) — rejected today.
      This blocks `uart/rx` (which uses all three at once) and is what the README's
      "write FSMs naturally with async/await" claim rests on. Needs a richer FSM
      than the linear phase counter: states that self-loop with counters and take
      data-dependent transitions.

      **← DESIGNED, next to implement (2026-07-24). See
      [CONTROL_EXTRACTION.md](CONTROL_EXTRACTION.md).** Chosen architecture: a
      FIR→FIR pass that flattens the async control flow into an **explicit
      single-tick FSM** (`match pc { .. }; clk.tick().await`) — the shape the
      existing pipeline already lowers correctly (`det_010`, `mac_fsm`), so all of
      CHIR/SHIR/VLIR/emit is reused and the new code is confined to the FIR pass.
      The `lower_into` algorithm (straight-line + `if`/`else`, incl. the
      empty-continuation correctness crux) is worked out in the doc. Increments:
      (A) straight-line + `if`/`else`, (B) `while` self-loops, (C) counted `for`
      delays (counter register), (D) `continue`/`break`. Adjudicator:
      `det_010_awaits` (the multi-tick twin of the verified single-tick `det_010`,
      proven equal in sim by `pattern_detector_2`); stretch goal: `uart/rx`.
      **Bonus:** because the output is single-tick, this *dissolves* the timing
      discrepancy below (single-tick is where sim and hardware agree).
- [ ] **P1 — Sim vs hardware timing: a uniform 1-cycle offset for multi-cycle
      sequential logic (2026-07-24). See
      [EXECUTION_MODEL_RECONCILIATION.md](EXECUTION_MODEL_RECONCILIATION.md).**
      Found via the equivalence harness: `mac_pipeline` (multi-tick) is
      `verilator: FAIL` / `trace: PASS` — the transpiled FSM reads inputs one cycle
      later than the sim. Then `mac_fsm` (single-tick, once conditional-output was
      fixed) showed the *same* one-cycle gap on the **output** side (registered
      `out` lags the sim by 1). Unified finding: **natural clocked hardware — a
      register capturing at the clock edge — is consistently one cycle *behind* the
      simulator for multi-cycle sequential logic**, on reads (mac_pipeline) and
      writes (mac_fsm) alike. The two sim codings agree with *each other*
      (self-consistent) but both hardware realizations land one cycle later ⇒ the
      **sim appears to run one cycle "ahead"** of clocked hardware. Not provable
      without an **independent hand-written Verilog reference**; that's the only
      thing that settles it. `mac_pipeline_equivalence` and `mac_fsm_equivalence`
      are `#[ignore]`d documenting this. **Likely resolved by control extraction**
      (above): extracted single-tick FSMs are the agreeing category, so the offset
      may simply not arise once async bodies are extracted rather than phased.
- [ ] **P2 —** Confirm the trailing-segment fix is right: post-tick *wires* are hoisted
      into the same phase's pre-edge, port drives keep their timing. Worth a second look.

## Phase B — CHIR

### CHIR consumption progress (2026-07-23)

The first increments that *consume* the file-scope items captured in #7a, plus
the value-lowering they need. All in `chir_lower.rs`; full detail (scope,
limitations, per-item deferrals) lives under Phase A's **#7b+** list. Ordered as
implemented:

1. **Free-fn inlining** (`2318abe`) — `lower_expr`'s `Call` arm inlines calls to
   file-scope free fns by substitution (params→args, `let`-folding into the tail);
   nested calls inline recursively; recursion rejected.
2. **Impl associated-fn inlining** (`e4363dd`) — same inliner, extended to
   receiver-less impl methods keyed `Type::method` (e.g. `Opcode::from_bits`).
3. **Struct lowering** (`12c099d`) — a struct-valued `let` binds one wire per
   field (`<base>_<field>`), directly or through one level of inlining; field
   access already matched that naming.
4. **Match-as-value** (`bc977d4`) — was largely present (`case` / `Mux`-chain);
   fixed a lone `_` arm against a tuple scrutinee.

> **Option `unwrap_or` (`30cd790`) was reverted (`bd7eed7`).** After weighing the
> hardware mapping, `Option`/`?`/`panic!` are now **banned** in hardware bodies
> rather than lowered — see *Cross-cutting → Banned software idioms*. Maybe-valid
> data is modeled with an explicit `{ valid: Logic, payload: T }` struct (which
> struct lowering already supports).

**Net:** the inline → struct → match stack composes, so `alu_exec_*`-shaped
bodies lower end-to-end. The CPU `decode` path is *not* reachable as written — it
used the now-banned idioms and was **rewritten** to the explicit form: total
`Opcode::from_bits -> Opcode` (with an `INVALID` variant), `BranchCond::from_bits
-> BranchCond` (defaulting to `Beq`), `decode -> InstrDecoded` (no `Option`/`?`),
and an `Opcode::INVALID` arm that traps by halting instead of `panic!`. Both CPU
examples build and pass (10/10 base, 13/13 pipelined).

- [ ] **P1 — Unary `!` on a multi-bit value** emits SV logical-`!` (1-bit reduce)
      instead of bitwise `~`. Correct for 1-bit `Logic`, wrong for `Bits<N>`. Needs
      operand-type dispatch in `lower_unop`. Not triggered by anything passing today.
- [ ] **P1 —** `break` / `continue` exist as FIR variants but aren't handled in CHIR.
- [ ] **P2 —** Width inference is bottom-up only. Targeted propagation exists for
      binop operands and assignment targets; there's no general expected-type pass.

## Phase A — FIR / parser

- [ ] **P2 — Audit for remaining `ExprType::Lit`-only matchers.** Adding `ExprPath`
      silently broke four identifier-extraction sites (`.write()` receivers, hardware
      call names, assignment targets) plus `lower_init_to_lit`. All fixed, but the same
      trap recurs whenever a variant is added. Consider a helper that extracts an
      identifier from *either* `Lit` or `Path`.

### FIR capture-completeness audit (2026-07-23)

Walked all 16 `examples/**/*.rs` against the parser (`parser.rs`) and the FIR
data model (`copper-core/src/frontend_ir.rs`). Goal: the FIR is "source-shaped,
pre-normalization" — it should losslessly represent what a hardware module's
source contains, even for constructs no later phase lowers yet. Scope for this
audit is the **full** example set (incl. CPU/UART). Grouped by severity.

**A — Silent data loss (FIR cannot represent the source at all):**

- [x] **Resolved (2026-07-23) — Method-call turbofish is dropped.** Was:
      `parse_expr_type`'s `Expr::MethodCall` arm ignored syn's `e.turbofish`, and
      `ExprMethodCall` had no field for it — losing the port index in
      `memory.read_port::<0>()` / `write_port::<1>()` and the width in
      `instr.truncate::<7>()` / `instr.part_select::<3>(12)` (the `12` offset
      survived as an arg; the `3` width did not). **Fix:** added
      `ExprMethodCall::turbofish: Vec<String>` (one canonical token string per
      generic arg, empty when absent), captured via `capture_method_turbofish` in
      `parser.rs`. Path-turbofish was already fine (`Bits::<32>::from_lit::<4>()`
      lives in `ExprPath::path_text`); this only closes the *method* form. Purely
      a capture fix — no CHIR consumer reads method turbofish yet. 3 parser tests.
- [x] **Resolved (2026-07-23) — Function generic parameters are never captured.**
      Was: `FrontendSignature` had only `params` + `return_ty`; `capture_signature`
      read neither `sig.generics` nor the where clause — losing
      `<const N: usize, const N_1: usize>` (shift_register, mux) and
      `<SrcD: ClockDomain>` (sync_2ff). **Fix:** added
      `FrontendSignature::generics: Vec<GenericParamMeta>` (+ `where_clause_text:
      Option<String>`), with `GenericParamMeta { kind, name, const_ty, bounds,
      default, raw_text, span }` and `GenericParamKind { Type, Const, Lifetime }`.
      Captured by `capture_generic_param` in `parser.rs`: a const generic carries
      its declared type (`usize`) for pairing with a call-site turbofish value; a
      `ClockDomain`-bounded type param carries its bounds so lowering can tell a
      domain param from an ordinary one. Where clauses preserved verbatim (no
      example uses one, but must not be dropped). 4 parser tests. Capture-only —
      monomorphization is downstream (coverage-map class C).
- [x] **Resolved (2026-07-23) — The `#[hardware(mode)]` argument is discarded.**
      Was: `classify_module` (`parser.rs:121`) inferred classification from
      `sig.asyncness` only, so `synchronizer` (also async) was indistinguishable
      from `sequential` — the exact distinction 2-FF CDC lowering needs. **Fix:**
      added `HardwareMode { Sequential, Combinational, Synchronizer }` and
      `FrontendModuleIR::declared_mode: Option<HardwareMode>`, captured by
      `capture_hardware_mode` (reads the attr's path arg; `None` when the fn has no
      `#[hardware(...)]` or an unrecognized arg — the proc-macro is the layer that
      diagnoses bad modes). `classification` is left as-is (async-inferred, still
      what CHIR branches on); `declared_mode` is the authoritative signal a later
      CDC-emission phase will consult. 2 parser tests.
      **Caveat surfaced:** on this branch the `copper-macros` `#[hardware]` macro
      only accepts `sequential`/`combinational` (see `parse_hardware_mode`,
      `copper-macros/src/lib.rs:11`) — `synchronizer` is *not* wired through the
      macro here, contrary to the frontend section's 2026-07-14 note. The FIR is
      ready for it; the macro is not. Reconcile when synchronizer lowering lands.

**B — Unhandled constructs fall through to the opaque-text `_ =>` arm
(`parser.rs:573`), losing structure:**

- [x] **Resolved (2026-07-23) — three constructs fell through to the opaque-text
      `_ =>` arm; now first-class FIR nodes.**
      - **`const { assert!(...) }`** (`Expr::Const`, shift_register/mux/rotate_right/
        priority_encode) → `ExprType::Const(ExprConst { stmts })`, a captured block a
        later pass can elide explicitly.
      - **`?`** (`Expr::Try`, `decode()`'s `Opcode::from_bits(...)?`) →
        `ExprType::Try(ExprTry { expr })`.
      - **`panic!` / macro invocations** (`Expr::Macro`, `None => panic!(...)` in
        rv32i_cpu; also the `Stmt::Macro` statement form) →
        `ExprType::Macro(ExprMacro { name, tokens_text })` via a shared
        `macro_to_expr` helper. Macro tokens stay raw (a macro body is not
        expression-shaped). This also replaces the old `Stmt::Macro`→`ExprLit`
        path; `test_emit_macro_captured_as_lit_stmt` was renamed and rewritten to
        assert the structured shape.
      CHIR's `ExprType` matches all have catch-all arms, so the new variants land
      in `UnsupportedConstruct` (capture now, lower later) without breaking the
      build. 5 parser tests (const block + its inner assert! macro, try, expr-macro,
      stmt-macro).

**C — Architecture (required for CPU/UART, deliberately unbuilt):**

- [~] **P1 — File-scope items beyond enums.** Was: only file-scope enums were
      captured (`capture_file_enums`); free fns (`decode`, `alu_exec_*`,
      `sign_ext_*`), structs (`InstrDecoded`, `AluOutput`), impl methods
      (`Opcode::from_bits`, `BranchCond::from_bits`), traits (`ReadOp`, `WriteOp`),
      and file-scope consts (`CLKS_PER_BIT`) were unreachable from the module's
      `ItemFn`. Single biggest blocker for CPU and UART. Own milestone, split into
      capture (#7a, **done**) and consumption (#7b+, open).

  **✅ #7a — capture DONE (2026-07-23).** Mirrors `capture_file_enums`: captured
  into the FIR, nothing consumes it yet, **zero transpile-behavior change**
  (confirmed by the `transpile_source` golden e2e tests, incl.
  `enum_state_machine_golden_output`, all still pinning identical emitted SV).
  - New FIR types (`copper-core/src/frontend_ir.rs`): `FrontendFnIR { name,
    signature, receiver: Option<Receiver>, raw_statements, span }`,
    `FrontendImplIR { self_ty, trait_name, methods, span }`, `FrontendTraitIR {
    name, methods, span }`, `Receiver { Value, Ref, RefMut }`. `signature` reuses
    `FrontendSignature` (already carries #2 generics); `capture_signature` was
    refactored to take `&syn::Signature` so modules, free fns, and methods share it.
  - Capture (`parser.rs`): `capture_file_scope(&File, &hardware_fns) -> FileScope`
    walks file items — free fns (hardware modules excluded), structs, consts,
    impls (incl. empty `impl ClockDomain for MainClk {}` markers → `methods: []`),
    traits (bodyless decls → empty `raw_statements`, receiver still captured). The
    `const`/`struct` capture arms were extracted into shared helpers
    (`capture_item_const` / `capture_item_struct`) reused by in-body and file-scope.
  - Container (Option A): sibling `file_fns`/`file_structs`/`file_consts`/
    `file_impls`/`file_traits` fields on `FrontendModuleIR`, injected in
    `lib.rs::transpile_source` via `inject_file_scope` right beside the enum
    injection. No `transpile_fir`/`lower_to_chir` signature changes.
  - 5 parser tests: free-fns+consts (`decode` with `?`+struct-return body captured,
    hardware module excluded), structs (named + tuple → `field_0`…), impl methods
    (inherent assoc-fn no receiver; marker trait-impl), traits + `&self`/`&mut self`
    receivers, and the empty-file-scope invariant for a bare `ItemFn`.

  **#7b+ — consumption (separate milestones, dependency-ordered):**
  1. [~] Free-fn **inlining** — **increment 1 done (2026-07-23).** CHIR now inlines
     calls to file-scope free fns (`build_fn_registry` on `LowerCtx`; the `Call`
     arm of `lower_expr` dispatches known `file_fns` to `lower_inlined_fn_call`).
     Mechanism is substitution-based: params bound to args, `let` bindings folded
     into the tail via `substitute_expr`, result lowered; nested helper calls
     inline recursively; direct recursion rejected. **Scope:** receiver-less free
     fns whose body is `let`-bindings + a tail expr (pure combinational). Rejects
     arg-count mismatch and non-`let` statements before the tail. So `sign_ext_*`-
     shaped helpers inline end-to-end; `decode`/`alu_exec_*` still need the layers
     below (they use `?` / match-as-value / struct returns). **Limitations:** no
     shadowing tracking in `substitute_expr` (pure helpers don't rely on it).
     **Increment 2 done (2026-07-23) — impl associated fns.** `build_fn_registry`
     now also registers receiver-less impl-block methods under their qualified
     name (`Opcode::from_bits`), the exact string `call_path` yields for a
     `Type::method(args)` call — so the existing inliner handles them with no
     dispatch change. Covers the `from_bits`-shaped calls used by the CPU
     (mechanism-wise; they still need `?`/match-as-value to fully lower). Still
     deferred: **instance** methods (`self`-receiver, called via
     `receiver.method(..)`) — need self-binding + receiver-type disambiguation; no
     example uses them in a hardware body. 6 + 3 tests.
  2. [~] **Struct lowering** — **increment 1 done (2026-07-23).** A struct-valued
     `let` binds one wire per field (`<base>_<field>`), matching the name field
     access already lowers to (`base.field` → `base_field` var). `lower_stmt` and
     `lower_comb_body` route `let`s through a shared `lower_local_binding`;
     `resolve_struct_literal` recognizes a struct literal directly *or* through one
     level of free-fn inlining (so `let x = make(..)` where `make` returns a struct
     works); `resolve_field_type` types each field from the struct def (Bits/prim,
     or enum→width) with a value-inference fallback. Struct/enum registries added to
     `LowerCtx` (`build_struct_registry`). 4 tests. **Deferred:** `usize`/`isize`
     field types (fall to inference; `rd: usize`-style fields not yet), `..rest`
     functional update (rejected), nested structs, struct in match/if arms, and
     struct bindings in the sequential *pre-loop* scope (`lower_seq_body` collects
     those separately — not yet struct-aware).
  - [x] **Match-as-value — done (2026-07-23).** Was already largely implemented:
     `lower_expr`'s `Match` arm produces `CHIRExpr::Case` (all-literal / whole-
     wildcard patterns) or `lower_match_as_chain` → a `Mux` chain (guards, binders,
     partial wildcards, or-patterns, tuple scrutinees). The one gap found probing
     the `alu_exec_*` shape: a lone `_` arm against a **tuple** scrutinee was
     rejected on an element-count check (`parse_pattern_elems("_")` is 1 elem, the
     tuple 2). Fixed — a whole-pattern `_` is now unconditional regardless of
     scrutinee arity. So `alu_exec_reg`/`alu_exec_imm`-shaped `let r = match (t) {
     .. }` (partial-wildcard arms, if-expr arm bodies, trailing `_`) now lowers.
     2 tests; golden e2e unchanged.
  3. ~~**`Option`/`Result` + `?`**~~ — **resolved by banning, not lowering
     (2026-07-23).** Hardware has no `Option`; every lowering strategy (fold =
     logic duplication + erased valid signal; valid-bit = hidden state + `?`
     threading) fights the model, and `panic!` is unsynthesizable. Decision:
     reject `Option`/`?`/`panic!` in hardware bodies with actionable errors and
     use explicit `{ valid, payload }` structs instead. Enforced at the macro
     (see *Cross-cutting → Banned software idioms*). Increment 1 (`unwrap_or`
     fold) reverted.
  4. **Enum-with-methods** — `Opcode::from_bits` etc. return `Option`, so this
     folds into the ban above; an inherent-`impl` fn that returns a plain value
     still inlines (increment 2).
  5. [~] **Const generics** — **increment 1 done (2026-07-23).** Chose *SV
     parameters* over monomorphization (SystemVerilog has `parameter` natively —
     honest and one module, not one-per-instantiation). Filled the scaffolded
     `Width` M2 seam: `Width::Param(String)` (+ `as_concrete`); `Bits<N>` with an
     identifier arg → symbolic width; `CHIRModule.params` from the FIR's const
     generics (#2 capture) threaded CHIR→SHIR→VLIR; emit renders `#(parameter int
     N = <default>)` and `[N-1:0]`. A passthrough `Bits<N>` module emits a
     Verilator-lint-clean parametric module. `rotate_right`/`priority_encode`/
     `shift_register` now advance *past* `Bits<N>` to their next gaps (for-loops,
     width inference); `mux` hits array ports. **Deferred:** validating a
     `Bits<N>` param is actually declared (any identifier is accepted; undeclared
     → fails at emission), `Width::Sub` for `N-1`-style derived widths, and the
     companion features those examples still need (for-loops over a symbolic `N`,
     array ports, LHS bit-assign).
  6. **File-scope const substitution** (`CLKS_PER_BIT` into loop bounds; interacts
     with control-extraction, a separate P1).

**D — Hygiene (not a parser capability, but blocks even testing A–C):**

- [ ] **P2 — Example port-model / attribute drift.** Contrary to the frontend
      section's "all 17 examples carry `#[hardware]`," on this branch only
      `dual_port_ram`, `rv32i_cpu`(+pipelined), `one_bit_comparator`, `mux` are in a
      state `transpile_source` accepts. Missing the attribute (In/Out sig present):
      `two_domain_counter`, `shift_register`, `pipeline_mac`, `lfsr`,
      `traffic_light_fsm`, `pattern_detector`, `uart/rx`, `uart/system`. Still on
      the old return-value model (no In/Out): `rotate_right`, `priority_encode`,
      `ripple_carry_adder`. Decide which are canonical and bring them to the port
      model + attribute, or they can't exercise the pipeline.

> **Well-captured (verified, no action):** control flow, all binops incl.
> compound-assign/shifts, unops, casts, indexing, field access, struct literals,
> tuple-pattern `match` + guards, ranges, `break`/`continue`, references, in-body
> enums, clock domains. Composition is testbench-level (`spawn_wired` from a plain
> fn), so no in-body submodule instantiation exists to capture — the
> `ExprCall.is_hardware_module` path is currently unexercised by any example.

---

# Frontend / language design

Not transpilation — the user-facing language: the `#[hardware]` macro, the port
model, and the simulator API.

- [x] **Resolved — `#[hardware]` is mandatory and safe to attribute
      everywhere; `pre_edge_barrier` is gone, replaced by a per-read freshness
      check.** Was P0 ("optional and changes simulation semantics"), then
      went through several rounds of "fix the barrier's placement heuristic"
      that each solved one shape and broke the next (blunt loop-end placement
      → output-position sensitivity on `counter`/`traffic_light` → composed
      modules like `two_domain_counter`'s `slow_consumer` → conditionally-
      executed reads like `mac_fsm`'s `match` arm) — every one of those was a
      symptom of trying to decide, from one loop's syntax in isolation,
      whether a read needed protection. The actual fix replaces that
      approach entirely:
        - `copper-codegen::transpile_source` rejects a Clock/In/Out-shaped
          function without `#[hardware(...)]`, so the transpiler side has
          been mandatory since early in this work.
        - `pre_edge_barrier`, `PreEdgeBarrier`, and `LoopDeltaInjector` are
          deleted. In their place, `copper-sim/src/synced_read.rs` (private,
          `#[doc(hidden)]`, reached only via fully-qualified paths from
          macro-generated code) implements a per-*port* freshness check: a
          read blocks only if the enclosing loop has wrapped since that
          port's tracker last succeeded *and* no real `tick_clock()` call has
          happened since — i.e. it's a no-op whenever a read's natural cadence
          is slower than or equal to the rate new values arrive (covers
          `counter`, `mac_fsm`, composed modules like `slow_consumer` fed by
          `sync_2ff`'s output), and only actually suspends when a loop would
          otherwise double-consume a value within one `tick_clock()` call
          (`dual_port_ram`'s original case). One mechanism, no heuristics
          about loop shape, and it doesn't care whether a port is
          testbench-driven or fed by another module.
        - `copper-macros/src/lib.rs`'s `inject_synced_reads` wires this in:
          one hidden, function-scoped, never-reset wrap counter (shared by
          every tick-bearing loop in the function, including nested/re-entered
          ones — a *reset*-per-loop counter would go backwards relative to a
          tracker's high-water mark and cause spurious blocks on legitimate
          re-entry); one hidden tracker per `In<T, D>` parameter; every
          `<param>.read()` call site rewritten to go through them. Entirely
          invisible to users — `.read()` in source stays exactly as written.
        - **Fails loud, not silently, on patterns it can't protect.**
          Before rewriting anything, `find_unprotectable_in_uses` rejects the
          whole macro invocation if an `In` parameter is used any way other
          than a direct `.read()` call (through a `.clone()`, a reassignment,
          passed to a helper) — it can't see into those, and given the entire
          history above is "silently did the wrong thing in an unanticipated
          shape," under-protecting silently was not an acceptable fallback.
          See `copper-macros/tests/ui/fail/sequential_unprotectable_read.rs`.
        - Validated against every failure mode found during the investigation
          (dual_port_ram-style staleness, output-position sensitivity,
          same-port-twice-in-one-iteration, composed modules, multi-tick
          loops) before wiring into the macro, then against full regression
          across every previously-attributed example plus `rv32i_cpu`
          (10 tests incl. 1679-cycle bubblesort) and `rv32i_cpu_pipelined`
          (13 tests) — both full of nested/re-entered wait-loops — and the
          `counter`/`traffic_light` fixtures still pass their full Verilator
          cross-check.
        - **✅ Simulation side now mandatory too, via a marker type
          (2026-07-14).** The transpiler already rejected an unattributed
          hardware-shaped fn, but *simulation* still accepted a plain
          `async fn` (spawn took any `Future`). The `#[hardware]` macro now
          rewrites `async fn m(..) {..}` into
          `fn m(..) -> HardwareModule<impl Future> { HardwareModule::__new(async move {..}) }`,
          and `HardwareExecutor::spawn` / `spawn_wired` / `spawn_child` require a
          `HardwareModule` — whose only constructor is the hidden `__new`. So a
          bare `async fn` produces a `Future`, not a `HardwareModule`, and cannot
          be spawned: forgetting the attribute is `error[E0308]: expected
          HardwareModule<_>, found future` (compile_fail doctest on
          `HardwareModule`). This closes the "you can forget the macro and
          silently simulate an unprotected module" gap for good.
        - Fallout handled: the affine-port prototype (a disconfirmed experiment
          that couldn't use the macro) was deleted; the composition test's stages
          are now attributed, and its `counter_by` became a const-generic
          `<const STEP: u8>` (a compile-time module parameter — which the macro
          accepts, unlike a runtime non-port `u8`).
        - **✅ Legacy `emit!` subsystem removed entirely (airtight, 2026-07-14).**
          The 5 `*_function_typed`/`*_into_with_unknown` spawn methods took raw
          futures (a marker bypass) and, with the whole `emit!` machinery
          (`emit_to_current`, `push_emit_target`/`take_emit_dirty`,
          `EmitTargetGuard`, the `emit!` macro, `emit_target`/`set_unknown` task
          fields, the X-injection path in `poll_tasks`) plus ~11 tests, were dead
          code from the pre-`In`/`Out` model. All removed. Now the *only* public
          spawn paths require a `HardwareModule`, so there is no way to spawn a
          raw future — the mandatory-macro guarantee has no bypass. `poll_tasks`
          kept its combinational-loop detection (now a plain panic, no X-inject).
      **Net effect: all 17 `examples/` files carry `#[hardware]`** (up from 5 at
      the start), the 3 pre-`In`/`Out` stragglers having since been migrated, and
      the attribute is now mandatory on *both* the transpiler and simulator sides.
- [~] **CDC enforcement — audited 2026-07-14.** Executable spec + audit is now
      `copper-core/src/cdc.rs` (6 doctests: 3 `compile_fail` guarantees, 3 passing).
      Findings:
      - **Holds:** cross-domain *connections* are `E0308` at compile time —
        `In`/`Out`/`Clock` of one domain into a port of another (all three verified).
        So **every domain crossing must be a visible, typed module boundary**;
        implicit crossings are impossible. That guarantee is real.
      - **Gap 1 — synchronizer correctness is NOT checked.** `.read()` erases the
        domain to a plain `T`, so a mixed-domain module can forward `fast → slow`
        with no flip-flops and still compile. The type system localizes the
        crossing; it does not verify the CDC logic inside it.
      - **Gap 2 — clocks are freely constructible** (`Clock::<D>::new()` anywhere);
        the association lives on ports/wires, enforced at connection.
      - **Wording — for the paper:** the mechanism is **phantom-type domain
        tagging**, not "ownership." The ownership property (`Out` non-`Clone` →
        single writer) prevents multiple drivers, a *different* guarantee. The
        README's "ownership-based CDC" and "compile-time CDC verification" both
        overstate — it verifies crossing *points*, not crossing *correctness*, via
        type params (which Clash/others also do; see `paper/` prior-work notes).
      - **✅ Gap-1 fix — shipped (library-synchronizer design), 2026-07-14.**
        Prototyped two approaches: (a) domain-tagged read values — *rejected*, taxes
        every module (`.get()` on every conditional/mixed read; `if` can't coerce a
        wrapper to `bool`); (b) a **library synchronizer module** — *chosen*, honest
        to the hardware and zero tax on single-domain modules. What landed:
        - `#[hardware(synchronizer)]` mode — the only kind allowed a foreign-domain
          port (the sanctioned crossing point).
        - `copper::sync_2ff` (root package `src/sync.rs`) — a provided two-FF
          synchronizer, `Logic`-only (a 2-FF on a multi-bit bus is itself a CDC
          bug), generic over source+destination domains.
        - Signature-level CDC check in `#[hardware(sequential)]`: a regular module
          may not declare a foreign-domain port (in or out). Multi-clock skipped.
          Correctly ignores the default `()` domain (`In<u8>` isn't "foreign").
        - `two_domain_counter` migrated to `copper::sync_2ff` (hand-written one
          deleted); runs green. An ad-hoc cross-domain `#[hardware(sequential)]`
          module now fails to compile with an actionable message.
        - Audit doc `copper-core/src/cdc.rs` updated: Gap 1 now closed at the macro
          layer.
      - **✅ README softened + tests/examples added (2026-07-14).** README bullet
        rewritten from "Ownership-Based CDC Safety / First HDL / verification" to
        "Typed Clock Domains" — accurate: phantom-type domains, crossings localized
        to explicit synchronizer boundaries, *not* a correctness proof of the
        synchronizer. (The bibtex citation title still says "Ownership-Based CDC
        Safety" — left for the authors to decide.) Added: `compile_fail` + pass
        doctests in `src/sync.rs` (regular module rejected; custom
        `#[hardware(synchronizer)]` 3-FF accepted), a runnable
        `tests/cdc_crossing.rs` (sync_2ff latency + settling), and
        `examples/cdc/flag_crossing.rs` (a minimal fast→slow flag crossing).
        Confirmed users can write their own synchronizer via
        `#[hardware(synchronizer)]`.
      - **✅ Gap 2 (clock construction) closed at the macro layer (2026-07-14).**
        Clocks are instance capabilities (`Arc<ClockState>`), so a fabricated clock
        *hangs* rather than mixing domains (low severity). The `#[hardware]` macro
        now rejects `Clock::new()` / `Clock::default()` in any module body
        (`check_no_clock_construction`, all modes) — modules receive clocks as
        params and may only `.clone()` them. 4 unit tests + `compile_fail`/pass
        doctests in `src/lib.rs`; audit doc updated.
      - **Remaining:** transpiler lowering of a `#[hardware(synchronizer)]` /
        submodule to Verilog (2-FF emission) — deferred with the rest of hierarchy
        emission (M3). Both audited CDC gaps are now closed at the macro layer.
- [x] **Macro now verifies the top-level infinite loop (2026-07-14).**
      `#[hardware(sequential)]` / `synchronizer` with no top-level `loop` is
      rejected at the macro with a clear message, instead of being accepted and
      failing later in CHIR (`has_top_level_loop` + a check in
      `validate_hardware_fn`). Combinational modules are exempt (the macro adds
      their loop). 4 unit tests: missing loop rejected, pre-loop `let`s ok,
      synchronizer covered, combinational exempt.
- [x] **Three examples migrated off the pre-`In`/`Out` port model.**
      `rotate_right`, `priority_encode`, `ripple_carry_adder` now use
      `#[hardware(combinational)]` + `In`/`Out` params (const generics kept), write
      outputs via `.write()`, and their testbenches use `spawn_wired` / `poll_tasks`.
      All three still simulate (`All tests passed`) and are now detected by the
      transpiler's module detection. `full_adder` intentionally stays a plain
      combinational helper fn. **Note:** they still do not *transpile* — blocked on
      const generics (class C), `for`-loop unrolling + LHS bit-assign (class D), and
      `rotate_right`'s dynamic bit select — all tracked in the coverage map, not here.
- [ ] **P2 — Memory's language integration.** The simulation side is complete and
      the semantics are now pinned down (pipelined, `READ_LAT`/`WRITE_LAT`,
      ReadFirst — see the fifo equivalence rewrite). What remains is whether
      `Memory` is a first-class construct or a library type; plan in
      `MEMORY_DESIGN.md`.
- [ ] **P2 —** `Bits` correctness TODO (`copper-core/src/types.rs:313`) and
      unhandled overflow (`:894`).
- [ ] **P2 —** Simulator waveform/VCD API TODO (`copper-sim/src/lib.rs:230`).

> The conditional-output semantics question (implicit hold / `OutReg` / proof
> tokens) is a frontend decision too, but it gates Phase D's P0 items — it is
> tracked under **Cross-cutting** below.

---

## Cross-cutting

- [x] **Banned software idioms: `Option` / `?` / `panic!` (decided + enforced
      2026-07-23).** These have no honest gate mapping — folding duplicates logic
      and erases the valid signal, a valid-bit representation hides state and needs
      cross-fn `?` threading, and `panic!` is unsynthesizable (any lowering is the
      compiler guessing a value on an "impossible" path). Per Copper's
      "unrepresentable bugs" premise they are **rejected**, not lowered:
      - **Enforcement:** `check_no_unsupported_idioms` in `copper-macros`
        (`UnsupportedIdiom` visitor) rejects `?`, `panic!`/`unreachable!`/`todo!`/
        `unimplemented!`, and `Option` (`Some`/`None` expr+pattern, `Option<T>`
        type, `unwrap`/`unwrap_or`/`expect`) in any hardware-module body, at the
        definition site, with actionable messages pointing to the explicit form.
      - **Explicit form:** maybe-valid data → a `{ valid: Logic, payload: T }`
        struct you branch on (struct lowering already supports this); an error
        condition → an explicit output (e.g. `illegal: Logic`), not a panic.
      - **Fallout:** the `unwrap_or` lowering increment was reverted (`bd7eed7`).
        The `rv32i_cpu`(+pipelined) examples used these idioms in `decode` and were
        **rewritten** to the explicit form (total `from_bits` returning the enum
        with an `INVALID`/default variant; `decode -> InstrDecoded`; an
        `Opcode::INVALID` arm that halts instead of `panic!`). Both re-enabled and
        passing; workspace green. (The pipelined version's only `panic!` was in its
        testbench, not the module, so it needed no change.)
      - **Not yet:** the macro only sees a module's own body. A banned idiom inside
        a *file-scope helper* that gets inlined (e.g. a `from_bits` returning
        `Option`) is caught later by CHIR as an unsupported construct, not yet with
        the same actionable message — a future polish. A first-class `Valid<T>`
        sugar (explicit `.valid`/`.payload`, no `?`/panic) could be revisited later
        if ergonomics demand, but is out of scope.

      **Design note — why a syntactic scan, and the `HardwareType`-bound path
      (future refinement, decided to defer 2026-07-23).** We considered making
      these *type* errors instead of a macro scan. Per construct:
      - **`?`** is *already* a type error in a module body — a hardware fn returns
        `()`, and `?` needs a `Try`/`FromResidual` return type. The macro check
        just supplies a nicer, domain-specific message over an error Rust already
        gives.
      - **`panic!`** *cannot* be a type error: it has type `!` and coerces to
        anything, so it is always well-typed. Rejecting it requires a syntactic
        scan or a custom lint — **there is no type-only route**, which is the
        decisive reason the scan stays even if the rest moves to types.
      - **`Option`** *could* be a genuine type error via a `HardwareType` marker
        trait (implemented by `Bits`/`Logic`/hardware structs, not `Option`)
        threaded through the hardware value model so every value in a module body
        must be provably a hardware type. Upsides over the scan: it also catches
        banned idioms **inside inlined helpers** (closes the gap above) and has no
        syntactic false positives (a user fn named `Some`, an unrelated `unwrap`).
        Cost: it interacts with generics, inference, and every port/op signature —
        a proper language-design milestone, best paired with the wider typing /
        width-checking story, **not** a same-day swap.
      **Decision:** keep the macro scan as the mechanism now (ships, tailored
      errors, and needed for `panic!` regardless); pursue the `HardwareType` bound
      later as its own milestone — it would subsume only the `Option` third of the
      scan, never the `panic!` part.
- [~] **Conditional/phased output semantics — decided + implemented (2026-07-24).**
      A conditionally-driven **output port** in a sequential module is an
      **implicit-hold register**: its drive lowers to a guarded `always_ff`
      `out <= v` (holding otherwise), not a combinational latch. Unconditional
      outputs stay `assign`s. Unblocks `mac_fsm` and the Phase D multiply-driven P0.
      `vlir_lower::{conditional_output_ports, split_output_regs}`. **Caveat:** this
      matches the sim's *storage* semantics (out holds between writes) but not yet
      its *timing* — the register lands one cycle later (see the 1-cycle-offset
      finding under Phase C / EXECUTION_MODEL_RECONCILIATION.md), which is the
      remaining open part, expected to resolve with control extraction.
- [ ] **P2 — Stale design docs.** `TRANSPILATION_PLAN.md`, `FIR_DESIGN.md`,
      `CHIR_DESIGN.md` still describe the obsolete `function_typed` / `emit!` model.
- [ ] **P2 — Retire `copper-codegen/src/verilog.rs`** (legacy, unreachable from the
      current pipeline).

---

## Verification you can lean on while auditing

- `cargo test --lib -p copper-codegen -p copper-core` — 296 + 102, 0 failures.
  (`cargo test --workspace` also builds the examples; skip a WIP example if one
  doesn't yet compile.)
- **8 equivalence tests** (`tests/*_equivalence.rs`) behaviorally verify
  transpiled-SV vs sim under Verilator, `.with_params` for parametric modules:
  `m1_counter`, `lfsr`, `pattern_detector`, `traffic_light`, `shift_register`
  (N=8), `rotate_right` (N=8), `priority_encode` (N=8), `det_010`. Two more
  (`mac_pipeline`, `mac_fsm`) are `#[ignore]`d — the multi-cycle 1-cycle-offset
  finding (Phase C). The harness caught two real bugs this session.
- Golden tests pin exact emitted text for the counter, the `Logic` comb module,
  block-branch `if`, and the enum FSM.
- **Caution:** Verilator *lint* is not a sufficient gate — it missed the
  multiply-driven output. But the equivalence harness (sim vs Verilator sim, sim
  = source of truth) does catch behavioral disagreement; lean on it for any
  sequential change.
