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
holds corpus-wide across 17+ sequential modules with codegen adding only its synthesized phase/pc
FSM counter — i.e. the inferred set is exactly the design's registers, with no rustc-style
over-capture.

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
