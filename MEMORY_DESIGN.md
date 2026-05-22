# Memory as a First-Class Language Construct

## The Problem

`Memory<T, R, W, D>` currently works as a simulation tool — the Rust executor handles clock-edge semantics correctly and the examples pass Verilator cross-validation. But it is a library type the compiler knows nothing about. This creates two real problems:

**1. The `#[hardware]` macro is blind to memory.** It cannot enforce correct usage patterns, catch illegal accesses at compile time, or derive hardware semantics from how the user wrote the code. All of the correctness invariants — single-drive per port, read-before-write ordering for ReadFirst semantics, not writing before the clock edge — are either runtime panics or silent conventions the user must know to follow.

**2. Users must write very specific code to get correct timing.** The `sync_if_advanced` mechanism fires on the first port access after the clock advances, so call order matters in a non-obvious way. The correct pattern (emit local var in pre-tick, compute it in post-tick via `rp.read()` before `wp.write()`) is not enforced — it is a convention that leaks the simulation internals into user code.

The goal of this document is to define what Memory as a first-class construct means for the **simulation and execution model**: what the user writes, what the macro enforces, and what changes need to be made. Transpilation to Verilog is noted briefly at the end but is not the immediate focus.

---

## The Mental Model: Memory as a Submodule

A `Memory<T, DEPTH, R, W, D>` is not a local variable. It is a **hardware submodule** — a separate block of silicon wired to the parent module's logic through R read ports and W write ports.

Think of a register file in a CPU: it is not "inside" the ALU or the fetch stage. It is a dedicated block that the fetch stage reads from and the writeback stage writes to. Those two operations happen at different places in the pipeline, potentially in different clock cycles, and the register file just does its job at each clock edge.

This is the correct mental model for `Memory<>` in Copper. Declaring one instantiates a hardware block and connects its clock. Accessing it through port handles is wiring signals to its read/write buses. The `#[hardware]` macro understands this and enforces the rules that physical hardware imposes.

---

## The API

Port handles with array indexing syntax:

```rust
// Read from read port N at address addr — returns T
let val: T = mem.rp::<N>()[addr];

// Write to write port N at address addr
mem.wp::<N>()[addr] = val;
```

`rp::<N>()` returns a `ReadPort` handle. `wp::<N>()` returns a `WritePort` handle. Both support standard array index syntax via the `Index` / `IndexMut` traits. The port index N is a const generic checked at macro expansion time against the declared R and W.

**What goes away:** `.read_first()`, `.write_first()` builder calls. The read-during-write mode is no longer declared — it is inferred from where in the loop body reads and writes appear relative to `tick()`. This is described in the timing rules below.

**What stays:** `Memory::<T, DEPTH, R, W, D>::new(clk, size)`, `from_contents`, `from_fn`. The declaration syntax is unchanged.

---

## Timing Rules

Where a memory access appears in the `#[hardware]` loop body relative to `clk.tick().await` determines what hardware it maps to. The rules are explicit and enforced by the macro.

### Rule 1 — Pre-tick read: output register

A `rp::<N>()[addr]` call **before** `clk.tick().await` reads the output register — the value captured at the immediately preceding clock edge. It also sets the read address that will be captured at the next edge.

```rust
loop {
    let out = mem.rp::<0>()[addr];  // output register from previous edge
    emit!(out);
    clk.tick().await;
    // ... post-tick logic
}
```

In hardware: the output register is a real flip-flop updated on every posedge. Reading it in the pre-tick region gives you the value from the last clock edge, regardless of which phase that was. This is always well-defined after the first cycle.

### Rule 2 — Pre-tick write: compile error

A `wp::<N>()[addr] = val` call **before** `clk.tick().await` is a compile error.

```
error: memory write before clock edge has no meaning in synchronous hardware
  --> src/my_module.rs:12:5
   |
12 |     mem.wp::<0>()[addr] = val;
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^
   | note: move this write to the post-tick region
```

Writes commit at the clock edge. A write staged before the clock edge of the same `tick()` call is not representable in synchronous hardware.

### Rule 3 — Post-tick read before write: ReadFirst

