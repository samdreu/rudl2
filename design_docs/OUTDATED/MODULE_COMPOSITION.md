# Module Composition in Copper HDL

**Type convention:** New hardware-facing code should use `Logic` and `Bits<N>` at module boundaries. The `Arc<Mutex<T>>` examples below describe the older raw signal style and remain useful for understanding composition, but they should be read as legacy patterns rather than the preferred new-code interface.

## Current Status

**What Works Today:**
- Multiple independent modules via `exec.spawn()` (see `independent_counters.rs`)
- Shared signals via `Arc<Mutex<T>>` for inter-module communication
- All modules run in lockstep on same clock domain

**Example (Current Pattern):**
```rust
let shared_signal = Arc::new(Mutex::new(0u8));
exec.spawn(producer(clk.clone(), Arc::clone(&shared_signal)));
exec.spawn(consumer(clk.clone(), Arc::clone(&shared_signal)));
```

## Design Goals for Module Composition

### 1. **Hierarchical Modules** (Parent instantiates child)
```rust
#[hardware]
async fn parent(clk: Clock<MainClk>, input: Arc<Mutex<u8>>, output: Arc<Mutex<u8>>) {
    let child_out = Arc::new(Mutex::new(0u8));
    
    // Spawn child as sub-module
    let child_handle = spawn_child(child(clk.clone(), input.clone(), child_out.clone()));
    
    loop {
        // Use child's output
        let val = *child_out.lock().unwrap();
        *output.lock().unwrap() = val + 1;
        clk.tick().await;
    }
}
```

### 2. **Module Handles** (For control/inspection)
```rust
struct ModuleHandle<T> {
    future: Pin<Box<dyn Future<Output = T>>>,
    name: String,
    // Potential: access to internal signals for debug
}
```

### 3. **Static Hierarchy** (Compile-time module tree)
For Verilog generation, we need to know the full module hierarchy statically:
```rust
#[hardware(modules = [child_a, child_b])]
async fn parent(...) {
    // Macro validates that child_a and child_b are spawned
}
```

## Three Approaches to Module Composition

### Approach 1: **Current Pattern (Flat spawn)**
**Pros:**
- Already works
- Simple executor
- No special machinery needed

**Cons:**
- No hierarchy (all modules are peers)
- Hard to map to nested Verilog modules
- No parent-child relationship for codegen

**Best for:** Peer modules at same hierarchy level

---

### Approach 2: **Explicit Handles**
```rust
#[hardware]
async fn parent(clk: Clock<MainClk>, input: Arc<Mutex<u8>>) {
    let wire1 = Arc::new(Mutex::new(0u8));
    let wire2 = Arc::new(Mutex::new(0u8));
    
    // Spawn children and get handles
    let child1 = spawn_module!(child_a(clk.clone(), input.clone(), wire1.clone()));
    let child2 = spawn_module!(child_b(clk.clone(), wire1.clone(), wire2.clone()));
    
    loop {
        // Parent logic
        clk.tick().await;
    }
}
```

**Pros:**
- Clear parent-child relationship
- Can map to Verilog module instantiation
- Handles allow inspection/control

**Cons:**
- More complex executor (needs hierarchy tracking)
- Macro must parse and validate module structure

**Best for:** Hierarchical designs with clear parent-child

---

### Approach 3: **Implicit Composition (Function Calls)**
```rust
#[hardware]
async fn parent(clk: Clock<MainClk>, input: u8) -> u8 {
    let stage1 = child_a(input);  // Combinational
    let mut stage2_reg = 0u8;
    
    loop {
        // Output previous cycle's value
        let output = stage2_reg;
        clk.tick().await;
        
        // Compute for next cycle
        stage2_reg = child_b(stage1);  // Combinational
    }
    output
}
```

**Pros:**
- Most natural Rust syntax
- Combinational children are just function calls
- Sequential children need explicit spawn

**Cons:**
- Mixing combinational and sequential feels inconsistent
- Hard to generate correct Verilog hierarchy

**Best for:** Small combinational sub-functions

---

## Recommended Hybrid Approach

**For Copper, use a combination:**

### 1. Combinational Modules = Functions
```rust
fn add_one(x: u8) -> u8 { x + 1 }

#[hardware]
async fn sequential_user(clk: Clock<MainClk>, input: Arc<Mutex<u8>>) {
    loop {
        let val = *input.lock().unwrap();
        let result = add_one(val);  // Just call it
        clk.tick().await;
    }
}
```

### 2. Sequential Modules = spawn_child!()
```rust
#[hardware]
async fn parent(clk: Clock<MainClk>, input: Arc<Mutex<u8>>, output: Arc<Mutex<u8>>) {
    let wire = Arc::new(Mutex::new(0u8));
    
    // Spawn child as sub-module
    spawn_child!(child_module(clk.clone(), input.clone(), wire.clone()));
    
    loop {
        *output.lock().unwrap() = *wire.lock().unwrap();
        clk.tick().await;
    }
}
```

### 3. Peer Modules = exec.spawn()
```rust
fn main() {
    let mut exec = HardwareExecutor::new();
    exec.spawn(module_a(...));
    exec.spawn(module_b(...));
    exec.tick_clock(&mut clk);
}
```

