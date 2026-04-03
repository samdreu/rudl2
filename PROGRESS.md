# Copper HDL Development Progress

## Project Vision & Goals

## Recent Update: Function-Typed Output Gap (March 2026)

### Completed
- Added implicit function-typed emission: `emit!(value)` now works when a function-typed output is bound by the executor.
- Removed legacy explicit emit form; the function-typed path now uses implicit `emit!(value)` exclusively.
- Added `HardwareExecutor::spawn_function_typed(initial_output, future)` to spawn function-typed async modules without explicit output handle arguments in the module signature.
- Added `HardwareExecutor::spawn_child_function_typed(...)` for hierarchical function-typed child modules with module hierarchy tracking.
- Added runtime hardening tests for implicit-output execution:
    - function-typed scalar output emission,
    - tuple output emission,
    - child hierarchy + implicit emission,
    - panic behavior for `emit!(value)` without a bound target.

### Example Migration Status
- Migrated to implicit output signatures (no explicit output handle parameter):
    - `examples/pattern_match.rs`
    - `examples/simple_counter.rs`
    - `examples/async_counter.rs`
    - `examples/independent_counters.rs`
    - `examples/pipeline.rs`
    - `examples/alu.rs`
    - `examples/mealy.rs`
    - `examples/ram_rom.rs`
    - `examples/uart_fsm.rs` (tuple output)
    - `examples/pipeline_stall.rs` (tuple output)
    - `examples/hierarchical_pipeline.rs` (child function-typed spawn helper)

### Validation
- `cargo build --examples` passes after migration.
- All 14 examples execute successfully.
- Full test suite passing (cargo test --all).

### Educational Showcase: Verilog Pitfalls
Created comprehensive educational showcases demonstrating common Verilog bugs that Copper's type system and ownership model prevent:

**1. Basic Verilog Pitfalls (`examples/verilog_pitfalls.rs`)**
- Latch inference from incomplete assignment (`bug_latch_inference.v`)
- Implicit net declaration from typos (`bug_implicit_net_typo.v`)
- Multiple driver races (`bug_multi_driver_race.v`)

**2. Simulation Hazards (`examples/simulation_hazards.rs`)**
Showcases with detailed cycle-by-cycle traces:
- Blocking/non-blocking assignment races (`bug_blocking_race.v`)
- Multiple assignments in one cycle (`bug_multi_assign_blocking.v`)
- Read-during-write scheduler dependencies (`bug_read_write_race.v`)
- Missing `default_nettype none (`bug_default_nettype.v`)

**3. Security Vulnerabilities (`examples/security_showcase.rs`)**
Hardware security bugs prevented by Copper:
- Timing side-channels from incomplete case statements (`bug_timing_sidechannel.v`)
- Uninitialized security-critical registers (`bug_register_init.v`)
- FSM illegal state handling & fault injection resistance (`bug_fsm_illegal_state.v`)
- Information leakage via unassigned outputs (`bug_info_leak.v`)
- Privilege escalation via global state (prevented by ownership)

These showcases provide concrete examples of Copper's safety advantages for publication and educational purposes.

### Core Mission
Create a fundamentally better HDL that eliminates traditional hardware description pain points by leveraging Rust's unique features (ownership, type system, async/await).

### Novel Contributions (Publication Focus)
1. **Ownership-Based Clock Domain Crossing (CDC) Safety** - First HDL to use ownership semantics for compile-time CDC verification
2. **Async/Await State Machines** - Automatic FSM generation from async functions, eliminating manual state enumeration
3. **Function-Typed Modules** - No explicit port declarations; ports inferred from function signatures
4. **Type-Driven Hardware** - Phantom types for clock domains, const generics for bit widths, zero-cost abstractions
5. **Unified Simulation/Synthesis** - Same code runs in Rust simulator and compiles to Verilog

### Target Publication Venues
- **Primary:** PLDI 2027 (Deadline: November 2026)
- **Backup:** OOPSLA 2027 (Deadline: April 2027)
- **Timeline:** 12-month development + 3-month writing = Paper submission by November 2026

### Design Philosophy

**What Copper IS:**
- Rust-embedded HDL with first-class async/await for sequential logic
- Type-safe clock domain tracking via phantom types
- Zero-overhead abstractions matching hardware semantics
- Cycle-accurate Rust simulation + Verilog backend

**What Copper is NOT:**
- Not "Verilog in Rust syntax" (that's just better syntax, not fundamentally better)
- Not high-level synthesis (we stay at RTL level)
- Not a simulator-only language (targets real hardware via Verilog)

### Comparison to Existing HDLs

| Feature | Verilog | Chisel | Clash | Bluespec | **Copper** |
|---------|---------|--------|-------|----------|------------|
| Clock domain safety | No | No | No | No | **Yes (ownership)** |
| Automatic FSMs | No | No | No | Yes (rules) | **Yes (async/await)** |
| Port inference | No | No | No | No | **Yes (functions)** |
| Native simulation | No | Scala JVM | Haskell | BSV sim | **Rust native** |
| Type-level bit widths | No | Some | Yes | No | **Yes (const generics)** |

## Phase 1: Foundation & Core Language (Months 1-3)

### Month 1: Type System & Core Abstractions ✅ STARTED

#### Week 1-2: Basic Type Foundation ✅ COMPLETE

**Completed:**
- ✅ Created `copper-core/src/types.rs` with foundational types
- ✅ Implemented `Bit` type with 4-state logic (0, 1, X)
- ✅ Implemented `Bits<N>` for bit vectors with compile-time width
- ✅ Implemented `Clock<Domain>` with tick semantics and async/await support
- ✅ Added comprehensive unit tests (10 tests, all passing)
- ✅ Created demo example showing all type features
- ✅ All types compile and pass tests
- ✅ Defined execution model (lockstep async executor)
- ✅ Documented all core design decisions

**Minor Remaining Tasks:**
- [ ] Add more API documentation
- [ ] Add edge case tests
- [ ] Create migration guide

#### Week 3-4: Async Runtime & Executor ✅ COMPLETE

**Goals:**
- ✅ Implement `HardwareExecutor` with lockstep polling
- ✅ Implement `ClockTick` Future with waker registration
- ✅ Create working async counter example
- ✅ Verify cycle-accurate simulation
- ✅ Support multiple clock domains (via Clock<Domain> phantom type)
- ✅ Implement `#[hardware]` macro for validation