A `rp::<N>()[addr]` **after** `tick()` and **before** any `wp::<M>()[...]` in the same phase reads the data as it existed before any write in this cycle commits. This is ReadFirst semantics.

```rust
loop {
    clk.tick().await;
    let out = mem.rp::<0>()[rd_addr];      // ReadFirst: sees pre-write data
    if we { mem.wp::<0>()[wr_addr] = din; } // write staged after read
    emit!(out);
}
```

In hardware: this maps to the standard block RAM pattern where `data_out <= mem[rd_addr]` is captured before `if (we) mem[wr_addr] <= din` in the `always_ff` block.

### Rule 4 — Post-tick write before read: WriteFirst

A `wp::<N>()[addr]` **after** `tick()` and **before** a `rp::<M>()[addr]` in the same phase means the read sees the newly staged value if the addresses match. This is WriteFirst semantics.

```rust
loop {
    clk.tick().await;
    if we { mem.wp::<0>()[wr_addr] = din; } // write staged first
    let out = mem.rp::<0>()[rd_addr];       // WriteFirst: sees staged write if addr matches
    emit!(out);
}
```

### Rule 5 — Double write to the same port in the same phase: compile error

One physical write port has one address bus and one data bus. Driving it twice in the same clock cycle is a hardware violation.

```
error: write port 0 of `mem` driven more than once in phase 0
  --> src/my_module.rs:15:5
   |
12 |     mem.wp::<0>()[addr_a] = val_a;  // first drive
   |     ------------------------------ driven here
15 |     mem.wp::<0>()[addr_b] = val_b;  // second drive
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
```

This replaces the current `debug_assert!` — it is a compile error, not a runtime panic.

### Rule 6 — Port index out of range: compile error

`rp::<N>()` with N ≥ R, or `wp::<N>()` with N ≥ W, is a compile error at macro expansion time. The port count is part of the type and known statically.

### Rule 7 — Mode must be consistent across phases

If multiple phases of a multi-tick loop each have both a read and a write to the same memory, they must use the same mode (both ReadFirst or both WriteFirst). Mixing is a compile error — a block RAM has one static read-during-write mode.

```
error: `mem` uses ReadFirst semantics in phase 0 but WriteFirst in phase 2
   | note: a block RAM has one read-during-write mode; keep ordering consistent
```

---

## Multi-Tick Behavior

In a multi-tick loop, each `tick()` is a separate clock edge and therefore a separate hardware phase. This has direct consequences for memory.

**Reads and writes in different phases are in different clock cycles — they cannot conflict.** The ReadFirst/WriteFirst question only arises when a read and write appear in the same phase (between the same two `tick()` calls). Cross-phase accesses are always temporally separated.

```rust
loop {
    clk.tick().await;
    let val = mem.rp::<0>()[read_addr];   // phase 0 — read only, no conflict
    clk.tick().await;
    mem.wp::<0>()[write_addr] = compute(val); // phase 1 — write only, no conflict
}
```

This is clean. Phase 0 and phase 1 are different cycles; no mode question arises.

**Port reuse across phases is legal.** Writing to `wp::<0>` in phase 0 and again in phase 1 is fine — those are different clock cycles, each with their own drive. The double-drive rule applies within a single phase only.

**Pipeline hazards are the user's responsibility.** If phase 0 reads address A and phase 1 writes address A, the phase 0 read in cycle N sees data committed before cycle N — not the value written in cycle N+1. The compiler does not inject forwarding. This is the same as real hardware.

**The output register updates on every clock edge.** In a multi-tick loop, the pre-tick read (Rule 1) returns the output register as captured at the end of the immediately preceding clock edge — the last phase of the previous iteration. The register does not hold values across multiple edges; it always reflects the most recent posedge. If a value needs to survive multiple phases, it must be stored in a local variable, which naturally becomes a register crossing the `tick()` boundary via Rust's async transform.

---

## What the Macro Does

The `#[hardware]` macro, upon seeing a `Memory<T, DEPTH, R, W, D>` binding in a hardware function, takes over all of the following:

**Detection.** It recognizes `Memory::new`, `from_contents`, and `from_fn` let-bindings as memory declarations, not generic bindings. It extracts DEPTH, R, W, and D from the type parameters.