## Implementation Plan

### Phase 1: spawn_child!() Macro ✅ NEXT
```rust
macro_rules! spawn_child {
    ($module:expr) => {
        // During simulation: just spawn normally
        // During codegen: record as child instance
        $module
    };
}
```

### Phase 2: Hierarchy Tracking
Add to executor:
```rust
struct ModuleInfo {
    name: String,
    parent: Option<String>,
    children: Vec<String>,
}

impl HardwareExecutor {
    pub fn spawn_child(&mut self, name: &str, parent: &str, future: impl Future) {
        self.module_tree.add_child(parent, name);
        self.spawn(future);
    }
}
```

### Phase 3: Verilog Generation
When generating Verilog:
1. Walk module tree from root
2. Generate each module with its children as instances
3. Wire connections from Arc<Mutex<T>> become wire declarations

## Implications

### For Rust Simulation
- **No change** to current executor (still lockstep polling)
- `spawn_child!()` is syntactic sugar for `spawn()` during sim
- Hierarchy is metadata only

### For Verilog Generation
- **Critical:** Need static hierarchy to generate nested modules
- Each `#[hardware]` function becomes a Verilog module
- `spawn_child!()` calls become module instantiations
- Arc<Mutex<T>> become wire declarations

### For Type Safety
- Clock domain safety already works (Clock<Domain> phantom type)
- No additional type constraints needed for hierarchy
- Parent and child must share same clock domain (enforced by type system)

### For Async Semantics
- Parent and children all run in **same event loop**
- All tick on same clock edge (lockstep)
- No "await child" needed—children run automatically
- Parent cannot "call" child's async fn—must spawn

### For Function-Typed Sequential Modules (Planned)
To keep sequential modules function-typed, the `#[hardware]` macro will support a `yield`-style output inside the loop:

```rust
#[hardware]
async fn module(clk: Clock<MainClk>, input: u8) -> u8 {
    let mut reg = 0u8;
    loop {
        let comb_out = reg.wrapping_add(1); // combinational output from reg
        yield comb_out;                     // output this cycle

        clk.tick().await;                  // sequential edge
        reg = input;                        // update state
    }
}
```

Macro desugaring rules:
- `yield expr;` becomes `*__output.lock().unwrap() = expr;`
- function parameters become input reads via `Arc<Mutex<T>>`
- `clk.tick().await` remains the edge boundary

This avoids `tick_with_output(...)` limitations when outputs come from internal combinational logic.

## Example: 2-Stage Pipeline (Refactored)

**Current monolithic version:**
```rust
async fn registered_pipeline(clk, in_data, out_data) {
    let mut stage1_reg = 0;
    let mut stage2_reg = 0;
    loop {
        *out_data = stage2_reg;
        clk.tick().await;
        let input = *in_data;
        stage1_reg = input + 1;
        stage2_reg = stage1_reg + stage1_reg;
    }
}
```

**Refactored with child modules:**
```rust
#[hardware]
async fn stage1(clk: Clock<MainClk>, input: Arc<Mutex<u8>>, output: Arc<Mutex<u8>>) {
    let mut reg = 0u8;
    loop {
        *output.lock().unwrap() = reg;
        clk.tick().await;
        reg = input.lock().unwrap().wrapping_add(1);
    }
}

#[hardware]
async fn stage2(clk: Clock<MainClk>, input: Arc<Mutex<u8>>, output: Arc<Mutex<u8>>) {
    let mut reg = 0u8;
    loop {
        *output.lock().unwrap() = reg;
        clk.tick().await;
        let val = *input.lock().unwrap();
        reg = val.wrapping_add(val);
    }
}

#[hardware]
async fn pipeline(clk: Clock<MainClk>, in_data: Arc<Mutex<u8>>, out_data: Arc<Mutex<u8>>) {
    let wire = Arc::new(Mutex::new(0u8));
    
    spawn_child!(stage1(clk.clone(), in_data.clone(), wire.clone()));
    spawn_child!(stage2(clk.clone(), wire.clone(), out_data.clone()));
    
    // Pipeline doesn't need its own logic—just instantiates children
    loop {
        clk.tick().await;
    }
}
```

## Open Questions

1. **Should parent wait for children to complete?**
   - No—all run in lockstep, no waiting needed
   
2. **Can parent read child's internal state?**
   - Only through explicit output wires (Arc<Mutex<T>>)
   
3. **How to handle module parameters?**
   - Rust const generics: `async fn child<const N: usize>(...)`
   
4. **How to reset child modules?**
    - Add reset signal as explicit input: `reset: Arc<Mutex<Logic>>`

## Next Steps

1. Create `spawn_child!()` macro in copper-macros
2. Add hierarchy tracking to HardwareExecutor
3. Create hierarchical pipeline example
4. Verify Verilator generation with nested modules
5. Document patterns in examples/
6. Extend `#[hardware]` macro to support `yield` for sequential function-typed outputs

---

**Status:** Design complete, ready for implementation
**Priority:** High (needed for Month 2: Verilog codegen)
**Estimated Effort:** 2-3 days