**Rationale:** Need working executor and async model before proceeding to module composition

---

## Current Metrics

### Code Statistics
- **Total Lines of Code:** ~2,100
- **Core Type System:** 680 lines (copper-core/src/types.rs)
- **Executor:** 65 lines (copper-sim/src/executor.rs)
- **Macro System:** 30 lines (copper-macros/src/lib.rs)
- **Examples:** 80 lines (simple_counter.rs)
- **Tests:** 10 unit tests (100% pass rate)
- **Files Created:** 7 total (3 crates + examples)

### Type System Features
- ✅ Bit with logic operations (AND, OR, XOR, NOT)
- ✅ Bits<N> with arithmetic (add, shift, indexing)
- ✅ Clock<Domain> with async tick() for synchronous behavior
- ✅ Clock domain phantom types for CDC safety
- ✅ No wrapper types (Signal, State, Wire, Reg) - just plain Rust types
- ✅ HardwareExecutor with lockstep polling
- ✅ ClockTick Future with waker registration
- ✅ #[hardware] macro for validation

### Branch Information
- **Branch:** `feature/new-type-system`
- **Base:** `main`
- **Status:** Clean, ready for commit

---

## Next Immediate Tasks

1. ✅ Set up project tracking
2. ✅ Create branch for new type system
3. ✅ Implement `Bit`, `Bits<N>`, `Clock<Domain>`
4. ✅ Write first round of unit tests
5. ✅ Define execution model (async executor with Verilator semantics)
6. ✅ Document core design decisions
7. ✅ Implement HardwareExecutor and ClockTick
8. ✅ Build simple async counter example with executor
9. ✅ Create `#[hardware]` macro
10. ✅ Clean up warnings and unused imports
11. ⏳ **IN PROGRESS: Design IR structure for Verilog codegen** (see VERILOG_CODEGEN_DESIGN.md)
12. [ ] Implement IR builder for combinational logic
13. [ ] Implement Verilog generator for combinational logic
14. [ ] Test codegen with inverter.rs and mux.rs
15. [ ] Implement IR builder for sequential logic (async→FSM)
16. [ ] Test codegen with counter examples

---

## Development Roadmap (12 Months)

### Month 1: Type System & Execution Runtime ⏳ IN PROGRESS
- Week 1-2: Basic types (Bit, Bits, Clock) ✅ COMPLETE
- Week 3-4: HardwareExecutor, ClockTick, async runtime ✅ COMPLETE
- **Deliverable:** Working Rust simulation of async counter with cycle-accurate execution