**Access classification.** For every `rp::<N>()[...]` and `wp::<N>()[...]` call in the body, it determines:
- Which phase the access is in (which pair of `tick()` calls surrounds it)
- Whether it is pre-tick or post-tick
- The read/write ordering within each phase

**Mode inference.** From the ordering within each post-tick phase, it infers ReadFirst or WriteFirst and inserts the appropriate mode at the `Memory::new()` call site. Users never call `.read_first()` or `.write_first()`.

**Rule enforcement.** It emits compile errors for violations of Rules 2–7 above, pointing to the exact source location and explaining the hardware reason.

**Simulation correctness.** The macro routes `rp::<N>()[addr]` to the correct internal runtime method (`read_output_reg` in the pre-tick region, `read_data` in the post-tick region) based on position. The runtime does not need to reason about call ordering — the clock edge drives memory state, and the macro handles the routing.

---

## What Needs to Change

The rewrite happens in two layers: the simulation runtime first, then the macro on top of it. Getting the runtime right is the prerequisite for the macro work because the macro generates calls into the runtime.

### 1. `Clock<D>` — listener registration

`Clock<D>` needs a mechanism to notify registered memories when the clock advances. This is a small targeted change to `copper-core/src/types.rs`.

```rust
// New trait in copper-core
pub(crate) trait ClockEdgeListener: Send + Sync {
    fn on_posedge(&self);
}

// Clock gains two methods:
impl<D: ClockDomain> Clock<D> {
    pub(crate) fn register_listener(&self, listener: Weak<dyn ClockEdgeListener>);
}
```

`advance()` iterates the registered listeners and calls `on_posedge()` on each after incrementing the cycle counter. Listeners are stored as `Weak` pointers so memory instances can be dropped without leaving dangling references in the clock.

This is the only change needed to `Clock<D>`.

### 2. `Memory<>` — rewrite `memory_new.rs`

The data model is correct and stays unchanged: `data`, `staging`, `read_addr`, `output_reg`. What changes is everything that drives and accesses that state.

**Remove `sync_if_advanced` and `last_cycle`.** The lazy-commit mechanism is replaced entirely by the clock listener. `MemoryInner` no longer needs a `last_cycle` field. The posedge logic (capture output registers, commit staging, reset write flags) moves into `on_posedge()`:

```rust
impl<T: Clone, const R: usize, const W: usize, D: ClockDomain>
    ClockEdgeListener for MemoryListenerInner<T, R, W>
{
    fn on_posedge(&self) {
        // ReadFirst: capture output_reg BEFORE committing staging
        // WriteFirst: commit staging FIRST, then capture output_reg
        // Reset write_used flags
    }
}
```

`Memory::new()` creates the listener, registers it with the clock, and holds the `Arc`. When the clock advances, posedge fires automatically — no port access required to trigger it.

**Remove `write_used` tracking.** This field exists only to catch double-drive at runtime. With the macro enforcing Rule 5 at compile time, the runtime tracking is dead weight. Remove `write_used` from `MemoryInner` and remove the `debug_assert!` from `WritePort::write()`.

**Split the read API into two internal methods on `ReadPort`:**

```rust
// Pre-tick: returns output_reg[I] (captured at last posedge)
// Also sets read_addr[I] so this posedge captures the right address
pub(crate) fn read_output_reg(&self, addr: usize) -> Option<&T>

// Post-tick: returns current committed data[addr]
// WriteFirst: checks staging for same-address forwarding
// Also sets read_addr[I] for the next posedge
pub(crate) fn read_data(&self, addr: usize) -> T
```

The user-facing `Index` impl (`rp[addr]`) calls `read_data` as the default — this preserves direct unit test access without the macro. The macro transforms user-written `rp[addr]` calls in hardware functions to either `read_output_reg` or `read_data` based on the position in the body.

**`WriteMode` and `ReadMode` become `pub(crate)`.** They are still needed internally (`WriteMode` drives the forwarding path in `read_data`; `ReadMode` determines whether `on_posedge` captures output registers). The builder methods `.read_first()`, `.write_first()`, `.async_read()` become `pub(crate)` — called by macro-generated code, not by users. `.async_read()` is the one exception that remains conceptually user-facing because it declares a hardware property of the read port that cannot be inferred from position alone; it will be handled separately.

