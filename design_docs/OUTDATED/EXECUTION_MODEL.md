# Copper Execution Model

This document describes the complete simulation execution model for Copper: how modules are written, how signals connect them, how the executor drives simulation forward, and how waveform data is produced automatically.

---

## Open Work Items

Items are ordered by dependency — earlier items unblock later ones.

---

### 1. Port Direction and Signal Ownership (`In<T>` / `Out<T>`)

- [ ] Add `Out<T, D>` to `copper-core`: non-`Clone`, wraps `Arc<Mutex<T>>` with a clock-domain phantom type, exposes only `write()`. Non-`Clone` enforces single-driver at compile time — you cannot accidentally give two modules the same `Out`.
- [ ] Add `In<T, D>` to `copper-core`: `#[derive(Clone)]`, same internals as `Out`, exposes only `read()`. Cloneable because multiple modules reading the same signal is valid.
- [ ] Add `wire::<T, D>()` constructor that allocates one `Arc<Mutex<T>>` and returns a matched `(Out<T, D>, In<T, D>)` pair. The only way to create a connected port pair, ensuring every driven signal has exactly one driver.
- [ ] Migrate all existing examples and tests from raw `Arc<Mutex<T>>` + `emit!` to `wire()` + `out.write()` / `in_.read()`.

**Implications.** `Out<T>` being non-`Clone` gives a compile-time single-driver guarantee. In Verilog, connecting two output drivers to the same wire produces undefined or X behavior that is only caught at simulation time. Here it is a type error. `In<T>` being `Clone` correctly models the fact that any number of modules can read the same signal. The `wire()` constructor is the only construction path, which means every `Out` in a design provably has a matching `In` and vice versa — dangling drivers and floating inputs are structurally impossible.

---

### 2. Clock Domain on Signal Types

- [ ] Add clock-domain phantom type `D` to `In<T, D>` and `Out<T, D>`. Currently only `Clock<D>` carries domain information; signals are domain-agnostic.
- [ ] Connecting an `Out<T, A>` to a module that expects `In<T, B>` (where `A ≠ B`) becomes a compile-time type mismatch.
- [ ] Allow crossing of clock domains only when explicit synchronization hardware is inserted between them (see item 3).

**Implications.** Clock-domain crossing (CDC) bugs are among the most common and hardest-to-find bugs in real RTL. Commercial linting tools (Synopsys SpyGlass, Cadence JasperGold) run dedicated CDC checks at elaboration time because the bug does not show up in functional simulation unless metastability is explicitly modeled. Encoding the domain in the signal type moves this check to the Rust compiler. A module in clock domain B that accidentally reads a signal driven in clock domain A produces a compile error rather than a silent metastability hazard.

---

### 3. Clock-Domain Crossing Synchronizer

- [ ] Implement `two_ff_sync` as a built-in CDC synchronizer module in `copper-core`. Signature: takes `In<T, A>` and `Clock<B>`, produces `In<T, B>`. This is the explicit proof to the type system that a CDC crossing is intentional and has synchronization hardware between the domains.
- [ ] `two_ff_sync` is the only sanctioned way to convert an `In<T, A>` into an `In<T, B>` when `A ≠ B`. Any direct crossing without it is a type error.

**Implications.** The two-flop synchronizer is the standard hardware primitive for safely crossing clock domains. By making it the only type-legal CDC path, the design language makes safe CDC the default and unsafe CDC structurally impossible. This mirrors what tools like Intel Quartus and Xilinx Vivado enforce with dedicated synchronizer primitives, but at the language level rather than the synthesis level.

---

### 4. Logic and Bits Types

- [ ] Use `Logic` for single-bit hardware signals. It is the single-bit hardware type in Copper and carries the three-state semantics needed for control signals that may become `X`.
- [ ] Add tests for the three-state logic system covering all base operations (`&`, `|`, `^`, `!`) with X inputs. The conservative propagation rules (`One & X = X`, `Zero & X = Zero`, `One | X = One`, `Zero | X = X`) must be verified exhaustively — all nine input combinations for each binary operation.
- [ ] Add tests for multi-bit operations on `Bits<N>`: arithmetic, bitwise, shifts, and X propagation through each. Pay particular attention to addition: `Bits<N>` addition is wrapping integer arithmetic, but what should happen when one or more input bits are X? The current implementation converts to `u128` via `as_u128` before adding, which silently drops X bits. The correct hardware behavior is that any X input bit produces an X output — the entire sum is unknown when any input bit is unknown.
- [ ] Decide on `Z` (high-impedance). IEEE 1364-2005 defines four states: `0`, `1`, `X`, `Z`. `Logic` does not currently include Z. Required for tri-state buses and bidirectional ports; not needed for purely synchronous FPGA-targeted designs. Tracked under item 6 (`MultiDriver`).

**Implications.** The multi-bit add problem is the most significant. Current `Bits<N>` arithmetic calls `as_u128()` which maps X to 0 before adding. This means `X + 1 = 1` in simulation, which is wrong — the correct result is X (unknown). Any test that relies on arithmetic results after X propagation may be silently producing incorrect simulation values. This needs to be fixed before the logic type is used for synthesis verification.

---

### 5. Module API and `#[hardware]` Annotation

- [ ] Resolve the distinction between `#[hardware]` and `#[hardware(function_typed)]`. With the `In<T>` / `Out<T>` port design, the output is an explicit `Out<T>` parameter rather than a return type, so `function_typed` (which enforces a return type) may no longer be the right distinction. Determine what validation each variant should perform and whether a single `#[hardware]` annotation suffices.
- [ ] Determine whether combinational inter-module tasks still need `delta_yield().await` explicitly, or whether the `#[hardware(combinational)]` macro can make it implicit. The current position is that the macro generates the loop and yield automatically — the user writes a plain synchronous function body. Confirm this is correct and implement it.
- [ ] Consult with Mark on the port API question: explicit `Out<T>` parameter vs return-value style. The current document describes both and the tradeoffs are clear (see Sections 4.3 and 4.4). This decision gates the module API migration.
- [ ] Once the port API is decided, migrate all examples and tests.

