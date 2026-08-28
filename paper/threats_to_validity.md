# Threats to Validity & Language-Substrate Limitations (draft)

> The honest cost of building an FSM surface on top of Rust's `async` lowering. Each item is a
> reviewer question we should preempt rather than have raised. Grounded in the Rust async model
> (self-referential futures / `Pin`; conservative state-machine layout; no-borrow-across-await;
> boxed async recursion) and in the code (`copper-sim/src/executor.rs`, `copper-macros/`).
> `[VERIFY]` marks anything to confirm against the current tree before camera-ready.

Building the FSM surface on rustc's coroutine transform (contribution 1) means inheriting the
properties of that transform — including several it was never designed to guarantee. We
separate these into *substrate limitations* (properties of Rust `async` we depend on but do not
control) and the *legal synthesizable subset* (the restrictions the `#[hardware]` macro must
impose because the coroutine surface is strictly more expressive than synthesizable hardware).

## Substrate limitations (properties of Rust `async` we inherit)

**T1 — "Live across `.await` = register" is an upper bound, not rustc's actual capture set.**
The registers of an FSM are, semantically, the values that must survive a clock edge. Rust's
generator transform is *conservative*: to avoid a hard liveness analysis across suspension
points it may retain state that is never read again, and the resulting `Future` layout is an
explicit non-contract that changes across compiler versions. Consequences: (a) simulation
behavior is unaffected — retained-but-unread values are inert, so the sim stays correct; only
future *size* is affected; (b) but we must not claim rustc's `Future` layout *is* the register
set — it is a conservative superset. The synthesizable register set is therefore computed by
Copper's own liveness pass (the reachability/liveness CFG), **not read off the coroutine
layout**. We frame this as motivation, not a wart: the coroutine transform gives correct
*simulation behavior* for free; a separate liveness analysis gives the *minimal register set*
for synthesis, and the two are deliberately distinct jobs.

That pass now exists: the `copper-analysis` crate builds a control-flow graph over the module's
`syn::ItemFn` (`Cfg`, `copper-analysis/src/cfg.rs`) and infers the register set by **backward
liveness** — a local is a register iff it is *defined inside the loop* **and** *live across a
tick edge* (`infer_registers` / `Cfg::registers`). Being a `syn::ItemFn` analysis, it is the
*one* authoritative pass **both** front-ends consume (the `#[hardware]` macro and the transpiler
`copper-codegen`), so the sim and the netlist cannot disagree on the register set by
construction. Its correctness — and specifically that it is the *minimal* set, not rustc's
superset — is evidenced two independent ways: (i) a **structural reg-for-reg match against
independent hand-written SystemVerilog** (`copper_analysis::assert_source_registers_match_reference_sv`,
wired into `tests/common::EquivalenceTest::with_reference_registers` for `mac_fsm` name-exact and
`det_010`/`det_110101`/`lfsr` storage-equivalent); and (ii) a **reconciliation against the
transpiler's own emitted flip-flops** (`copper-codegen/tests/register_reconciliation.rs`), which
holds across the fixture corpus (97 clocked modules as of 2026-08-27) with codegen adding only its
synthesized phase/pc FSM counter — i.e. the inferred set is exactly the design's registers, with no rustc-style
over-capture.

**A counter-example found and fixed (2026-08-21) — worth reporting, because it is evidence the
check has teeth.** Both evidence paths above were *sequential-scoped*: the G2 references are all
sequential DUTs, and `register_reconciliation.rs` filtered on `#[hardware(sequential)]`, so
`#[hardware(synchronizer)]` modules had never been reconciled. Lifting that filter found the
library CDC primitive `copper::sync_2ff` was **under**-approximated — inference reported one
flip-flop where the simulator's behaviour, an independent hand-written reference, and codegen all
have two.