**The `output()` method on `ReadPort` is removed.** Pre-tick reads using `rp[addr]` call `read_output_reg` via the macro, which is the correct way to access the output register. A separate `.output()` method that bypasses address setup is a footgun — remove it.

### 3. The `#[hardware]` macro

This is the main work and is unblocked once the runtime changes above are in place. The macro needs to:

- Detect `Memory<>` bindings (currently treated as opaque let-bindings)
- Walk the async function body and classify every port access by phase and pre/post-tick position
- Route pre-tick `rp[addr]` → `rp.read_output_reg(addr)` and post-tick `rp[addr]` → `rp.read_data(addr)` in the emitted token stream
- Infer and insert the read-during-write mode at the `Memory::new()` call site
- Emit structured compile errors for each of the 7 rules

The macro already parses the full function body. The new work is teaching it to recognize the memory access patterns and reason about their positions.

---

## Notes on Transpilation

Making the macro memory-aware is the prerequisite for everything downstream. Once the macro understands memory declarations and accesses as structured concepts (not opaque method calls), the codegen pipeline can follow:

- **CHIR** gains `CHIRMemoryDecl` on `CHIRSeqBody` and `CHIRStmt::MemRead` / `MemWrite` variants
- **SHIR** places each memory operation into the correct timing region (pre-edge address setup, at-edge capture and commit) based on the inferred mode
- **Phase E emission** generates the structural Verilog block RAM pattern (`reg` array + `always_ff` with read/write ordered by mode + `always_comb` for address setup) that synthesis tools infer as dedicated BRAM primitives

None of this is possible while the macro treats memory as opaque library calls. Getting the macro right first unlocks the full pipeline.

---

## Implementation Timeline

The timeline is split into two phases. **Phase A (Steps 1–3)** rewrites the simulation runtime to accurately model memory hardware. **Phase B (Steps 4–9)** builds the macro on top of that runtime. The transpilation work (Step 10) is deferred until Phase B is complete.

The runtime rewrite comes first because the macro generates calls into the runtime. Building the macro on top of `sync_if_advanced` would mean compensating for an implementation artifact in user-facing error messages and code generation — the wrong foundation.

---

### Step 1 — Add clock listener registration to `Clock<D>`

**What:** Add a `register_listener` method to `Clock<D>` in `copper-core/src/types.rs`. Define a `ClockEdgeListener` trait. Update `Clock::advance()` to call `on_posedge()` on all registered listeners after incrementing the cycle counter.

```rust
pub(crate) trait ClockEdgeListener: Send + Sync {
    fn on_posedge(&self);
}

// On ClockInner — stores Weak pointers so dropped memories don't leave dangling refs
listeners: Mutex<Vec<Weak<dyn ClockEdgeListener>>>,
```

**Things to consider:**
- `Weak` pointers require upgrading before calling — dead entries (memories that have been dropped) should be pruned from the list during advance. A simple `retain` that only keeps entries that upgrade successfully keeps the list from growing unboundedly.
- The existing `advance()` call in `HardwareExecutor::tick_clock` does not need to change — it calls `clk.advance()` which now does the additional listener notification internally.
- `Clock::advance()` is currently `pub` (called by executors and unit tests). This change is backwards-compatible — existing callers see no difference, they just now also trigger memory posedges.
- Unit tests in `tests/memory_new.rs` call `clk.advance()` directly and will automatically benefit from this change — no test updates needed.

---

### Step 2 — Rewrite `memory_new.rs`

**What:** Remove `sync_if_advanced` and `last_cycle`. Implement `ClockEdgeListener` for `Memory`. Register with the clock at construction. Split the read API. Remove `write_used`. Make builder methods `pub(crate)`. Remove `output()`.