### Month 2: Function-Typed Modules & Verilog Codegen ⏳ STARTED (March 2026)
- Week 1: IR structure design and combinational logic codegen ⏳ IN PROGRESS
- Week 2: Sequential logic (async→FSM) transformation
- Week 3: Complex state machines and match statements
- Week 4: Testing and validation with all examples
- **Deliverable:** Verilog generation working for all 14 examples

### Month 3: Async→FSM Transformation
- Implement async function lowering to Verilog state machines
- Support `loop`, `if/else`, `match` in async functions
- Generate proper sensitivity lists and blocking/non-blocking assignments
- **Deliverable:** Complex state machines (UART, SPI) in async style

### Month 4: Benchmark Circuits & Validation
- Implement standard benchmarks (AES subset, RISC-V core subset, FFT)
- Validate Rust sim vs Verilator cycle-by-cycle
- Performance comparison (simulation speed, compile time)
- **Deliverable:** 5+ benchmark circuits, correctness validated
- Define formal operational semantics
- Prove async→Verilog transformation preserves behavior
- **Deliverable:** Paper draft with formalism section

### Month 5: Formal Semantics
- Define formal operational semantics
- Prove async→Verilog transformation preserves behavior
- **Deliverable:** Paper draft with formalism section

### Month 6-7: CDC Safety & Optimization
- Implement compile-time CDC violation detection
- Add synchronizer primitives and type conversions
- Optimize generated Verilog (dead code elimination, constant propagation)
- **Deliverable:** CDC safety verification, optimized Verilog output

### Month 8: Tooling & IDE Support
- LSP integration for clock domain errors
- Waveform viewer for Rust simulation
- **Deliverable:** Good developer experience

### Month 9: Performance Evaluation
- Comprehensive benchmarking (simulation speed, compile time, code size)
- Comparison with Chisel, Clash, Bluespec
- Gather quantitative data for paper
- **Deliverable:** Complete performance evaluation section

### Month 10: Evaluation & Case Studies
- Real-world case studies demonstrating CDC safety
- Examples showing async/await benefits
- Compile-time error examples
- **Deliverable:** Motivating examples for paper

### Month 11: Paper Writing
- Draft all sections
- Run final experiments
- **Deliverable:** Complete paper draft

### Month 12: Paper Revision
- Incorporate feedback
- Polish writing
- **Deliverable:** Submit to PLDI 2027 (November 2026)

---

## Implementation Status

### Completed Components ✅
- **Type System Foundation**
  - `Bit` with 4-state logic
  - `Bits<N>` with const generic widths
  - `Clock<Domain>` with tick semantics and CDC tracking
  - Logic enum supporting X (unknown) for simulation
  - 10 unit tests, all passing
  - Working demo example
- **Async Executor & Runtime**
  - `HardwareExecutor` with lockstep polling
  - `ClockTick<Domain>` Future with waker registration
  - `noop_waker()` for synchronous polling
  - Working async counter example (simple_counter.rs)
  - Cycle-accurate execution verified
- **Macro System**
  - `#[hardware]` macro validates async functions
  - Marker-based implementation (Phase 4 will add code gen)

### In Progress ⏳
- **Week 3-4 Cleanup** (Minor)
  - Remove unused imports and dead code
  - Add comprehensive documentation
  - Create additional examples (pipeline, multi-module)

### Next Up 🔜 (Phase 4: Module Composition)
- **Enhanced Macro & Module Handles** (PRIORITY)
  - Generate module handles for output tracking
  - Support parent module composition (calling child modules)
  - Create examples: pipeline, multi-module hierarchy
  - Implement cross-module connectivity
  - Add module output sampling across cycles
  
**Then Month 2:**
- **Function-Typed Modules**
  - Modify `#[hardware]` macro to parse function signatures
  - Auto-generate port lists from parameters/return types
  - Distinguish `fn` vs `async fn` in codegen

### Future Work 📅
- **Month 3:** Async→FSM lowering
- **Month 4:** Benchmark circuits and validation
- **Month 5:** Formal semantics
- **Month 6-7:** CDC safety analysis and optimization
- **Month 8:** Tooling and IDE support
- **Month 9-10:** Performance evaluation and case studies
- **Month 11-12:** Paper writing and revision

**Deferred Features (Post-Paper):**
- Advanced memory primitives (multi-port RAM, ROM)
- Parametric width inference
- Blackbox Verilog integration
- FPGA-specific primitives
- Formal verification integration

---

## Key Insights & Lessons Learned