The cause is instructive for the liveness rule itself. `ff2` is defined *post*-tick and read
*pre*-tick, so its live range crosses the loop back edge but no tick edge, and a rule keyed only
on ticks classified it a combinational wire. It is not one: `ff2 = ff1` reads `ff1`'s *pre-edge*
value (the next statement overwrites it), which a wire cannot reproduce — `assign ff2 = ff1` would
track the post-edge value and collapse the two synchronizer stages into a single flop. The rule now
has two clauses, tick edge **and** loop back edge, on the principle that a local defined in a
post-tick segment had its defining expression evaluated against pre-edge values, so if it survives
to the next iteration it needs storage.

The fix is validated against a differential oracle rather than by argument: inference is now
compared to codegen's emitted flip-flops across every clocked module that transpiles in
`tests/fixtures`, `examples`, and `src` — **41 of 41 agreed** at the time (97 as of 2026-08-27),
synchronizers included, with nothing newly over-reported. `register_reconciliation.rs`
covers `synchronizer` permanently so the shape cannot regress, and
`tests/cdc_synchronizer_anchor.rs` adds a structural reg-for-reg match against the independent
reference. Both production call sites of `infer_registers` only log it, so the correction changed
no emitted hardware — but it does restore the unqualified claim, and it removes the caveat that
routing codegen through the shared set would have dropped a flop on every synchronizer.

**T2 — We use the coroutine *transform*, not the async *runtime*.** Copper's executor polls
every task each delta cycle with a no-op waker (`copper-sim/src/executor.rs`); it uses none of
async's readiness/waker scheduling. This is deliberate and should be stated: we repurpose
rustc's coroutine *lowering* as an FSM encoder and supply our own synchronous scheduler; we do
not claim to use Rust's async executor model. Left implicit, a reviewer reads "misuse of
async"; stated, it is a precise design boundary.

**T3 — Async completion/cancellation has no hardware meaning.** Futures can complete and be
dropped mid-flight, running destructors — concepts with no hardware analogue. Copper uses only
the *suspension* half of the model: modules are non-terminating loops, and the executor
*panics* if a module future ever returns (`executor.rs`). Cancellation/`Drop` mid-simulation is
outside the model. This is a conceptual mismatch we fence structurally rather than a bug.

**T4 — No borrows may cross `.await`.** Rust forbids holding a reference across a suspension
(self-referential-future / `Pin` safety). For hardware this means any value live across
`clk.tick().await` must be owned/`Copy` — one cannot hold a `&mut` into a memory or port across
a clock edge. This aligns with hardware state being values rather than aliases, but it is a real
authoring constraint (notably on `Memory` ergonomics) and a deliberate restriction, not an
accident of the embedding.

**T5 — Structural recursion cannot ride the async path.** `async fn` recursion requires heap
indirection (boxing; stable since Rust 1.77) because the future would otherwise be
unbounded-size / self-referential — and heap indirection is meaningless for synthesis.
Recursively-structured hardware (a recursive reduction/adder tree, a divide-and-conquer
generator) therefore *cannot* be expressed as a recursive `async fn`; it must use const-generic
unrolling or explicit structural instantiation. This bounds what the coroutine surface can
express structurally.

## The legal synthesizable subset (why the macro restricts, not extends)

Unlike the usual HDL situation — a host language *less* expressive than needed — Rust's
coroutine surface is *more* expressive than synthesizable hardware: arbitrary `async` control
flow produces valid state machines that do not all correspond to clean hardware FSMs. The
`#[hardware]` macro's role is therefore to **carve out** a synthesizable subset. The subset (as
enforced / to-be-enforced; several are open items in the macro TODO) is:

- **One top-level loop shape.** A sequential module body is a single non-terminating
  `loop { … }`; the loop's iteration is one FSM traversal. (Combinational modules take the dual
  shape: a `loop { …; delta_yield().await; }` with no `clk.tick()`.)
