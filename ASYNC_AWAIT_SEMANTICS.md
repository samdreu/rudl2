# Async/Await and Emission Semantics in Copper

This document explains how Copper models cycle-accurate hardware behavior using Rust's async/await system, with precise coverage of `clk.tick().await` and `emit!`.

## Core Mental Model

Copper maps Rust async concepts directly onto hardware concepts:

| Rust concept | Hardware analog |
|---|---|
| `async fn` | A hardware module (sequential process) |
| `loop { ... .await }` | Always block / infinite process loop |
| `.await` point | Clock edge boundary |
| Variable living across `.await` | Register (state held between edges) |
| Variable not crossing `.await` | Wire / combinational signal |
| `emit!(value)` | Drive output port |

The executor runs all module futures in lockstep — every module sees the same cycle counter and advances together.

---

## Clock and Clock Domain

### `ClockDomain` trait

```rust
pub trait ClockDomain: 'static {}
```

`ClockDomain` is a marker trait implemented on user-defined zero-sized types:

```rust
struct MainClk;
impl ClockDomain for MainClk {}
```

It is a phantom type used at compile time to associate a `Clock<Domain>` with the correct domain. A `Clock<MainClk>` cannot be used where a `Clock<PeriphClk>` is expected. This prevents cross-domain clock mistakes at the type level, with zero runtime cost.

### `Clock<Domain>` internals

```rust
pub struct Clock<Domain: ClockDomain> {
    state: Arc<Mutex<ClockState>>,
    _domain: PhantomData<Domain>,
}

struct ClockState {
    cycle: u64,
    wakers: Vec<Waker>,
}
```

`Clock` wraps an `Arc<Mutex<ClockState>>` so that all clones share the same underlying cycle counter and waker list. Cloning a `Clock` is how you pass it into spawned module futures — all clones point to the same clock state and will observe the same `advance()` call.

### `Clock::advance()`

```rust
pub fn advance(&mut self) {
    let mut state = self.state.lock().unwrap();
    state.cycle += 1;
    let wakers = std::mem::take(&mut state.wakers);
    drop(state);
    for w in wakers {
        w.wake();
    }
}
```

`advance()` increments the cycle counter and wakes every task that registered a `Waker` while waiting for the next tick. The lock is released before waking to avoid deadlocks. Only the executor calls `advance()` — module futures never advance the clock themselves.

---

## `clk.tick().await` — Precise Semantics

### What `tick()` returns

```rust
pub fn tick(&self) -> ClockTick<Domain> {
    let target = self.cycle().wrapping_add(1);
    ClockTick {
        state: Arc::clone(&self.state),
        target_cycle: target,
        _domain: PhantomData,
    }
}
```

`tick()` captures the target cycle (`current + 1`) at call time and returns a `ClockTick` future. Calling `tick()` does nothing on its own — the future must be `.await`ed to have any effect.

### How `ClockTick` polls

```rust
impl<Domain: ClockDomain> Future for ClockTick<Domain> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut state = self.state.lock().unwrap();
        if state.cycle >= self.target_cycle {
            Poll::Ready(())
        } else {
            state.wakers.push(cx.waker().clone());
            Poll::Pending
        }
    }
}
```

On each poll:

- If the clock has already reached the target cycle → return `Ready(())` immediately.
- Otherwise → register the current task's `Waker` and return `Pending`.

When `clk.advance()` fires later, it drains the waker list and calls `wake()` on each one, which marks the task as ready to be polled again in the next executor pass.

### What `clk.tick().await` means at the call site

```rust
clk.tick().await;
```

1. `tick()` records `target = current_cycle + 1`.
2. The future is polled. Since the cycle has not advanced yet, it registers a waker and suspends (returns `Pending`).
3. The executor calls `clk.advance()`, incrementing the cycle and waking the task.
4. On the next executor poll pass the task resumes at the line after `.await`.

The module is effectively paused for exactly one clock edge.

---

## Executor Phases — `tick_clock`

```rust
pub fn tick_clock<Domain: ClockDomain>(&mut self, clk: &mut Clock<Domain>) {
    self.poll_tasks();   // pre-edge settle
    clk.advance();       // clock edge: cycle += 1, wake tick waiters
    self.poll_tasks();   // post-edge settle
    self.cycle += 1;
}
```

Each call to `tick_clock` performs three steps:

### Step 1 — Pre-edge settle (`poll_tasks`)

All tasks are polled once. Tasks that have not hit a `tick().await` will run until they either:
- Reach a `tick().await` and suspend, or
- Complete (unusual for infinite hardware loops).

This phase lets combinational logic compute and emit current-cycle outputs before the edge.