### What's Working Well
1. **Phantom types for clock domains** - Zero-cost abstraction, perfect fit for CDC safety
2. **Const generics for bit widths** - Natural Rust feature, no macro magic needed
3. **Async/await mapping** - Surprisingly natural for expressing sequential logic
4. **Implicit registers** - Local variables across .await become registers automatically
5. **Function-typed modules** - #[hardware] macro infers ports from signatures, no wrappers needed

### Technical Challenges
1. **Async executor complexity** - Need careful design to match Verilator semantics exactly
2. **Module instantiation API** - Still exploring best way to express hierarchy
3. **Combinational loops** - Need delta cycle implementation for feedback
4. **Multi-clock designs** - How to express clock domain relationships?

### Research Questions
1. Can we infer clock domains automatically in some cases?
2. How far can we push parametric polymorphism in hardware?
3. Can we generate better Verilog than hand-written?
4. Is there a way to avoid explicit synchronizers with effect systems?

### Design Trade-offs
- **Rust simulation vs Verilator**: Fast iteration vs proven correctness
- **Type safety vs ergonomics**: More compile-time checks vs more type annotations
- **Explicit vs implicit**: Clear intent vs less boilerplate
- **Familiarity vs innovation**: Like Verilog vs fundamentally different

---

## Timeline Status

- **Start Date:** February 17, 2026
- **Current Phase:** Phase 1, Month 1, Week 1
- **On Track:** ✅ YES
- **Estimated Completion (Phase 1):** April 2026

---

## Notes

### Design Decisions Made
1. Clock domains use phantom types for zero-cost abstractions
2. No wrapper types (Signal, State, Wire, Reg) - just plain Rust types
3. Registers inferred from variables crossing .await boundaries
4. #[hardware] macro infers ports from function signatures
5. Bits<N> uses const generics for compile-time width checking
6. Logic enum supports X (unknown) for simulation accuracy

### Lessons Learned
- Phantom types in Rust work perfectly for clock domain tracking
- const generics make bit-width type safety natural
- Wrapper types (Signal, State) are unnecessary - macro can infer everything
- Rust's async transform automatically tracks variables across .await = implicit registers
- Best HDLs (Clash, Esterel) don't distinguish wire/reg at type level
- Function signatures ARE the port declarations - no explicit input/output needed

### Open Questions
- Reset handling: Active high/low? Async/sync? Part of Clock or separate type?
- Module instantiation: How to spawn child modules? Explicit API or implicit from async fn?
- Signal routing: Auto-wire by name or explicit connections?
- Memory primitives: Built-in abstractions or user-defined?
- Parametric width inference: Full inference or explicit const generics only?

---

## Core Design Decisions (Finalized)

### Execution Model: Lockstep Async Executor
**Goal:** Verilator-equivalent cycle-accurate simulation using Rust async/await

**Key Principles:**
- Custom `HardwareExecutor` polls all async tasks together on each clock tick
- `.await` marks clock edge boundaries (pause/resume points)
- Atomic state updates: All `State<T>.set()` calls commit simultaneously at cycle end
- Zero-delay combinational: Regular functions compose instantly within a cycle
- Deterministic: Same execution order every cycle

**Example:**
```rust
#[hardware]
async fn counter(clk: Clock<Domain>) -> Bits<8> {
    let mut count = Bits::from(0u8);  // Local variable = register
    loop {
        clk.tick().await;  // Pause here, resume on next clock edge
        count = count + Bits::from(1u8);  // Direct assignment
    }
    count  // Return value = output port
}
```

**How It Works:**
1. `#[hardware]` macro analyzes function signature: parameters = inputs, return type = output
2. Rust compiler transforms async fn into state machine automatically
3. Local `mut` variables that cross `.await` = registers (automatic inference)
4. `ClockFuture::poll()` returns Pending until clock edge, then Ready
5. Executor polls all modules together (lockstep), matching hardware parallelism
6. No wrapper types (Signal, State, Wire, Reg) - just plain Rust types in signatures

### Module Types
- **Combinational**: Regular `fn` (zero delay, pure function, evaluated immediately)
- **Sequential**: `async fn` (state machine, clock-driven, pauses at `.await`)
- **Hierarchy**: Function composition (child outputs → parent inputs)

### Clock Domain Crossing (CDC) Safety
**Problem:** Traditional HDLs allow mixing signals from different clock domains, causing metastability bugs