**Implications.** The `#[hardware]` vs `#[hardware(function_typed)]` distinction exists because the current API uses the return type to declare the output signal type — `function_typed` enforces this. With `Out<T>` as an explicit parameter, the return type becomes `()` for all modules, and the `function_typed` distinction becomes meaningless. The annotation may collapse to a single `#[hardware]` that validates the `In<T>` / `Out<T>` signature. This is a breaking API change for all existing modules.

---

### 6. Topological Task Ordering in the Executor

- [ ] Once `In<T>` / `Out<T>` connections are explicit at spawn time, build a dependency graph from the port connections and sort tasks topologically before the first `poll_tasks` call.
- [ ] With topological ordering, a combinational chain `A → B → C` converges in one delta cycle instead of three: A is polled first, writes its output; B is polled second, reads A's output and writes its own; C is polled third. No delta cycles needed.
- [ ] Tasks with no dependency ordering (parallel modules, sequential modules waiting on the clock) remain unordered among themselves.

**Implications.** The current executor polls all tasks in spawn order on every delta cycle, requiring as many delta cycles as the longest combinational path depth to converge. Topological ordering means most designs converge in a single pass, eliminating almost all delta cycles for acyclic combinational graphs. This also makes the simulation more deterministic — behavior no longer depends on spawn order.

---

### 7. Migrate Executor to Use the `futures` Crate ✓