### Step 2 — Clock edge (`clk.advance()`)

`clk.advance()` increments the clock's cycle counter and wakes all tasks that were waiting. These tasks are now schedulable but are not polled yet.

### Step 3 — Post-edge settle (`poll_tasks`)

All tasks are polled again. Tasks that were just woken by the clock edge resume execution after their `tick().await` and run until their next suspension point. This is where registered/sequential logic updates happen.

### Timeline visualization

```
tick_clock call N:
  ┌─ pre-edge poll ─────────────────────────────────────┐
  │  tasks run until tick().await                       │
  │  emit!() calls here → visible in pre-edge          │
  └─────────────────────────────────────────────────────┘
  ┌─ clk.advance() ─────────────────────────────────────┐
  │  cycle counter increments                           │
  │  wakers fired → tasks become schedulable           │
  └─────────────────────────────────────────────────────┘
  ┌─ post-edge poll ────────────────────────────────────┐
  │  woken tasks resume after tick().await              │
  │  emit!() calls here → visible in post-edge         │
  └─────────────────────────────────────────────────────┘
```

---

## `emit!(value)` — Precise Semantics

### The emit target system

Each spawned function-typed task has an associated output signal, stored as `Option<Arc<dyn Any + Send + Sync>>`:

```rust
struct TaskEntry {
    future: Pin<Box<dyn Future<Output = ()>>>,
    emit_target: Option<Arc<dyn Any + Send + Sync>>,
}
```

The concrete type behind the `Arc<dyn Any>` is `Arc<Mutex<T>>` where `T` is the module's output type.

### How the target is bound during polling

```rust
pub fn poll_tasks(&mut self) {
    for task in &mut self.tasks {
        let _emit_guard = crate::push_emit_target(task.emit_target.clone());
        let _ = task.future.as_mut().poll(&mut context);
    }
}
```

Before polling each task, `push_emit_target` installs the task's emit target into a thread-local slot (`CURRENT_EMIT_TARGET`) and returns an RAII guard. When the guard drops (after the poll call returns), the previous target is restored. This means `emit!` always routes to the correct output for whichever task is currently being polled.

### What `emit!(value)` does

```rust
macro_rules! emit {
    ($value:expr) => {{
        $crate::emit_to_current($value);
    }};
}

pub fn emit_to_current<T: Send + 'static>(value: T) {
    CURRENT_EMIT_TARGET.with(|cell| {
        let target = cell.borrow().as_ref().cloned()
            .expect("emit!(value) called without a bound function-typed output");
        let typed = Arc::downcast::<Mutex<T>>(target)
            .expect("emit!(value) type mismatch for currently bound function-typed output");
        *typed.lock().unwrap() = value;
    });
}
```

`emit!(value)`:
1. Reads the current thread-local emit target.
2. Panics if no target is bound (called outside a `spawn_function_typed` task).
3. Downcasts the `Arc<dyn Any>` to `Arc<Mutex<T>>`. Panics on type mismatch.
4. Locks and overwrites the shared output value.

The caller (simulation harness) reads this `Arc<Mutex<T>>` after `tick_clock` returns to observe the emitted value.

### Spawning a function-typed module

```rust
let output: Arc<Mutex<u8>> = exec.spawn_function_typed(0u8, my_module(clk.clone()));
```

`spawn_function_typed` allocates the `Arc<Mutex<T>>`, stores the inner `Arc` as the task's emit target, and returns the outer `Arc<Mutex<T>>` handle to the caller. Reading `*output.lock().unwrap()` after a `tick_clock` gives the most recently emitted value.

---

## Ordering: `emit!` vs `tick().await`

The position of `emit!` relative to `tick().await` inside the loop determines the observable timing.

### Pattern A — Emit then tick (pre-edge / same-cycle output)

```rust
loop {
    emit!(q);           // drive output with current state
    clk.tick().await;   // wait for next edge
    q = next_q;         // update state after edge
}
```

Execution timeline:

```
Cycle N pre-edge:   emit!(q_N) → output reads q_N
                    tick().await → suspend
Cycle N clock edge: advance
Cycle N post-edge:  resume, q = next_q (ready for cycle N+1)
```

The emitted value reflects state computed before the edge. This models **combinationally-driven or immediately-registered outputs** where the current state is visible in the same cycle it was computed.

### Pattern B — Tick then emit (post-edge / registered output)

```rust
loop {
    clk.tick().await;   // wait for edge first
    q = next_q;         // update state
    emit!(q);           // drive output with new state
}
```

Execution timeline:

```
Cycle N pre-edge:   tick().await → suspend (no emit)
Cycle N clock edge: advance
Cycle N post-edge:  resume, q = next_q, emit!(q_new)
```