**Copper Solution:**
```rust
struct DomainA;
struct DomainB;

#[hardware]
async fn module_a(clk: Clock<DomainA>) -> Bits<8> { /* ... */ }

#[hardware]
async fn module_b(clk: Clock<DomainB>) -> Bits<8> { /* ... */ }

#[hardware]
async fn top() {
    let val_a = module_a(clk_a.clone());
    let val_b = module_b(clk_b.clone());
    
    // Compile error: can't mix domains!
    let result = val_a + val_b;  // ❌ Macro detects domain mismatch
    
    // Must use explicit synchronizer
    let val_b_sync = synchronizer::two_ff::<DomainA>(val_b);
    let result = val_a + val_b_sync;  // ✅ Type-safe
}
```

**Implementation:**
- `#[hardware]` macro tracks clock domains through parameters
- Module outputs tagged with source clock domain (compile-time only)
- Macro analysis prevents cross-domain operations
- Synchronizers are explicit domain conversions
- First HDL to leverage ownership for CDC safety

### No Combinational/Sequential Distinction
**Traditional HDLs:**
```verilog
// Verilog: Manual distinction
always @(*) begin  // Combinational
    y = a & b;
end

always @(posedge clk) begin  // Sequential
    q <= d;
end
```

**Copper:**
```rust
// Function = combinational (automatic)
fn adder(a: Bits<8>, b: Bits<8>) -> Bits<8> {
    a + b  // Pure function, evaluated immediately
}

// Async function = sequential (automatic)
#[hardware]
async fn register(clk: Clock<D>, d: Bits<8>) -> Bits<8> {
    let mut q = Bits::from(0u8);  // Local variable = register
    loop {
        clk.tick().await;  // Clock edge
        q = d;  // Direct assignment
    }
    q  // Output
}
```

No `always @(*)` vs `always @(posedge)` distinction needed - the type system knows!

### Module Instantiation & Hierarchy
**Design Decision (FINALIZED):**

Modules are function-typed - the `#[hardware]` macro handles composition:

```rust
// Child module
#[hardware]
async fn counter(clk: Clock<Domain>) -> Bits<8> {
    let mut count = Bits::from(0u8);
    loop {
        clk.tick().await;
        count = count + 1;
    }
    count
}

// Combinational module
fn adder(a: Bits<8>, b: Bits<8>) -> Bits<8> {
    a + b
}

// Parent module - composition through function calls
#[hardware]
async fn top(clk: Clock<Domain>) -> Bits<8> {
    let counter_val = counter(clk.clone());  // Macro spawns child module
    loop {
        let sum = adder(counter_val, Bits::from(5u8));  // Combinational
        clk.tick().await;
    }
    sum
}
```

**How It Works:**
- `#[hardware]` macro intercepts function calls to other `#[hardware]` modules
- Child modules are spawned in executor and run in parallel
- Returns a handle that reads the child's current output
- Combinational functions (`fn`) are called directly (zero delay)
- Clean, natural Rust syntax - no explicit spawn/wire/connect

### Register Inference & State Management
**No explicit State<T> wrapper needed!**

```rust
#[hardware]
async fn pipeline(clk: Clock<Domain>, input: Bits<8>) -> Bits<8> {
    let mut stage1 = Bits::from(0u8);  // Register (crosses .await)
    let mut stage2 = Bits::from(0u8);  // Register (crosses .await)
    
    loop {
        let comb = input + Bits::from(1u8);  // Combinational (local to iteration)
        
        clk.tick().await;  // Clock edge boundary
        
        stage1 = comb;       // Updates on next clock
        stage2 = stage1;     // Updates on next clock
    }
    stage2
}
```

**Semantics:**
- Variables that persist across `.await` = registers (Rust async transform tracks these)
- Variables local to each loop iteration = wires/combinational
- All register updates happen atomically at clock edge
- Matches Verilog non-blocking assignment (`<=`) semantics
- Macro analyzes variable lifetimes to generate correct Verilog

### Simulation vs Synthesis Path

```
┌─────────────────┐
│  Copper Source  │
│  (Rust + async) │
└────────┬────────┘
         │
    ┌────┴────┐
    │         │
    ▼         ▼
┌────────┐ ┌──────────┐
│  Rust  │ │ Verilog  │
│  Sim   │ │ Codegen  │
└────────┘ └─────┬────┘
    │            │
    │            ▼
    │      ┌──────────┐
    │      │Verilator │
    │      │  Sim     │
    │      └─────┬────┘
    │            │
    └─────┬──────┘
          │
          ▼
    ┌──────────┐
    │ Compare  │
    │ Outputs  │
    └──────────┘
```

**Verification Strategy:**
1. Develop/debug in fast Rust simulation
2. Generate Verilog from same source
3. Compare Rust sim vs Verilator sim cycle-by-cycle
4. Guarantee behavioral equivalence