- **`clk.tick().await` must be a *direct* await.** The clock edge cannot be hidden behind an
  arbitrary user `async fn`, so that every suspension point is statically identifiable as a
  clock-cycle boundary (and not, e.g., an arbitrary future). Awaiting anything other than the
  sanctioned clock/`tick`/`delta_yield` primitives is rejected.
- **No `.await` in sub-expression positions that fork a cycle boundary ambiguously** (e.g.
  multiple `tick().await`s within a single expression), so the mapping await-site ⇒ FSM state
  is unambiguous.
- **Bounded, statically-known iteration for anything meant to unroll** into combinational logic;
  data-dependent unbounded loops between ticks are not synthesizable.
- **Combinational modules:** every output assigned on all control-flow paths (no inferred
  latch); no stateful constructs / no `clk.tick()`.
- **No borrows across `.await`** (T4), **no async recursion** (T5), **no shadowing of hardware
  parameters**, and no persistent-state mutation before the first tick in a way that perturbs
  register inference.

We present this subset as a *specification of the contribution's scope*, not a disclaimer: the
claim "rustc's coroutine lowering is the FSM" holds precisely on this subset, and the macro's
job is to reject — at compile time — the coroutine programs that fall outside it. `[VERIFY:
reconcile this list against the actual macro checks in copper-macros/src/lib.rs and the
SIMULATOR/MACRO items in TODO; mark which are enforced vs planned before submission.]`

## Correctness-argument threat (cross-cut with contribution 2)

**T6 — Same-source equivalence is empirical, and the two halves share a front end only at the
source-text level.** The simulator (run the Rust) and the transpiler (lower FIR→…→SV) are
independent derivations from the same source; their agreement under Verilator is evidence
*because* they are independent. This independence is load-bearing and is the reason we do **not**
move the simulator to interpret a shared IR (which would make agreement true-by-construction and
therefore vacuous). The residual threat is that both could diverge from hardware in the same
way; we mitigate this — but do not eliminate it — by anchoring the simulator to third-party
SystemVerilog (BaseJump STL) and hand-written primitive references, so at least the anchored
constructs rest on hardware neither we nor our transpiler authored. The bound on this mitigation
is coverage: it is only as strong as the set of anchored modules. `[VERIFY: keep in sync with
intro_contributions.md contribution 2's evidence list.]`

## T-align — the pre-tick alignment restriction (added 2026-08-21)

A third restriction on the legal synthesizable subset, and the one that costs the most to admit.

**The defect.** A plain combinational `Out` driven from a register in a module's pre-tick segment
diverged silently between simulation and the transpiled netlist when that segment also assigned a
register with no preceding input read. Measured: `loop { r = r+1; o.write(r); tick; }` simulates
`[2,3,4,…]` against its own SV's `[1,2,3,…]`.

**Why it is worth reporting rather than eliding.** The mechanism indicts the coroutine surface
itself — contribution 1. A module's clock-phase alignment was decided by an *incidental* property:
whether it happened to read an input before its tick, because a leading read injects a pre-edge
barrier and nothing else does. Two structurally identical modules therefore behaved differently.
Worse, the divergence had **corrupted an anchor**: `two_domain_hierarchy_cdc.rs` — the independent
hardware check for the dual-clock design — was green because this defect and a second one cancelled
inside the chain. A green anchor agreeing for the wrong reason is worse evidence than a missing one,
and only a deliberate experiment (correcting one side and watching the boundary move) exposed it.

**The fix, and its cost.** The shape is rejected at compile time
(`copper_analysis::unprotected_pretick_out_write`), with `RegOut` or a post-tick update as the legal
forms and an explicit `allow_pretick_alignment` waiver for fixtures that demonstrate the hazard. So
the `sim ≡ synth` claim holds for every *accepted* program — but the accepted set is now smaller in a
way a reader should be told about, and finding the boundary took three rejected candidate fixes
(a uniform pre-edge barrier, a register-keyed static rule, and Prost-style codegen), each rejected on
measured evidence rather than argument — five rejected fixes by 2026-08-27, once two widenings of
D1 were also measured and refused (`PRETICK_ALIGNMENT_GUARDRAIL.md` §5). The family itself grew to
five rules the same way, each with an exact-set corpus pin.