The emitted value only appears after the clock edge. This models **strictly edge-triggered (registered) outputs** — the output is invisible until the edge fires.

### Summary table

| Pattern | When `emit!` runs | Output visible | Models |
|---|---|---|---|
| `emit!` → `tick().await` | Pre-edge poll | Same cycle (pre-edge) | Combinational / same-cycle registered |
| `tick().await` → `emit!` | Post-edge poll | After edge (post-edge) | Strictly edge-triggered register |

---

## Register Inference

Variables that live across a `.await` boundary are captured in the async state machine's internal struct (Rust compiler generated). This corresponds directly to **hardware registers** — state that must be held between clock edges.

Variables that are computed and used entirely within a single "segment" between two `.await` points are not persisted and correspond to **wires or combinational values**.

Example:

```rust
async fn example(clk: Clock<MainClk>) -> u8 {
    let mut reg: u8 = 0;  // register: lives across await
    loop {
        let wire = reg.wrapping_add(1);  // wire: computed each cycle, not across await
        emit!(wire);
        clk.tick().await;
        reg = wire;  // update register: assignment before next await
    }
}
```

Here `reg` is a register (captured in state machine), `wire` is a combinational value (recomputed every cycle and not in state).

---

## Module Hierarchy

Modules can be composed hierarchically using `spawn_child` or `spawn_child_function_typed`. These variants record the parent-child relationship in `HardwareExecutor::modules` for inspection and tooling, while otherwise behaving identically to the non-child variants.

```rust
let child_out = exec.spawn_child_function_typed(
    "child_name",
    "parent_name",
    0u8,
    child_module(clk.clone()),
);
```

Hierarchy metadata is accessible via `exec.module_info("name")` and `exec.module_infos()`.

---

## Common Pitfalls

### Calling `emit!` outside a function-typed task

```rust
// WRONG — will panic at runtime
exec.spawn(async { emit!(42u8); });

// CORRECT — use spawn_function_typed
let out = exec.spawn_function_typed(0u8, async { emit!(42u8); 42u8 });
```

### Calling `tick()` without `.await`

```rust
clk.tick();  // does nothing — the future is created and immediately dropped
```

Always use `.await`:

```rust
clk.tick().await;
```

### Missing the emit in a cycle

If a module does not call `emit!` during a given cycle's poll pass, the output retains its last written value (the `Arc<Mutex<T>>` is not cleared). This may or may not be the intended behavior depending on what you are modeling.

---

## Complete Examples

### Counter (Pattern A — pre-edge emit)

```rust
#[hardware(function_typed)]
async fn counter(clk: Clock<MainClk>) -> u8 {
    let mut x = 0u8;
    loop {
        emit!(x);                    // emit current value pre-edge
        clk.tick().await;            // wait for clock edge
        x = x.wrapping_add(1);       // update state post-edge
    }
}
```

After the first `tick_clock`:
- Pre-edge: emits `0`
- Post-edge: increments to `1`
- Reading output after `tick_clock` returns `0`

After the second `tick_clock`:
- Pre-edge: emits `1`
- Post-edge: increments to `2`
- Reading output returns `1`

### 2-Stage Pipeline (Pattern A)

```rust
#[hardware(function_typed)]
async fn registered_pipeline(clk: Clock<MainClk>, in_data: Arc<Mutex<u8>>) -> u8 {
    let mut stage1_r: u8 = 0;
    let mut stage2_r: u8 = 0;
    loop {
        emit!(stage2_r);                                  // output registered stage 2
        clk.tick().await;
        let input = *in_data.lock().unwrap();
        stage1_r = input.wrapping_add(1);                 // stage 1: increment
        stage2_r = stage1_r.wrapping_add(stage1_r);       // stage 2: double
    }
}
```

`stage1_r` and `stage2_r` cross the `.await` boundary → they are registers. The pipeline has a 2-cycle latency: input appears at output 2 cycles later.

### Mealy FSM (Pattern A — input-dependent combinational output)

```rust
#[hardware(function_typed)]
async fn mealy_101(clk: Clock<MainClk>, in_bit: Arc<Mutex<Bit>>) -> Bit {
    let mut state = State::S0;
    loop {
        let input = *in_bit.lock().unwrap();
        let output = match (state, input.0) {
            (State::S2, Logic::One) => Bit::ONE,
            _ => Bit::ZERO,
        };
        emit!(output);               // output depends on current state + input
        clk.tick().await;
        state = next_state(state, input);  // state updates on edge
    }
}
```

Output is combinationally derived from current state and current input, emitted pre-edge. State updates post-edge.