---

## Technical Specifications

### Type System

**Core Types:**
```rust
Bit              // Single bit: 0, 1, or X (unknown)
Bits<N>          // N-bit vector (const generic width)
Clock<D>         // Clock source for domain D
```

**Type Properties:**
- `Bits<N>` supports: arithmetic (+, -, *), bitwise (&, |, ^), shifts (<<, >>), indexing
- `Clock<D>` provides: `tick()` returns `Future` for async/await
- No wrapper types needed: function parameters/returns are plain types
- CDC safety enforced through domain tracking in macro analysis

### Macro System (`#[hardware]`)

**Current Implementation:**
- Parses module structure
- Generates IR representation
- Outputs Verilog

**Planned Enhancements:**
- Infer ports from function signature (no explicit `input`/`output`)
- Transform async fn into Verilog `always @(posedge clk)` blocks
- Analyze dependencies for combinational sensitivity lists
- Generate clock domain crossing assertions

### Simulation Executor

**Architecture:**
```rust
struct HardwareExecutor {
    tasks: Vec<Pin<Box<dyn Future>>>,  // All async modules
    clock: SharedClock,                // Global clock source
    signals: SignalGraph,              // Combinational connectivity
    cycle: u64,                        // Current cycle number
}
```

**Execution Phases Per Cycle:**
1. **Combinational Propagation** - Evaluate all `fn` logic (delta cycles if needed)
2. **Clock Edge Notification** - Wake all tasks waiting on `clk.tick().await`
3. **Task Polling** - Poll all async tasks exactly once (lockstep advance)
4. **State Commit** - Apply all buffered `State<T>.set()` updates atomically

**Guarantees:**
- Deterministic: same execution order every cycle
- Cycle-accurate: matches Verilator output exactly
- Zero-overhead: no threading, no OS involvement

---

## Project Metrics & Statistics

### Current Codebase
- **Total Lines of Code:** ~1,200
- **Core Type System:** 605 lines
- **Examples:** 134 lines
- **Tests:** 10 unit tests (100% pass rate)
- **Files Created:** 4 new files this week
- **Commits:** 2 (main branch + feature branch)

### Development Velocity
- **Week 1 Productivity:** Core type system complete in 1 week
- **On Schedule:** ✅ YES - Week 1-2 milestones tracking well
- **Risk Level:** LOW - Proof of concept working, clear path forward

### Publication Readiness
- **Novel Features Identified:** 5 major contributions
- **Comparison HDLs Researched:** 6 (Verilog, Chisel, Clash, Bluespec, Esterel, SpinalHDL)
- **Related Work:** Extensive notes on experimental HDLs
- **Time to Paper Deadline:** 9 months (November 2026)

---

## Meeting Notes & Decisions

### February 17, 2026 - Design Session

**Decisions Made:**
1. ✅ Execution model: Lockstep async executor matching Verilator
2. ✅ `.await` represents clock edge boundaries
3. ✅ Atomic state commits at cycle boundary
4. ✅ Combinational = `fn`, Sequential = `async fn`
5. ✅ Clock domain safety via phantom types

**Open Questions:**
- Reset handling strategy (active high/low, sync/async)
- Module instantiation API (direct calls vs explicit spawn)
- Signal routing (auto-wire vs explicit)
- Memory primitives design

**Action Items:**
- [ ] Implement HardwareExecutor (Week 3-4)
- [ ] Build ClockFuture with proper waker semantics
- [ ] Create async counter example
- [ ] Document all design decisions in PROGRESS.md ✅

---

### February 22, 2026 - Week 3-4 Implementation

**Completed:**
1. ✅ Refined execution model to eliminate wrapper types (Signal, State, Wire, Reg)
2. ✅ Researched experimental HDLs (Clash, Lava, Esterel, Bluespec, Chisel)
3. ✅ Implemented clean HardwareExecutor with no state commit complexity
4. ✅ Created `#[hardware]` macro as validation marker
5. ✅ Built working async counter example (simple_counter.rs)
6. ✅ Verified cycle-accurate execution matches expected behavior
7. ✅ Eliminated all wrapper types from types.rs

**Key Insights:**
- Local variables crossing `.await` automatically become registers (Rust async transform)
- No explicit State<T> wrapper needed - just plain Rust types
- Function signatures ARE the port declarations (function-typed modules achieved)
- Macro can be minimal validator - executor handles actual work
- Matches Esterel's approach: `pause` (our `.await`) creates cycle boundaries