Concrete checklist:
- [ ] Remove `last_cycle` from `MemoryInner`
- [ ] Remove `write_used` from `MemoryInner`
- [ ] Remove `sync_if_advanced` method from `Memory`
- [ ] Add `on_posedge()` implementing `ClockEdgeListener` with the ReadFirst/WriteFirst logic
- [ ] Register `self` as a listener in `Memory::new()`, `from_contents()`, `from_fn()`
- [ ] Add `read_output_reg<const I>(&self, addr: usize) -> Option<&T>` to `ReadPort`
- [ ] Add `read_data<const I>(&self, addr: usize) -> T` to `ReadPort`
- [ ] Update `Index` impl on `ReadPort` to call `read_data` (default, for unit tests)
- [ ] Remove `output()` from `ReadPort`
- [ ] Remove `debug_assert!` from `WritePort::write()` and `IndexMut`
- [ ] Mark `.read_first()`, `.write_first()`, `.async_read()` as `pub(crate)`

**Things to consider:**
- The `Arc` ownership structure needs care. `Memory` holds `Arc<UnsafeCell<MemoryInner>>` and registers a `Weak` with the clock. The `Weak` must point to the same inner data that `on_posedge` mutates. A clean approach: introduce a separate `MemoryShared<T, R, W>` struct that is `Arc`-wrapped and implements `ClockEdgeListener`, and have `Memory` hold an `Arc<MemoryShared>`.
- All existing unit tests in `tests/memory_new.rs` call `clk.advance()` and use `rp.read()` or `rp[addr]`. Since `Index` still calls `read_data`, these tests continue to work. The tests for `output()` will need to be rewritten to use `read_output_reg` directly or removed if `output()` is gone.
- `from_contents` and `from_fn` preload data directly into `data` without staging — this logic is unchanged.
- `WriteFirst` forwarding in `read_data` must still check the staging slot for a matching address, exactly as `sync_if_advanced` did. This is a software simulation of same-cycle write-then-read visibility — keep it.

---

### Step 3 — Update and revalidate tests

**What:** Update `tests/memory_new.rs` to match the new API. Run the full test suite. Run `sync_ram` and `reg_file` examples to confirm simulation output is unchanged.

**Things to consider:**
- Tests that used `rp.output()` need to be updated to call `rp.read_output_reg(addr)` and advance the clock first (since the output register is now set by posedge, which fires on `advance()`).
- Tests that called `.write_first()` or `.read_first()` directly need to use `pub(crate)` access or be moved to an internal test module.
- The Verilator cross-validation in `sync_ram` and `reg_file` is the correctness oracle — if those still produce matching output, the semantic behavior of the runtime is preserved.
- This step is complete when `cargo test --all` passes and both Verilator examples pass.

---

### Step 4 — Define the macro-side memory representation

**What:** Define Rust types inside `copper-macros` that represent a parsed memory binding and its accesses. These are the macro's internal IR — not exposed to users, not part of CHIR/SHIR yet.

```rust
struct MemoryBinding {
    name: String,       // "mem"
    depth: usize,
    num_read_ports: usize,   // R
    num_write_ports: usize,  // W
    span: Span,
}

struct MemAccess {
    mem_name: String,
    port: usize,
    kind: AccessKind,   // Read | Write
    phase: usize,       // which tick()-segment (0 = pre-first-tick, 1 = after tick 0, ...)
    pre_tick: bool,     // true if before the phase's own tick()
    span: Span,
}
```