**The honest framing for contribution 5.** Copper did not merely *rediscover* the
blocking/non-blocking distinction — it rediscovered it **three times**, in three different
disguises: `Out`-hold semantics, the multi-write collapse, and this. That is not a coincidence; it
is the predictable cost of the one thing Copper does that no comparison HDL does — **inferring** the
register/combinational boundary rather than declaring it. Every other system in the comparison
(MyHDL, Chisel, Amaranth, Spade, Bluespec, Clash) makes the current/next distinction syntactic and
so cannot express the hazard; Verilog can express it but lints it by comparing two author-written
declarations, which Copper does not have. Inference buys the ergonomics that are the headline claim,
and this is its bill. See `design_docs/PRETICK_ALIGNMENT_GUARDRAIL.md` §10 for the prior-art survey
and `SYNCHRONOUS_SEMANTICS.md` for the normative statement.

**T-align-2 — resolved 2026-08-21.** The companion divergence (a combinational passthrough of a
post-edge-produced signal lagging a cycle) was adjudicated against independent hand-written Verilog
and **fixed** rather than restricted: a read feeding a combinational `Out` in a segment that assigns
no register is now `Immediate`. The barrier was doing two jobs — deferring the read *and* pinning the
segment's phase — and only the second is ever needed. Note this one went the other way from
T-align: the fix was in the *simulator*, and it cost nothing (666/667 corpus, the single failure
being the test that pinned the old behaviour). Worth reporting as the counterweight: not every
member of this family has to be paid for with a restriction on the language.

## T7 — the same-source claim and `Memory` (added 2026-08-22; RESOLVED and re-scoped 2026-08-27)

**As written on 2026-08-22 this threat said `Memory` had no transpiled path at all.** True then,
false now: `Memory` transpiles — declared, preloaded, multi-port, both read-during-write modes,
and **received as a parameter** (the bus ABI, `design_docs/RECEIVED_MEMORY_ABI.md`) — and the
guarantees that were simulator-only are sweep-verified: `dual_port_ram` and the
latency/write-mode/arbitration fixtures (`tests/fixtures/pipelined_ram_dut.rs`,
`write_first_ram_dut.rs`) all run sim ≡ emitted-SV under Verilator `-Wall`. The closing
sentence of the old text — that the RISC-V CPU is not covered by the equivalence claim — is
retired in the strongest way available: the pipelined CPU transpiles and matches the simulator
cycle-for-cycle on 13 architectural programs (`tests/rv32i_pipelined_verilator.rs`; see
`00_claims_audit.md` §Re-verification). `arithmetic_shift_right` and the other constructs the
old list bundled here are likewise supported; the one operator gap left is `/`.

**The residual threats are narrower, and worth stating precisely:**

1. **A received memory's collision policy lives in the OWNER, not in the type.** `Memory<…>`
   carries no write mode (`.write_first()` is a runtime builder), so the transpiled child's bus
   is policy-agnostic and read-during-write semantics are whatever the instantiating owner
   wires. The CPU lane's owner implements WriteFirst to match its `build_memory`; an owner with
   the wrong policy would diverge silently. Today this is a documented ABI contract verified
   for the shipped designs, not a checked property of every instantiation.
2. **Out-of-range addressing still splits.** The simulator panics, naming the port, address and
   size; the synthesized side truncates the address to `clog2(depth)` bits via an explicit
   width cast. The split is confined to designs that have already failed in simulation, but it
   is a stated semantic divergence at the boundary, not an adjudicated equivalence.