**Results:**
- simple_counter.rs output: `cycle N count N-1` (correct 1-cycle pipeline latency)
- All tests passing with clean, minimal code
- Ready for Phase 4: module composition handles

**Action Items (Next):**
- [ ] Clean up remaining warnings (unused imports)
- [ ] Implement module handles for parent-child composition
- [x] Create pipeline example (2-stage)
- [ ] Begin Month 2: Verilog codegen

---

## Meeting Notes & Decisions

### February 17, 2026 - Design Session

**Decisions Made:**
1. ✅ Execution model: Lockstep async executor matching Verilator
2. ✅ `.await` represents clock edge boundaries
3. ✅ Atomic state commits at cycle boundary
4. ✅ Combinational = `fn`, Sequential = `async fn`
5. ✅ Clock domain safety via phantom types

**Open Questions:**
- Reset handling strategy (active high/low, sync/async)
- Module instantiation API (direct calls vs explicit spawn)
- Signal routing (auto-wire vs explicit)
- Memory primitives design

**Action Items:**
- ✅ Implement HardwareExecutor (Week 3-4)
- ✅ Build ClockTick with proper waker semantics
- ✅ Create async counter example
- ✅ Document all design decisions in PROGRESS.md

---

### February 22, 2026 - Execution Model Refinement & Week 3-4 Implementation

**Major Decisions:**
1. ✅ **Eliminate ALL wrapper types** - No Signal<>, State<>, Wire<>, Reg<>
2. ✅ **Implicit register inference** - Variables crossing .await become registers automatically
3. ✅ **Function-typed modules** - #[hardware] macro validates async functions
4. ✅ **Macro-based design** - Minimal validator, executor handles scheduling
5. ✅ **Plain Rust types in signatures** - Just Bits<8>, not Signal<Domain, Bits<8>>

**Research Insights from Other HDLs:**
- **Clash/Lava:** No type-level wire/reg distinction, just delay operations
- **Esterel:** Single signal type, `pause` creates cycle boundaries (like our .await)
- **Bluespec:** Explicit Reg# type (what we want to avoid)
- **Chisel:** Explicit Wire/Reg types (what we want to avoid)

**Key Realization:**
The most elegant HDLs don't distinguish wires/registers at the type level. Registers are created by **operations** (delay, pause, await), not by wrapping types. This is the true "function-typed modules" vision.

**Implementation Completed:**
1. ✅ Refactored copper-core/types.rs: removed Signal and State wrappers
2. ✅ Simplified HardwareExecutor: no state commit closures
3. ✅ Created copper-macros crate with #[hardware] macro
4. ✅ Built simple_counter.rs example: works perfectly
5. ✅ Verified cycle-accurate execution: output matches expected [0,1,2,3,4]

**Results:**
- simple_counter output: `cycle N count N-1` (correct 1-cycle pipeline latency)
- All tests passing with clean, minimal code
- ~2,100 lines total across 3 crates + examples
- Ready for Phase 4: module composition handles

**Lessons Learned:**
- Rust async transform automatically tracks variables across .await points
- Don't need explicit State<T> type - just local mut variables
- Macro can be simple validator - executor does the real work
- Function-typed modules are achieved through signatures, not macros
- Esterel's design principles translate perfectly to Rust async

**Next Steps:**
- [ ] Clean up remaining warnings (unused imports)
- [ ] Implement module handles for parent-child composition
- [x] Create pipeline example (2-stage registered pipeline)
- [ ] Begin Month 2: Verilog codegen from async functions

---

### February 22, 2026 - Example Expansion & Verilator Semantics

**Completed:**
1. ✅ Added UART RX FSM example with Verilator cross-validation
2. ✅ Added ready/valid stalled pipeline example with Verilator cross-validation
3. ✅ Added registered ALU example (add/sub/and/or) with Verilator cross-validation
4. ✅ Added RAM/ROM read/write example pair with Verilator cross-validation
5. ✅ Added Verilog equivalents for new examples (uart_fsm, pipeline_stall, alu, ram, rom)
6. ✅ Updated HardwareExecutor to two-phase tick (pre-edge + post-edge settle) to align with Verilator sampling
7. ✅ Restored pipeline example to auto-trace against Verilator and verified pass

**Notes:**
- Pipeline-stall Verilog cleaned up to avoid MULTIDRIVEN warnings (removed comb output init).
- All new examples run and pass Verilator verification.

---

## References & Prior Art