**Things to consider:**
- How to represent accesses that are inside `if`/`match` blocks — the phase and pre/post-tick position is still the same (control flow doesn't change the clock boundary), but the condition must be tracked for the write-enable path later (used by transpilation Step 10)
- Multiple distinct memories in the same function need to be tracked separately — use a `HashMap<String, MemoryBinding>`

---

### Step 5 — Teach the macro to detect `Memory<>` declarations

**What:** Walk the function body looking for `let <name> = Memory::<T, DEPTH, R, W, D>::new(...)` patterns. Extract DEPTH, R, W, D from the turbofish generics. Record the binding.

**Things to consider:**
- The user may omit the turbofish if the type is inferred from context (e.g., the type annotation is on the `let` binding). The macro needs to handle both `let mem: Memory<u8, 4, 2, 1, D> = Memory::new(clk, 4)` and `let mem = Memory::<u8, 4, 2, 1, D>::new(clk, 4)`.
- `from_contents` and `from_fn` must also be recognized — same binding detection, different constructor.
- A `Memory` with `W = 0` is a ROM — `wp` accesses on it should be caught at Step 5 (Rule 6 extension: write port on ROM). Record the W=0 case explicitly.
- If the macro cannot determine R or W statically (e.g., they are const generic parameters of the enclosing function), emit an error asking for explicit values.

---

### Step 6 — Build the phase/position classifier

**What:** Walk the function body and divide it into labeled segments separated by `clk.tick().await` expressions. Assign each statement a `(phase, pre_tick)` pair. Phase 0 is before the first `tick()`, phase 1 is after the first `tick()` and before the second, etc. Pre-tick is `true` for phase 0 (everything before the first tick in the loop is pre-tick for that phase).

**Things to consider:**
- The `#[hardware]` function body is a `loop` with `tick().await` inside. Walking it means descending into the loop body and tracking tick boundaries.
- `tick().await` is a specific pattern: a method call `tick()` on a clock variable, wrapped in an `Await` expression. The macro must reliably identify this vs. other `.await` expressions.
- Control flow (`if`, `match`) does not create new phases — all branches inherit the same phase. However, a `tick()` inside an `if` branch is currently not supported (and should produce an error — conditional clock edges are not valid synchronous hardware).
- For multi-tick loops, the last phase wraps back to phase 0 on the next iteration. The classifier should number phases starting at 1 for post-tick regions (phase 1 = after tick 0, phase 2 = after tick 1, etc.) and use phase 0 for the pre-loop region.

---

### Step 7 — Detect and classify memory accesses

**What:** Walk the function body and find every `mem.rp::<N>()[addr]` and `mem.wp::<N>()[addr] = val` expression. For each, record the memory name, port index, kind (read/write), and the `(phase, pre_tick)` from Step 3.

The pattern to match is a method call chain:
```
Index(MethodCall(expr, "rp" | "wp", [const N], []), addr_expr)
```
The inner `expr` must resolve to a known memory binding name.

**Things to consider:**
- The address expression `addr_expr` can be arbitrarily complex. The macro does not need to evaluate it — it just needs to record it for later code generation.
- `rp` vs `wp` must be recognized by method name, and the const generic port index N extracted from the angle bracket generics on the method call.
- Accesses nested inside `if`/`match` blocks must still be found — the walker must recurse into branches. The phase/pre_tick tag comes from the surrounding block, not the branch itself.
- If the memory name cannot be resolved (user wrote `some_fn().rp::<0>()[addr]` where the receiver is not a known binding), skip it or emit a warning — this is not a macro-managed memory.

---

### Step 8 — Implement the compile-time rules

**What:** With the classified accesses from Step 4, enforce all 7 timing rules as `syn::Error` compile errors.

In order of implementation priority:

1. **Rule 6 — Port index out of range.** Simplest: compare N against the recorded R or W. Emit error at the access span.
2. **Rule 2 — Pre-tick write.** Any write with `pre_tick == true` is an error.
3. **Rule 5 — Double-write in same phase.** Group writes by `(mem_name, port, phase)`. If any group has more than one entry, emit an error pointing to both spans.
4. **Rule 7 — Inconsistent mode across phases.** For each phase that has both reads and writes, determine the ordering (read-before-write → ReadFirst, write-before-read → WriteFirst). If two phases disagree, emit an error.
5. **Rule 2 extension — Write on ROM (W=0).** Any `wp` access on a memory with `num_write_ports == 0` is an error.

**Things to consider:**
- Multiple errors should be emittable in one pass — collect all errors rather than bailing on the first one. Users should see all violations in a single compile cycle.
- The "ordering" within a phase is source order (AST traversal order). The macro uses this as the authoritative ordering for ReadFirst/WriteFirst inference.
- A phase with only reads or only writes has no mode question — skip it for Rule 7.
- A phase with reads and writes inside separate `if` branches (e.g., write in the `then`, read in the `else`) is ambiguous. For now: emit an error saying the ordering is not statically determinable and asking the user to restructure. This can be relaxed later.

---

### Step 9 — Mode inference and code generation

**What:** For each memory declaration, determine the inferred mode from the classified accesses and transform the emitted token stream in two ways:

1. Insert `.read_first()` or `.write_first()` on the `Memory::new()` constructor call site so the runtime is configured with the correct mode.
2. Transform each `rp::<N>()[addr]` expression to `rp::<N>().read_output_reg(addr)` (pre-tick) or `rp::<N>().read_data(addr)` (post-tick) so the runtime method that fires is correct for the timing region.

If only reads or only writes appear across the whole function (no mode question), default to ReadFirst.

**Things to consider:**
- The macro transforms the input token stream into an output token stream. Inserting `.read_first()` means finding the `Memory::new(...)` expression in the token stream and wrapping it: `Memory::new(...).read_first()`. This is a local transformation at the constructor call site.
- If both ReadFirst and WriteFirst appear across different phases (Rule 7 fired), no code is emitted — the error path took over.
- `async_read` mode: `.async_read()` remains a user-facing builder call. The macro cannot infer combinational vs. registered read mode from position alone — it is a static property of the port's hardware type, not a timing choice. This is the one builder method that stays public.
- After this step, `.read_first()` and `.write_first()` are internal — generated by the macro, not written by users. Remove them from public API docs.

---

### Step 10 — Update the `Memory<>` public API

**What:** Remove `.read_first()` and `.write_first()` from the public API (mark `#[doc(hidden)]` or make them `pub(crate)` — they are still called by the macro-emitted code, just not by users directly). Update all existing examples and tests to not call them.

**Things to consider:**
- `sync_ram.rs` and `reg_file.rs` do not call `.read_first()` or `.write_first()` directly — they rely on the default. After the macro exists, it will insert these calls automatically. No changes to those files.
- `tests/memory_new.rs` has explicit `.write_first()` tests that call the builder directly. These should be moved to an internal `#[cfg(test)]` module inside `memory_new.rs` using `pub(crate)` access rather than external tests calling a public method.
- The consuming `WritePort::write(self)` migration (making single-drive a type-level guarantee) is **deferred**. It requires removing `IndexMut` from `WritePort`, which is a larger ergonomics tradeoff. The macro's Rule 5 compile error provides the same safety guarantee in the meantime.

---

### Step 11 — End-to-end simulation tests

**What:** Write new tests in `tests/` that specifically exercise the macro's enforcement:

- A test that the correct mode is inferred for ReadFirst usage
- A test that the correct mode is inferred for WriteFirst usage
- Compile-fail tests (using `trybuild` or `compile_fail`) for each of Rules 2, 5, 6, 7
- A test that multi-tick memory usage (different phases, no conflict) compiles and produces correct simulation output

**Things to consider:**
- `trybuild` is the standard crate for testing that specific inputs produce specific compiler errors. It checks both that the compile fails and that the error message matches. This is the right tool for Rules 2, 5, 6, 7.
- The existing Verilator cross-validation examples (`sync_ram`, `reg_file`) serve as the integration test for simulation correctness — they should continue to pass without any changes to the example files themselves.

---

### Step 12 — Validate existing examples are unchanged

**What:** Confirm that `sync_ram.rs`, `reg_file.rs`, and `ram_rom.rs` continue to compile and produce correct output without any changes to those files. The macro's new behavior should be additive — existing valid code stays valid.

**Things to consider:**
- The macro currently passes through all code unchanged (it is a marker). Adding detection and enforcement may accidentally reject valid patterns. The tests in Step 8 exist to catch this, but a manual check of every existing memory-using example is warranted.
- `ram_rom.rs` uses `Memory` in a different pattern. Verify it is correctly classified by the new macro.

---

### Step 13 — Transpilation (deferred, unlocked by Step 9)

Once the macro understands memory as a structured concept, the codegen pipeline can be extended. This is a separate body of work, not part of the simulation milestone:

- **CHIR**: add `CHIRMemoryDecl` to `CHIRSeqBody`; add `CHIRStmt::MemRead` and `MemWrite` variants; update Phase B lowering to recognize the macro-annotated patterns
- **SHIR**: add `SHIRMemoryDecl`; place read/write ops into correct timing regions based on inferred mode; add `mem_writes` to `SHIRPhase`
- **Phase E**: emit structural Verilog block RAM pattern that synthesis tools recognize as BRAM primitives
- **Validation**: run `sync_ram` and `reg_file` through the full pipeline; verify generated Verilog passes the same Verilator stimulus tables as the hand-written reference Verilog