3. **Memory's independent-anchor set is still thin.** The sweep proves sim ≡ own-SV
   *consistency*; the anchors that adjudicate memory *semantics* remain the hand-written
   references (`tests/verilog_fifo_memory_new.rs` is sim-vs-hand-written; `dual_port_ram`'s
   independent RAM template) plus, indirectly, the CPU's ISA-level programs. "The two agree"
   and "the two are right" stay distinct claims here, as in T6.

## T8 — X-propagation cannot be checked against the reference simulator (added 2026-08-22)

Copper's `Logic` is 3-state and `Bits<N>` carries X per bit, so the simulator models unknowns. That
modelling is **unverifiable against Verilator**, for two independent reasons either of which is
fatal alone:

1. **The X initialiser is dropped in transpilation.** `let mut r: Bits<8> = Bits::x()` emits a bare
   `logic [7:0] r;` with no initial value — the unknown is simply not represented in the generated
   SystemVerilog.
2. **Verilator is 2-state.** Its `--x-assign` / `--x-initial` flags *assign X away* to 0, 1, or a
   random value; they do not model X propagation. There is no X in the reference to compare
   against even in principle.

Measured: the simulator reports X where Verilator reports 0
(`tests/x_propagation_and_reset.rs::x_cannot_be_checked_against_verilator`, which pins the
divergence so it fails if either half is fixed). Closing this needs a 4-state reference simulator.

**A semantic difference worth declaring, not just an evidence gap.** X splits into two regimes in
Copper, and they behave differently:

- **Data is X-pessimistic** — X flows through the datapath and contaminates what it touches.
- **Control aborts** — `as_bool` / `as_uint` *panic* on X rather than propagating, so branching on
  an unknown stops the simulation instead of exploring both arms.

Four-state Verilog does neither: `if (x)` takes the else branch. Copper's choice is defensible for a
simulator (an unknown condition usually means the testbench is wrong, and failing loudly beats
silently taking a branch), but it is a deliberate divergence from the reference semantics and should
be stated rather than left for a reader to discover.

## T9 — the verification harness is itself part of the trusted base (added 2026-08-22)

Every equivalence claim in this paper rests on a harness that runs Verilator and compares traces.
That harness is code, and an audit of it found **five** defects, each of which could make a check
pass or vanish without anyone noticing. They are fixed, but the episode bounds how much weight the
evidence can bear and motivates the guards now in `tools/regression.sh`.

- **Verilator failures were swallowed.** The "should we skip?" test matched `err.contains("not
  found")`, and Verilator's C++ stage emits `fatal error: 'Vfoo.h' file not found` for a broken
  testbench — so a genuine build failure was reported as "Verilator not available" and the test
  **passed**.
- **Tests could verify against the wrong model.** Verilated output directories were keyed by module
  name, but `det_010_independent_golden.rs` runs two tests against the *same* top module in parallel
  threads; they shared a directory and clobbered each other's build. A false-*pass* mechanism, not
  merely a flake.
- **An anchor was green for the wrong reason.** `two_domain_hierarchy_cdc.rs` — the independent
  hardware check for the dual-clock design — passed because two silent sim≠SV divergences cancelled
  inside the chain. Only a deliberate experiment (correct one side, watch the boundary move) exposed
  it.
- **Most examples never ran.** Examples carry self-checks, several against third-party BaseJump
  Verilog, and `cargo test` only *builds* them. The default regression ran 5 of 26.
- **A disabled test's stated reason had gone stale.** `accum_2` was `#[ignore]`d as "sim and
  transpiler disagree by one cycle"; the divergence had since been fixed, but the claim was still
  being cited in design documents as a known limitation.

The pattern is one thing in five costumes: **a check that does not run looks exactly like a check
that passes.** The mitigation is structural — the regression driver now asserts that it ran what it
claims (every example registered and executed, every test file producing a binary), prints the
`#[ignore]`d list on every run, and preserves the log when it fails. The residual threat is the one
that cannot be designed away: our evidence is only as good as the machinery producing it, and that
machinery had bugs we found by looking rather than by being told.