- [x] Add `futures = { version = "0.3", default-features = false, features = ["executor"] }` to `copper-sim/Cargo.toml`.
- [x] Replace the hand-rolled `noop_waker()` in [`copper-sim/src/executor.rs`](../copper-sim/src/executor.rs) with [`futures::task::noop_waker()`](https://docs.rs/futures/0.3.32/futures/task/fn.noop_waker.html). This removes the only `unsafe` block in the executor.
- [ ] Optionally use `futures::future::BoxFuture<'static, ()>` as a type alias for `Pin<Box<dyn Future<Output = ()> + Send + 'static>>` in `TaskEntry`.

**What the `futures` crate cannot replace.** The executor's core loop — poll all tasks every delta cycle until no signal changes — is fundamentally incompatible with the standard async runtime model. `futures::executor::LocalPool` and `FuturesUnordered` are wake-driven: they only re-poll a task when its waker is called. Copper's simulation semantics require polling every task unconditionally on every delta cycle, because any signal change can cascade to any downstream task and the executor has no dependency graph to consult (until item 6 above is implemented). Replacing `poll_tasks` with `LocalPool` would break convergence — tasks would be polled once and then stuck, because the noop waker never signals readiness. `FuturesUnordered` has the same problem. The `futures` crate has no primitive that matches the fixed-point iteration model.

**Implications.** The only practical gains from the `futures` crate are the removal of the `unsafe` block and the `BoxFuture` type alias. The `unsafe` removal is the meaningful one: the current `RawWaker` / `RawWakerVTable` construction requires the caller to manually uphold a safety contract that the Rust compiler cannot verify. `futures::task::noop_waker()` provides the same semantics with a safe API, verified by the `futures` crate's own test suite. This is a low-risk, zero-behavior-change improvement that eliminates the only unsafety in the simulation engine.

---

### 8. Multi-Driver Buses (Lower Priority)

- [ ] Design `MultiDriver<T, R>` for intentional multi-driver buses: tri-state, wired-AND, wired-OR, open-drain. This is the explicit opt-out from the single-driver guarantee that `Out<T>` provides.
- [ ] Requires `Z` (high-impedance) in the `Logic` type — a floating bus whose drivers are all disabled should resolve to Z, and Z combined with driven values should resolve according to the bus topology.
- [ ] Design the resolution function API: callers specify how multiple drivers combine (wired-AND, wired-OR, tri-state resolution).

**Implications.** This unblocks modeling of I2C, open-drain buses, and any design that intentionally has multiple drivers on a net. It is lower priority because purely synchronous FPGA-targeted designs rarely need it — FPGA routing is point-to-point, not wired-logic. It requires Z in the logic type (item 4) and is a deliberate escape hatch from the single-driver type safety that `Out<T>` provides.

---

## Table of Contents

1. [Logic Types](#1-logic-types)
2. [Clock Domains and the Clock Primitive](#2-clock-domains-and-the-clock-primitive)
3. [Writing Modules](#3-writing-modules)
   - [Sequential Modules](#31-sequential-modules)
   - [Combinational Logic](#32-combinational-logic)
     - [Intra-Module: Plain Rust Functions](#intra-module-plain-rust-functions)
     - [Inter-Module: Async Combinational Tasks](#inter-module-async-combinational-tasks)
     - [Planned: #\[hardware(combinational)\]](#planned-hardwarecombinational)
   - [Internal State](#33-internal-state)
4. [Signal Registry and SignalHandle](#4-signal-registry-and-signalhandle)
   - [The Registry](#41-the-registry)
   - [SignalHandle](#42-signalhandle)
   - [Inter-Module Connections](#43-inter-module-connections)
   - [Port Direction Types: In\<T\> and Out\<T\>](#44-port-direction-types-int-and-outt)
5. [The Emit Machinery](#5-the-emit-machinery)
   - [Thread-Local Emit Target](#51-thread-local-emit-target)
   - [Value-Change Detection](#52-value-change-detection)
   - [The emit! Macro](#53-the-emit-macro)
6. [The Executor](#6-the-executor)
   - [Task Representation](#61-task-representation)
   - [Spawning Modules](#62-spawning-modules)
   - [Module Hierarchy](#63-module-hierarchy)
7. [The Delta-Cycle Convergence Loop](#7-the-delta-cycle-convergence-loop)
   - [What a Delta Cycle Is](#71-what-a-delta-cycle-is)
   - [poll_tasks()](#72-poll_tasks)
   - [Convergence Condition](#73-convergence-condition)
   - [Oscillation Detection](#74-oscillation-detection)
   - [X Propagation](#75-x-propagation)
8. [tick_clock(): A Full Clock Cycle](#8-tick_clock-a-full-clock-cycle)
9. [Memory](#9-memory)
10. [Automatic Waveform Tracing](#10-automatic-waveform-tracing)
11. [The Noop Waker](#11-the-noop-waker)

---

## 1. Logic Types

All signal values in Copper are built on a three-state logic system. The base type is:

```rust
// copper-core/src/types.rs
pub enum Logic {
    Zero,
    One,
    X,   // unknown / indeterminate
}
```

IEEE 1364-2005 (Verilog) defines a fourth state, `Z` (high-impedance), representing a net that is not being driven — a floating wire, a disabled tri-state buffer, or a bus with all drivers turned off. Copper does not currently implement `Z`. This is a known gap: `Logic` currently models only `0`, `1`, and `X`. For most FPGA-targeted synchronous digital logic this does not matter — FPGA fabrics do not expose tri-state routing internally, only at I/O pads — but designs involving bidirectional buses or tri-state buffers cannot be modeled correctly without it.

`X` is not an error state — it is a valid simulation value meaning "could be zero or one." It propagates through combinational logic according to conservative rules: `One & X = X`, `Zero & X = Zero`, `One | X = One`, `Zero | X = X`. This matches the behavior of commercial Verilog simulators for the unknown state.

`Logic` is the single-bit hardware type. It carries the three-state semantics needed for control signals that may become `X`, and it is the right choice for 1-bit hardware ports, enables, flags, and comparators.

`Bits<N>` is a compile-time fixed-width bit vector backed by `[Logic; N]`. Width is encoded in the type, so a `Bits<8>` and a `Bits<16>` are distinct types and cannot be connected by accident. Arithmetic operations (`+`, `-`, `&`, `|`, `^`, `!`, shifts) are defined on `Bits<N>` with wrapping semantics and X propagation. Use it for counters, addresses, buses, ALU results, and any other data path that should have an explicit bit width.

See [`copper-core/src/types.rs`](../copper-core/src/types.rs) for the full set of trait implementations.

### HasUnknown

The `HasUnknown` trait marks types that have a defined X state:

```rust
// copper-core/src/types.rs:318
pub trait HasUnknown {
    fn unknown() -> Self;
}
```

Implemented for `Logic`, `Bits<N>`, and tuples of those up to four elements. The executor uses this to inject X into an oscillating combinational loop rather than panicking — see [Section 7.5](#75-x-propagation).

---

## 2. Clock Domains and the Clock Primitive

Every clock in Copper has a phantom domain type, enforced at compile time:

```rust
// copper-core/src/types.rs:366
pub trait ClockDomain: 'static {}
```

Users define a domain by declaring a unit struct and implementing the trait:

```rust
struct MainClk;
impl ClockDomain for MainClk {}
```

Two clocks with different domain types are incompatible at the type level. A module parameterized on `Clock<MainClk>` cannot accept a `Clock<PeriphClk>` — the compiler rejects it. This catches clock-domain crossing bugs at compile time rather than at simulation runtime.

### Clock\<Domain\>

`Clock<Domain>` holds shared mutable state behind `Arc<Mutex<ClockState>>`:

```rust
// copper-core/src/types.rs:368
struct ClockState {
    cycle:     u64,
    wakers:    Vec<Waker>,
    listeners: Vec<Weak<dyn ClockEdgeListener>>,
}
```

- `cycle` — the current simulation cycle count
- `wakers` — tasks that have called `clk.tick().await` and are waiting for the next edge
- `listeners` — objects (e.g. `Memory`) that need to be notified on every positive edge

`Clock<Domain>` implements `Clone`: clones share the same `Arc`, so all copies of a clock observe the same cycle counter and can all suspend on the same edge.

### tick()

`Clock::tick()` captures the current cycle and returns a `ClockTick` future:

```rust
// copper-core/src/types.rs:437
pub fn tick(&self) -> ClockTick<Domain> {
    let target = self.cycle().wrapping_add(1);
    ClockTick { state: Arc::clone(&self.state), target_cycle: target, _domain: PhantomData }
}
```

`ClockTick` implements `Future`. When polled:
- If `state.cycle >= target_cycle` → returns `Poll::Ready(())`
- Otherwise → pushes the waker onto `state.wakers` and returns `Poll::Pending`

Because the executor uses a noop waker (see [Section 11](#11-the-noop-waker)), the waker push is effectively a no-op for scheduling purposes. The executor does not rely on wakers to know when to re-poll tasks — it polls everything on every delta cycle. The wakers are drained on each `advance()` call.

### advance()

The executor calls `clk.advance()` once per simulated clock cycle, between the pre-edge and post-edge settle phases:

```rust
// copper-core/src/types.rs:404
pub fn advance(&mut self) {
    let mut state = self.state.lock().unwrap();
    state.cycle += 1;
    state.listeners.retain(|weak| {
        weak.upgrade().map(|l| { l.on_posedge(); true }).unwrap_or(false)
    });
    let wakers = std::mem::take(&mut state.wakers);
    drop(state);
    for w in wakers { w.wake(); }
}
```

`advance()` increments the cycle, fires `on_posedge` on all registered listeners (used by `Memory`), then drains and calls all pending wakers. After this call, any `ClockTick` future whose `target_cycle` equals the new cycle value will return `Poll::Ready` on its next poll.

---

## 3. Writing Modules

A Copper module is an `async fn` that runs as a single long-lived future for the entire simulation. It is marked with `#[hardware]` or `#[hardware(function_typed)]`.

The `#[hardware(function_typed)]` variant enforces that:
- The function has an explicit non-unit return type (the output signal type)
- All input parameters are read-only (no `mut`, no `&mut`)

See [`copper-macros/src/lib.rs`](../copper-macros/src/lib.rs) for the validation logic.

### 3.1 Sequential Modules

A sequential module computes outputs, writes them, then suspends until the next clock edge. Its local variables persist across the suspension — they are the module's registers.

```rust
#[hardware]
async fn counter(clk: Clock<MainClk>, out: Out<Bits<8>>) {
    let mut count = Bits::<8>::from_u128(0);
    loop {
        out.write(count);
        clk.tick().await;
        count = count + Bits::from_u128(1);
    }
}
```

The control flow here encodes timing directly:
- `out.write(count)` — drive the output with the current register value (pre-edge)
- `clk.tick().await` — suspend. The executor polls this point many times during the pre-edge settle phase. Each poll returns `Poll::Pending` until `advance()` is called.
- `count = count + 1` — update the register after the edge (post-edge)
- Loop back to `out.write` — the updated value is visible on the next cycle

Multiple `clk.tick().await` calls in a single loop body model multi-cycle behavior naturally. Each `.await` is a separate clock edge boundary. Intermediate local variables between two `.await` points are pipeline registers: they are stored in the generated `Future` struct and survive across polls.

```rust
async fn two_stage(clk: Clock<MainClk>, input: SignalHandle<Bits<32>>) -> Bits<32> {
    loop {
        let a   = read_signal(input);       // not a register: consumed before .await
        let mid = expensive_stage_one(a);   // register: lives across the first .await
        clk.tick().await;
        let result = stage_two(mid);        // not a register: consumed before .await
        emit!(result);
        clk.tick().await;
    }
}
```

The rule is: only variables that are **live across an `.await` point** become fields in the generated future struct. `a` is consumed by `expensive_stage_one` before the first `.await` — the compiler does not need to store it. `mid` is assigned before the first `.await` and read after it, so it is stored as a struct field — a pipeline register. `result` is assigned and consumed between the two `.await` points, so it is also just a temporary.

This is a 2-cycle latency pipeline, written as straight-line code. No explicit state enum, no register declarations.

### 3.2 Combinational Logic

Combinational logic has two distinct forms in Copper depending on where it lives.

#### Intra-Module: Plain Rust Functions

The primary way to write combinational logic is as a plain synchronous Rust function called from within a sequential module's loop body. The function is re-evaluated every time the loop body runs — once per clock edge — from the current values of whatever arguments are passed to it. This is exactly the semantics of combinational logic: a pure function of its inputs.

```rust
fn alu(a: Bits<8>, b: Bits<8>, op: AluOp) -> Bits<8> {
    match op {
        AluOp::Add => a + b,
        AluOp::And => a & b,
        AluOp::Or  => a | b,
    }
}

#[hardware(function_typed)]
async fn cpu(clk: Clock<MainClk>, op: SignalHandle<AluOp>, ...) -> Bits<8> {
    let mut reg_a = Bits::from_u128(0);
    let mut reg_b = Bits::from_u128(0);
    loop {
        let result = alu(reg_a, reg_b, read_signal(op));  // combinational, called inline
        emit!(result);
        clk.tick().await;
        // update reg_a, reg_b from inputs
    }
}
```

No executor involvement, no signal handles, no `delta_yield`. The function is called synchronously as part of the sequential module's computation. This is the correct and preferred approach for all combinational logic that is internal to a module.

#### Inter-Module: Async Combinational Tasks

When combinational logic needs to be a **separate named module** — connecting two signals in the registry, appearing in the waveform trace, or reused across multiple parent modules — it is written as an async task with a `loop` and `delta_yield().await`.

```rust
#[hardware]
async fn and_gate(a: In<Logic>, b: In<Logic>, out: Out<Logic>) {
    loop {
        out.write(a.read() & b.read());
        delta_yield().await;
    }
}
```

`DeltaYield` is a minimal future defined in [`copper-sim/src/lib.rs:98`](../copper-sim/src/lib.rs#L98):

```rust
pub struct DeltaYield { yielded: bool }

impl Future for DeltaYield {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.yielded { Poll::Ready(()) }
        else { self.yielded = true; Poll::Pending }
    }
}
```

On the first poll it returns `Pending`, letting all other tasks run in the current delta cycle. On the second poll it returns `Ready` and the loop body re-executes. This gives the module one re-evaluation per delta cycle, which is the correct semantics for combinational logic at the task level.

`delta_yield().await` is required for two reasons. Without any `.await`, the loop spins forever inside a single `poll` call and the executor never regains control. Without a loop, the future completes after one evaluation and the output is never updated when inputs change.

A combinational task must have **exactly one `delta_yield().await` per loop iteration, at the end, after `out.write`**. Multiple yields in one iteration introduce artificial delta-cycle latency: the output in delta cycle N would be computed from the input read in delta cycle N-1, which is not combinational behavior. Any variable read before the first yield and used after it would also be stored in the generated future struct — effectively a register — which is incorrect for purely combinational logic.

`clk.tick().await` must not appear in a combinational task. It would suspend the module until the next clock edge, making the output stale for the entire settle phase.

#### Planned: #\[hardware(combinational)\]

The inter-module async pattern has necessary but mechanical boilerplate: the `loop`, `out.write`, and `delta_yield().await`. A planned `#[hardware(combinational)]` attribute will eliminate this by allowing the user to write a plain synchronous function and having the macro generate the async wrapper:

```rust
// what the user writes
#[hardware(combinational)]
fn and_gate(a: In<Logic>, b: In<Logic>, out: Out<Logic>) {
    out.write(a.read() & b.read())
}

// what the macro generates
async fn and_gate(a: In<Logic>, b: In<Logic>, out: Out<Logic>) {
    loop {
        out.write(a.read() & b.read());
        delta_yield().await;
    }
}
```

The macro will validate that:
- The function is not `async` (the macro makes it async)
- No `clk` parameter is present (combinational modules have no clock)

### 3.3 Internal State

Local variables in a module body that are not emitted to any output signal are internal state. They exist entirely inside the `Future` struct generated by the compiler for the `async fn`. The executor never interacts with them directly.

```rust
#[hardware]
async fn fifo(clk: Clock<MainClk>, write_en: In<Logic>, data_in: In<Bits<8>>, out: Out<(Bits<8>, Logic, Logic, Logic)>) {
    let mut buffer: Vec<Bits<8>> = ...;
    let mut read_ptr: usize = 0;
    let mut write_ptr: usize = 0;
    let mut count: usize = 0;
    loop {
        // read_ptr, write_ptr, count, buffer are all internal registers —
        // they survive across clk.tick().await in the generated Future struct
        out.write((dout, empty, full, valid));
        clk.tick().await;
        // update internal state here
    }
}
```

The Rust compiler's async transformation stores every variable that is live across an `.await` point as a field in the generated state machine struct. This is precisely the semantics of a hardware register: a value stored between clock edges. No annotation or declaration is needed — `let mut` is sufficient.

Internal variables that a developer wants visible in waveform output can be optionally registered in the signal store:

```rust
let count_sig = exec.register_signal("fifo.count", 0usize);
// reads: exec.read(count_sig)
// writes: exec.drive(count_sig, new_val)
```

By default they are invisible to the registry and not traced.

---

## 4. Signal Registry and SignalHandle

### 4.1 The Registry

The signal registry (`SignalStore`) is a contiguous collection of typed signal slots owned by the executor. Every named signal in the design — module output ports and any explicitly registered internal signals — has one slot.

Each slot holds:
- A string name (for VCD export and debugging)
- The current value (type-erased as `Box<dyn ErasedSignal>`)
- A dirty flag indicating whether the value changed during the most recent executor pass
- A history of `(cycle, value)` pairs recorded automatically at the end of each `tick_clock`

The `ErasedSignal` trait provides the interface the executor needs without knowing the concrete type:

```rust
trait ErasedSignal: Any {
    fn name(&self) -> &str;
    fn width(&self) -> usize;          // bit width, for VCD
    fn is_dirty(&self) -> bool;
    fn clear_dirty(&mut self);
    fn record_current(&mut self, cycle: u64);  // append to history
    fn vcd_changes(&self) -> &[(u64, Vec<Logic>)];
}
```

Typed access is recovered through the `SignalHandle`'s index combined with `Any` downcasting.

The store lives in a thread-local so that module futures can read and write signals without holding a reference to the executor. This is the same pattern as the existing emit machinery. The executor initializes the thread-local at startup and accesses it directly during `poll_tasks`.

### 4.2 SignalHandle

A `SignalHandle<T>` is a typed index into the registry:

```rust
pub struct SignalHandle<T> {
    index: usize,
    _phantom: PhantomData<T>,
}

impl<T> Copy  for SignalHandle<T> {}
impl<T> Clone for SignalHandle<T> { fn clone(&self) -> Self { *self } }
```

It carries no ownership, no reference count, and no allocation. It is `Copy`, so modules accept it by value with no cloning. Passing `write_en: SignalHandle<Logic>` or `data_in: SignalHandle<Bits<8>>` to a module costs nothing more than passing a `usize`.

The phantom type `PhantomData<T>` ensures that you cannot read a `SignalHandle<Logic>` slot as `SignalHandle<Bits<8>>` — the type mismatch is a compile error, not a runtime panic.

Two free functions provide signal access from within module bodies:

```rust
pub fn read_signal<T: 'static>(handle: SignalHandle<T>) -> T { ... }
pub fn write_signal<T: 'static>(handle: SignalHandle<T>, value: T) { ... }
```

Both go through the thread-local signal store, following the same pattern as `emit_to_current`.

### 4.3 Inter-Module Connections

Every signal — whether driven by a module or by the testbench — is registered in the store and addressed by a `SignalHandle<T>`. Direction is communicated to modules through `In<T>` and `Out<T>` wrappers (see [Section 4.4](#44-port-direction-types-int-and-outt)).

**Input signals** are registered by the testbench or parent module and passed into child modules wrapped in `In<T>`. Because `SignalHandle<T>` is `Copy`, no cloning is needed:

```rust
let write_en = exec.register_signal("write_en", Logic::Zero);
let read_en  = exec.register_signal("read_en",  Logic::Zero);
let data_in  = exec.register_signal("data_in",  Bits::<8>::from_lit::<0>());
let fifo_out = exec.register_signal("fifo_out", initial);

exec.spawn(fifo(clk.clone(), In::new(write_en), In::new(read_en), In::new(data_in), Out::new(fifo_out)));
```

The testbench drives input signals between ticks and reads outputs directly via the handle:

```rust
exec.drive(write_en, Logic::One);
exec.drive(data_in,  Bits::from_lit::<42>());
exec.tick_clock(&mut clk);
let result = exec.read(fifo_out);
```

**Connecting modules** — the output of one module becomes the input of the next by passing the same `SignalHandle<T>` to both, wrapped in the appropriate direction type:

```rust
let stage1_out = exec.register_signal("stage1_out", 0u8);
let stage2_out = exec.register_signal("stage2_out", 0u8);

exec.spawn(stage1(clk.clone(), In::new(input), Out::new(stage1_out)));
exec.spawn(stage2(clk.clone(), In::new(stage1_out), Out::new(stage2_out)));
```

`stage1_out` is written by `stage1` via `Out::write` and read by `stage2` via `In::read`. The executor's delta-cycle convergence loop ensures that `stage2` sees `stage1`'s output from the same delta cycle before declaring a fixed point.

### 4.4 Port Direction Types: In\<T\> and Out\<T\>

A bare `SignalHandle<T>` carries no information about whether a port is an input or an output. A module that takes three `SignalHandle<Bits<8>>` parameters gives no indication from the signature alone of which are read and which are written. A reader must inspect the body to determine data flow direction.

`In<T>` and `Out<T>` are thin wrappers around `SignalHandle<T>` that encode direction in the type:

```rust
pub struct In<T>  { handle: SignalHandle<T> }
pub struct Out<T> { handle: SignalHandle<T> }

impl<T: Clone + 'static> In<T> {
    pub fn new(handle: SignalHandle<T>) -> Self { Self { handle } }
    pub fn read(&self) -> T { read_signal(self.handle) }
}

impl<T: PartialEq + 'static> Out<T> {
    pub fn new(handle: SignalHandle<T>) -> Self { Self { handle } }
    pub fn write(&self, value: T) { write_signal(self.handle, value) }
}
```

`In<T>` exposes only `read`. `Out<T>` exposes only `write`. The type system enforces direction — calling `write` on an input or `read` on an output is a compile error. Both are `Copy` (wrapping a `Copy` handle).

A module signature with direction types is self-describing:

```rust
async fn alu(
    a:   In<Bits<8>>,
    b:   In<Bits<8>>,
    op:  In<AluOp>,
    out: Out<Bits<8>>,
    clk: Clock<MainClk>,
) { ... }
```

The data flow is fully visible without reading the body. This follows the same convention as VHDL's `port (a : in std_logic; b : out std_logic)` and SystemC's `sc_in<T>` / `sc_out<T>`.

**Eliminating the thread-local.** `Out<T>.write` calls `write_signal` directly with the handle it carries. No thread-local binding is needed — the output target is explicit in the type. This removes both `CURRENT_EMIT_TARGET` and the `push_emit_target` / guard machinery from the executor. After polling a task, the executor checks the signal slot's dirty flag directly rather than reading `CURRENT_EMIT_DIRTY`. The entire emit thread-local mechanism described in [Section 5](#5-the-emit-machinery) is replaced by direct indexed writes into the signal store.

**The underlying handle is the same.** `In<T>` and `Out<T>` both wrap a `SignalHandle<T>`. Connecting the output of one module to the input of another is done by passing the same underlying handle wrapped in the appropriate direction type — no registry lookup, no string matching, no indirection beyond the index.

---

## 5. The Emit Machinery

> **Note:** This section describes the current implementation. With the `In<T>` / `Out<T>` port direction design (Section 4.4), the thread-local binding is eliminated entirely — `Out<T>.write` writes directly to the signal store via the handle it carries, and the executor checks the signal slot's dirty flag after each poll instead of reading `CURRENT_EMIT_DIRTY`.

### 5.1 Thread-Local Emit Target

The executor binds each task to its output signal slot before polling it. This binding is stored in a thread-local so that `emit!` inside the task body can write to the correct slot without any reference to the executor:

```rust
// copper-sim/src/lib.rs:20
thread_local! {
    static CURRENT_EMIT_TARGET: RefCell<Option<Arc<dyn Any + Send + Sync>>> = RefCell::new(None);
    static CURRENT_EMIT_DIRTY:  RefCell<bool> = RefCell::new(false);
}
```

In the registry design, `CURRENT_EMIT_TARGET` becomes a signal slot index rather than a raw `Arc`. The dirty flag thread-local is unchanged.

Before polling a task, the executor calls `push_emit_target`, which:
1. Clears `CURRENT_EMIT_DIRTY` to false (fresh start for this poll)
2. Stores the task's output signal index (or `None` for tasks without a typed output)
3. Returns a guard that restores the previous value on drop

```rust
// copper-sim/src/lib.rs:41
pub(crate) fn push_emit_target(target: Option<...>) -> EmitTargetGuard { ... }
```

After polling, the executor calls `take_emit_dirty()` to retrieve and reset the dirty flag:

```rust
// copper-sim/src/lib.rs:54
pub(crate) fn take_emit_dirty() -> bool {
    CURRENT_EMIT_DIRTY.with(|cell| { let d = *cell.borrow(); *cell.borrow_mut() = false; d })
}
```

### 5.2 Value-Change Detection

`emit_to_current<T>` writes to the bound signal slot and sets the dirty flag **only if the new value differs from the stored value**:

```rust
// copper-sim/src/lib.rs:62
pub fn emit_to_current<T: PartialEq + Send + 'static>(value: T) {
    // ... access CURRENT_EMIT_TARGET, downcast to Mutex<T> ...
    let mut guard = typed.lock().unwrap();
    if *guard != value {
        *guard = value;
        CURRENT_EMIT_DIRTY.with(|cell| *cell.borrow_mut() = true);
    }
}
```

This is critical for convergence. A module that unconditionally calls `emit!` with the same value every poll — which is common and correct — will not set the dirty flag if the value hasn't changed. This allows the delta-cycle loop to detect a fixed point even when every task is emitting on every pass. Without value-change detection, any unconditional `emit!` would prevent convergence.

### 5.3 The emit! Macro

`emit!` is a thin wrapper over `emit_to_current`:

```rust
// copper-sim/src/lib.rs:136
macro_rules! emit {
    ($value:expr) => { $crate::emit_to_current($value); }
}
```

It can only be called from within a module body during an executor poll. Calling it outside a task (when no emit target is bound) panics immediately with a descriptive message. Calling it with the wrong type panics on the `Any` downcast.

---

## 6. The Executor

### 6.1 Task Representation

Each spawned module is stored as a `TaskEntry`:

```rust
// copper-sim/src/executor.rs:35
struct TaskEntry {
    future:           Pin<Box<dyn Future<Output = ()>>>,
    emit_target:      Option<Arc<dyn Any + Send + Sync>>,
    set_unknown:      Option<Box<dyn Fn() + Send + Sync>>,
    consecutive_dirty: usize,
}
```

- `future` — the module's boxed, pinned future. Boxing allows heterogeneous futures in one `Vec`; pinning satisfies the requirement that a `Future`'s memory address not move while it is being polled. See [`std::pin`](https://doc.rust-lang.org/std/pin/index.html) for the full contract.
- `emit_target` — the signal slot this task writes to via `emit!`. `None` for tasks without a typed output.
- `set_unknown` — a closure that writes `T::unknown()` to the output signal. Present only for tasks spawned with `_with_unknown` variants. Used during oscillation resolution.
- `consecutive_dirty` — how many consecutive delta cycles this task's output has changed. Used to detect combinational loops.

### 6.2 Spawning Modules

The executor provides several spawn methods that register a module's future and, optionally, create and register its output signal:

| Method | Output signal | X propagation |
|--------|--------------|---------------|
| `spawn` | none | no |
| `spawn_function_typed` | creates + returns `SignalHandle<T>` | no |
| `spawn_function_typed_with_unknown` | creates + returns `SignalHandle<T>` | yes |
| `spawn_into_with_unknown` | caller provides handle (self-feedback) | yes |
| `spawn_child_*` variants | same as above + hierarchy tracking | — |

`spawn_into_with_unknown` is for self-feedback circuits — a module whose output feeds back to its own input. The caller allocates the signal handle and passes it both to the module and to `spawn_into_with_unknown`, so `emit!` writes to the same slot the module reads from.

### 6.3 Module Hierarchy

Each module can be registered with a parent name. The executor maintains a `HashMap<String, ModuleInfo>` tracking name, parent, and children:

```rust
// copper-sim/src/executor.rs:51
pub struct ModuleInfo {
    pub name:     String,
    pub parent:   Option<String>,
    pub children: Vec<String>,
}
```

`spawn_child_function_typed` (and its variants) call `ensure_module` for both parent and child, then link them bidirectionally. This hierarchy is available via `exec.module_info(name)` and `exec.module_infos()` for debugging and visualization purposes.

---

## 7. The Delta-Cycle Convergence Loop

### 7.1 What a Delta Cycle Is

A delta cycle is a zero-time simulation step in which signal values are allowed to propagate through combinational logic. The term comes from the IEEE 1364-2005 Verilog standard (section 5.4.1), where it describes the mechanism by which a simulator resolves the order of events that are scheduled at the same simulation time.

In Copper, one delta cycle = one complete pass over all tasks. The executor runs as many delta cycles as needed until no signal value changes — this is the fixed point. Only then does simulation time actually advance.

### 7.2 poll_tasks()

`poll_tasks` is the core simulation method. It runs the delta-cycle convergence loop for one settle phase (either pre-edge or post-edge):

```rust
// copper-sim/src/executor.rs:355
pub fn poll_tasks(&mut self) {
    const OSCILLATION_THRESHOLD: usize = 20;
    const MAX_DELTA_CYCLES:      usize = 1000;

    let waker   = noop_waker();
    let mut ctx = Context::from_waker(&waker);

    for delta in 0..=MAX_DELTA_CYCLES {
        assert!(delta < MAX_DELTA_CYCLES, "Delta-cycle limit exceeded ...");

        let mut any_dirty = false;
        for task in &mut self.tasks {
            let _guard = crate::push_emit_target(task.emit_target.clone());
            let _ = task.future.as_mut().poll(&mut ctx);
            if crate::take_emit_dirty() {
                task.consecutive_dirty += 1;
                any_dirty = true;
            } else {
                task.consecutive_dirty = 0;
            }
        }

        if !any_dirty {
            for task in &mut self.tasks { task.consecutive_dirty = 0; }
            break;
        }

        // oscillation detection and X injection (see Section 7.4 and 7.5)
        ...
    }
}
```

Each pass of the outer loop is one delta cycle. For each task in each delta cycle:

1. `push_emit_target` binds the task's output signal and clears the dirty flag
2. `task.future.as_mut().poll(&mut ctx)` polls the future once
3. `take_emit_dirty()` checks whether `emit!` changed the signal value
4. `consecutive_dirty` is incremented if dirty, reset to zero otherwise

After all tasks are polled, if `any_dirty` is false — no task changed any signal — the loop terminates. The circuit has reached a fixed point.

### 7.3 Convergence Condition

A circuit converges when a complete pass over all tasks produces no signal value changes. For acyclic combinational logic, convergence is guaranteed within a number of delta cycles equal to the longest combinational path depth. A chain `A → B → C` converges in three delta cycles:
- Delta 1: A emits (dirty), B and C have stale inputs
- Delta 2: B sees A's new value, emits (dirty); C still stale
- Delta 3: C sees B's new value, emits (dirty); nothing new
- Delta 4: No changes — fixed point

Purely sequential modules (all state behind `clk.tick().await`) converge in one delta cycle per settle phase: the module body executes once, emits, and then returns `Poll::Pending` on all subsequent polls within the same phase.

### 7.4 Oscillation Detection

A combinational loop — module A's output is an input to A itself, or a cycle through multiple modules — causes `consecutive_dirty` to grow on every pass without ever reaching zero. The executor tracks this per task.

```rust
// copper-sim/src/executor.rs:364
const OSCILLATION_THRESHOLD: usize = 20;
```

A threshold of 20 is generous for typical RTL fan-in depths (a signal in an acyclic graph can only be dirty for as many delta cycles as there are independent input chains feeding into it), while catching genuine loops well before `MAX_DELTA_CYCLES`.

When a task's `consecutive_dirty` reaches `OSCILLATION_THRESHOLD`:
- If `set_unknown` is present → call it (inject X), reset `consecutive_dirty`, continue
- If `set_unknown` is absent → panic with a message pointing to the task index and the `_with_unknown` spawn variant

### 7.5 X Propagation

When `set_unknown` is called, it writes `T::unknown()` to the oscillating task's output signal via the closure captured at spawn time:

```rust
// copper-sim/src/executor.rs:141
let set_unknown: Box<dyn Fn() + Send + Sync> =
    Box::new(move || *output_for_x.lock().unwrap() = T::unknown());
```

With the output now `X`, downstream combinational modules that read it will compute `X`-valued results (because `X` propagates through all `Logic` operations). Those downstream modules emit `X`, which equals the stored `X`, so the dirty flag is not set. The loop terminates.

This matches the behavior specified in IEEE 1364-2005 section 5.3: a combinational feedback loop with no initialization resolves to the unknown state in Verilog simulation.

---

## 8. tick_clock(): A Full Clock Cycle

`tick_clock` is the primary simulation API. One call advances simulation by exactly one clock cycle through three sequential phases:

```rust
// copper-sim/src/executor.rs:437
pub fn tick_clock<Domain: ClockDomain>(&mut self, clk: &mut Clock<Domain>) {
    self.poll_tasks();   // phase 1: pre-edge settle
    clk.advance();       // phase 2: clock edge
    self.poll_tasks();   // phase 3: post-edge settle
    self.cycle += 1;
}
```

**Phase 1 — Pre-edge settle.** The executor runs the delta-cycle convergence loop. Combinational modules re-evaluate from the current input values and reach a fixed point. Sequential modules are suspended at `clk.tick().await`, which returns `Poll::Pending` because the clock has not yet advanced. They contribute no changes. The result is a stable set of signal values representing the circuit state just before the rising edge.

**Phase 2 — Clock edge.** `clk.advance()` increments the cycle counter on the shared `ClockState`. Every `ClockTick` future whose `target_cycle` equals the new cycle value will now return `Poll::Ready` on its next poll. `on_posedge` is fired on all registered `ClockEdgeListener`s (used by `Memory` to advance its internal pipelines). The pending waker list is drained.

**Phase 3 — Post-edge settle.** The executor runs the delta-cycle convergence loop again. Now, when a sequential module's future is polled and it is suspended at `clk.tick().await`, the `ClockTick` future sees `cycle >= target_cycle` and returns `Poll::Ready(())`. Control returns to the module's loop body immediately after the `.await`. The module reads its inputs, updates internal state, emits its new output, and suspends at the next `clk.tick().await`. This new `ClockTick` has `target_cycle = current + 1`, which is not yet satisfied, so subsequent polls within this settle phase return `Poll::Pending`. Downstream combinational logic then settles from the new sequential outputs.

The combination of these three phases correctly models the behavior of synchronous digital logic: combinational paths settle before the edge, flip-flops sample and update on the edge, and the new register outputs propagate through combinational paths after the edge.

---

## 9. Memory

`Memory<T, R, W, D, READ_LAT, WRITE_LAT>` models a multi-port synchronous RAM with configurable port counts and pipeline latency. It is defined in [`copper-core/src/memory.rs`](../copper-core/src/memory.rs).

```rust
pub struct Memory<T, const R: usize, const W: usize, D: ClockDomain,
                  const READ_LAT: usize = 1, const WRITE_LAT: usize = 1> {
    shared: Arc<MemoryShared<T, R, W, READ_LAT, WRITE_LAT>>,
    _clock: Clock<D>,
    read_mode: ReadMode,
}
```

`Memory` registers itself as a `ClockEdgeListener` with the clock at construction time. On every `on_posedge` call (fired by `clk.advance()`), it:
1. Advances the write pipeline: pending writes shift toward commit, and the oldest in-flight write is applied to the data array
2. Advances the read pipeline: captures pending read addresses into the pipeline, shifts valid data toward the output

`READ_LAT` and `WRITE_LAT` encode the number of cycles between a request and a valid response. `ReadPort::is_ready` tells the caller whether the output pipeline currently holds valid data.

`WriteMode::ReadFirst` and `WriteMode::WriteFirst` control the behavior when a read and write target the same address in the same cycle:
- `ReadFirst` — the read captures the value before the write commits (matches most FPGA block RAM defaults)
- `WriteFirst` — the write commits first and the read sees the new value

Because `Memory` updates itself through `on_posedge` rather than through `emit!`, it does not participate in the dirty-flag convergence loop. Its state changes are atomic with the clock edge.

---

## 10. Automatic Waveform Tracing

Because every named signal in the design has a slot in the registry with a name, current value, and history buffer, the executor can record a complete waveform trace without any user intervention.

At the end of each `tick_clock`, after both settle phases complete, the executor iterates the registry and calls `record_current(cycle)` on every slot. Each slot appends a `(cycle, value)` entry to its history if the value differs from the most recent recorded entry (value-change compression, matching the VCD standard).

`exec.export_vcd(path)` iterates all slots and emits a valid VCD file:

```
$timescale 1ns $end
$scope module top $end
$var wire 8 ! counter_out $end
$var wire 1 " write_en $end
...
$dumpvars
...
$end
```

Each signal's history provides the full set of timestamped value changes. The VCD format is defined in IEEE 1364-2005 section 18. VCD files can be loaded by GTKWave, Surfer, and any other standard waveform viewer.

Internal module state that is explicitly registered via `exec.register_signal` also appears in the VCD. Unregistered internal state (`let mut count`) is not present — it exists only in the future's state machine struct and is never visible to the executor.

---

## 11. The Noop Waker

Rust's `Future::poll` requires a `Context` containing a `Waker`. A `Waker` is the mechanism by which an async runtime learns that a task is ready to make progress. When a future returns `Poll::Pending`, the contract is that it will call `waker.wake()` before it can make progress again.

Copper's executor does not use wakers for scheduling. It polls every task on every delta cycle unconditionally. The noop waker satisfies the `Future::poll` API contract without doing anything:

```rust
// copper-sim/src/executor.rs:13
fn noop_waker() -> Waker {
    fn clone(_: *const ()) -> RawWaker { RawWaker::new(std::ptr::null(), &VTABLE) }
    fn wake(_: *const ()) {}
    fn wake_by_ref(_: *const ()) {}
    fn drop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}
```

The `unsafe` block is required by [`RawWaker`](https://doc.rust-lang.org/std/task/struct.RawWaker.html) — the caller must guarantee that the vtable functions uphold the `Waker` safety contract. The contract for a noop waker is trivially satisfied: cloning produces another noop waker, waking does nothing, dropping does nothing.

This can be replaced with [`futures::task::noop_waker()`](https://docs.rs/futures/0.3.32/futures/task/fn.noop_waker.html) from the `futures` crate (feature `executor`), which provides the same semantics without the `unsafe` block.

The `ClockTick` future pushes the (noop) waker into `state.wakers` on every failed poll. `clk.advance()` drains and calls all stored wakers — a no-op. The waker list is bounded: it accumulates at most one entry per task per delta cycle within a settle phase, and is fully cleared on every `advance()`. There is no accumulation across clock cycles.