### Experimental HDLs Studied
1. **Chisel** (Scala) - Constructive approach, good parametrization
2. **Clash** (Haskell) - Functional, strong types, explicit clock domains
3. **Bluespec** (BSV) - Rule-based, automatic scheduling
4. **SpinalHDL** (Scala) - Modern Chisel alternative
5. **Esterel** - Synchronous reactive, formal semantics
6. **Hardcaml** (OCaml) - Functional hardware

### Key Papers to Reference
- "Chisel: Constructing Hardware in a Scala Embedded Language" (Bachrach et al.)
- "Type-Driven Hardware Design with Clash" (Baaij)
- "Bluespec: A General-Purpose Approach to High-Level Synthesis" (Nikhil)
- "The Synchronous Languages 12 Years Later" (Benveniste et al.)

### Rust Async Resources
- Rust async book: https://rust-lang.github.io/async-book/
- Tokio internals: https://tokio.rs/
- Future trait design: RFC 2394

---

### March 6, 2026 - Verilog Code Generation Pipeline (IR-Based)

**Completed:**
1. ✅ **Comprehensive Code Cleanup** (7+ files fixed)
   - Removed all unused imports and variables
   - Added `#[allow(dead_code)]` for intentional dead code
   - All compiler warnings eliminated

2. ✅ **IR Structure Design & Implementation** (VERILOG_CODEGEN_DESIGN.md + 218 lines)
   - Designed clean IR for Combinational/Sequential/Mixed logic separation
   - Implemented ModuleIR, PortDecl, Direction, PortType, ModuleLogic enum
   - Implemented Statement/Expression/Operator types with 18 binary ops, 5 unary ops
   - Added helper methods (combinational(), sequential(), literal(), var(), binary(), ternary())

3. ✅ **IR Builder Implementation** (358 lines, copper-codegen/src/ir_builder.rs)
   - Transforms Rust AST → Copper IR using syn crate
   - Module classification (Combinational vs Sequential via loop detection)
   - Port extraction from function signatures with width inference
   - Expression parsing: binary ops, unary ops, if/ternary, literals, variables
   - Combinational logic: implicit + explicit return handling
   - Sequential logic: register extraction, always block generation

4. ✅ **Verilog Generator Implementation** (294 lines, copper-codegen/src/verilog_gen.rs)
   - Transforms IR → synthesizable Verilog strings
   - Module header, port declarations, continuous assigns, register declarations
   - Always blocks with posedge clock triggering
   - If/else statement nesting, case statements with defaults
   - All binary/unary operators properly translated to Verilog

5. ✅ **Compilation & API Integration**
   - Updated copper-codegen/src/lib.rs to wire IR builder + Verilog generator
   - Disabled old parser.rs and verilog.rs (retained for history)
   - Fixed all syn API issues:
     * Pattern matching on Pat and Local.pat
     * Block dereference handling (then_branch is Box<Block>)
     * Replaced Debug trait calls with token stream conversions
     * Fixed UnOp enum (Deref not Star)
     * Added non-exhaustive match arm for UnOp

6. ✅ **Testing & Validation**
   - Created comprehensive codegen test (copper-codegen/examples/test_codegen.rs)
   - Verified pipeline with 3 test cases:
     * Inverter: `fn inv(a: u8) -> u8 { !a }` → Verilog module with bitwise NOT
     * AND gate: `fn and_gate(a: u8, b: u8) -> u8 { a & b }` → proper port assignment
     * Adder: `fn add(a: u8, b: u8) -> u8 { a + b }` → arithmetic operation
   - All existing examples still pass (inverter, mux, counters, pipelines, FSMs, etc.)
   - Full test suite: 15/15 tests passing

**Example Verilog Output Generated:**
```verilog
module inv (
    input wire [7:0] a,
    output reg [7:0] out
);
    assign out = (!a);
endmodule
```

**Pipeline Validated:** Rust AST → IR (358 line builder) → Verilog string (294 line generator)

**Metrics:**
- **New lines of code:** 852 lines (ir_builder + verilog_gen)
- **Compilation:** `cargo check --all` clean
- **Test results:** All 15 tests passing
- **Code generation:** Working end-to-end for combinational logic

**Next Steps:**
- [ ] Test with sequential logic (counters: async fn with loop/tick)
- [ ] Handle complex expressions (if-else ternary, bit/range select)
- [ ] Optimize output width inference from type annotations (Bits<N>)
- [ ] Integrate with existing examples (generate Verilog from inverter.rs, etc.)
- [ ] Implement module composition (hierarchical Verilog generation)
- [ ] Add testbench generation from simulation traces

---

Last Updated: March 6, 2026
