# Verilog Code Generation Design

## Goal
Transform Rust function-typed modules into synthesizable Verilog modules while preserving cycle-accurate behavior.

## Current System Overview

### Function-Typed Modules (What We Have Today)
```rust
#[hardware(function_typed)]
async fn counter(clk: Clock<MainClk>, increment: u128) -> Bits<8> {
    let mut count = Bits::<8>::from_u128(0);
    loop {
        emit!(count.clone());
        clk.tick().await;
        count = count + Bits::<8>::from_u128(increment);
    }
}
```

This needs to generate:
```verilog
module counter(
    input wire clk,
    input wire [127:0] increment,
    output reg [7:0] out
);
    reg [7:0] count;
    
    always @(posedge clk) begin
        out <= count;
        count <= count + increment[7:0];
    end
endmodule
```

## Code Generation Pipeline

### Phase 1: Parse Rust AST → Copper IR
**Input:** Rust function signature + body  
**Output:** Copper IR (enhanced version)

```
Rust Function → syn::ItemFn → IR Builder → ModuleIR
```

**Key transformations:**
1. Function parameters → module inputs
2. Return type → module output
3. `let mut` variables → registers
4. `loop { emit!(); clk.tick().await; ... }` → always @(posedge clk)
5. Combinational assignments → wire assignments
6. `if/else/match` → Verilog control flow

### Phase 2: Copper IR → Verilog AST
**Input:** Copper IR  
**Output:** Structured Verilog representation

### Phase 3: Verilog AST → String
**Input:** Verilog AST  
**Output:** Synthesizable Verilog code

## Enhanced IR Structure

### Module Representation
```rust
pub struct ModuleIR {
    pub name: String,
    pub ports: Vec<PortDecl>,
    pub logic: ModuleLogic,
}

pub struct PortDecl {
    pub name: String,
    pub direction: Direction,
    pub width: usize,
    pub ty: PortType,
}

pub enum Direction {
    Input,
    Output,
}

pub enum PortType {
    Wire,
    Reg,
}
```

### Module Logic Types
```rust
pub enum ModuleLogic {
    // Pure combinational (no async, no state)
    Combinational {
        expressions: Vec<ContinuousAssign>,
    },
    
    // Sequential (async fn with loop/await)
    Sequential {
        clock: String,
        reset: Option<String>,
        registers: Vec<RegisterDecl>,
        always_block: AlwaysBlock,
    },
    
    // Mixed (rare, but possible)
    Mixed {
        clock: String,
        registers: Vec<RegisterDecl>,
        combinational: Vec<ContinuousAssign>,
        sequential: AlwaysBlock,
    },
}

pub struct RegisterDecl {
    pub name: String,
    pub width: usize,
    pub initial: Option<Expression>,
}

pub struct ContinuousAssign {
    pub target: String,
    pub value: Expression,
}

pub struct AlwaysBlock {
    // List of statements inside always @(posedge clk)
    pub statements: Vec<Statement>,
}
```

### Statement Types
```rust
pub enum Statement {
    // Non-blocking assignment (<=)
    NonBlockingAssign {
        target: String,
        value: Expression,
    },
    
    // Blocking assignment (=)
    BlockingAssign {
        target: String,
        value: Expression,
    },
    
    // if/else
    If {
        condition: Expression,
        then_branch: Vec<Statement>,
        else_branch: Option<Vec<Statement>>,
    },
    
    // case statement
    Case {
        selector: Expression,
        arms: Vec<CaseArm>,
        default: Option<Vec<Statement>>,
    },
}

pub struct CaseArm {
    pub pattern: Pattern,
    pub body: Vec<Statement>,
}
```

### Expression Types
```rust
pub enum Expression {
    // Variable reference
    Var(String),
    
    // Literal value
    Literal {
        width: usize,
        value: u128,
    },
    
    // Binary operations
    BinaryOp {
        left: Box<Expression>,
        op: BinaryOp,
        right: Box<Expression>,
    },
    
    // Unary operations
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expression>,
    },
    
    // Bit selection
    BitSelect {
        signal: Box<Expression>,
        index: Box<Expression>,
    },
    
    // Range selection [high:low]
    RangeSelect {
        signal: Box<Expression>,
        high: Box<Expression>,
        low: Box<Expression>,
    },
    
    // Concatenation {a, b, c}
    Concat(Vec<Expression>),
    
    // Conditional (ternary)
    Ternary {
        condition: Box<Expression>,
        then_val: Box<Expression>,
        else_val: Box<Expression>,
    },
}

pub enum BinaryOp {
    Add, Sub, Mul, Div, Mod,
    And, Or, Xor,
    LogicalAnd, LogicalOr,
    Eq, Neq, Lt, Lte, Gt, Gte,
    ShiftLeft, ShiftRight,
}

pub enum UnaryOp {
    Not,        // ~
    LogicalNot, // !
    ReductionAnd, ReductionOr, ReductionXor,
}
```

## Parsing Strategy

### Step 1: Identify Module Type
```rust
fn classify_module(func: &syn::ItemFn) -> ModuleType {
    // Check if async
    if func.sig.asyncness.is_some() {
        // Check for loop + tick().await pattern
        if has_main_loop_with_tick(&func.block) {
            ModuleType::Sequential
        } else {
            ModuleType::AsyncCombinational
        }
    } else {
        ModuleType::Combinational
    }
}
```

### Step 2: Extract Ports
```rust
fn extract_ports(func: &syn::ItemFn) -> Vec<PortDecl> {
    let mut ports = vec![];
    
    // Inputs from parameters
    for param in &func.sig.inputs {
        if let syn::FnArg::Typed(pat_type) = param {
            let name = extract_param_name(pat_type);
            let ty = extract_param_type(pat_type);
            
            // Skip clock parameters
            if !is_clock_type(&ty) {
                ports.push(PortDecl {
                    name,
                    direction: Direction::Input,
                    width: extract_width(&ty),
                    ty: PortType::Wire,
                });
            }
        }
    }
    
    // Output from return type
    if let syn::ReturnType::Type(_, ty) = &func.sig.output {
        let width = extract_width(ty);
        ports.push(PortDecl {
            name: "out".to_string(),
            direction: Direction::Output,
            width,
            ty: PortType::Reg, // Sequential modules output regs
        });
    }
    
    ports
}
```

### Step 3: Extract Registers
```rust
fn extract_registers(block: &syn::Block) -> Vec<RegisterDecl> {
    let mut registers = vec![];
    
    for stmt in &block.stmts {
        if let syn::Stmt::Local(local) = stmt {
            // Check for `let mut variable = initial_value;`
            if local.init.is_some() {
                let name = extract_local_name(local);
                let init = local.init.as_ref().map(|(_, expr)| 
                    parse_expression(expr)
                );
                
                registers.push(RegisterDecl {
                    name,
                    width: infer_width(local),
                    initial: init,
                });
            }
        }
    }
    
    registers
}
```

### Step 4: Parse Loop Body
For sequential modules, the main loop structure is:
```rust
loop {
    emit!(output_expression);
    clk.tick().await;
    // ... state updates ...
}
```

Transform to:
```verilog
always @(posedge clk) begin
    out <= output_expression;
    // ... state updates ...
end
```

**Key rule:** Statements before `tick().await` use non-blocking (`<=`), statements after use blocking (`=`) or non-blocking depending on context.

## Example Transformations

### Example 1: Simple Counter
**Rust:**
```rust
async fn counter(clk: Clock<MainClk>) -> Bits<8> {
    let mut count = Bits::<8>::from_u128(0);
    loop {
        emit!(count.clone());
        clk.tick().await;
        count = count + Bits::<8>::from_u128(1);
    }
}
```

**IR:**
```rust
ModuleIR {
    name: "counter",
    ports: vec![
        PortDecl { name: "clk", direction: Input, width: 1, ty: Wire },
        PortDecl { name: "out", direction: Output, width: 8, ty: Reg },
    ],
    logic: Sequential {
        clock: "clk",
        reset: None,
        registers: vec![
            RegisterDecl { name: "count", width: 8, initial: Some(Literal(8, 0)) }
        ],
        always_block: AlwaysBlock {
            statements: vec![
                NonBlockingAssign { target: "out", value: Var("count") },
                NonBlockingAssign { 
                    target: "count", 
                    value: BinaryOp(Var("count"), Add, Literal(8, 1))
                },
            ]
        }
    }
}
```

**Verilog:**
```verilog
module counter(
    input wire clk,
    output reg [7:0] out
);
    reg [7:0] count;
    
    initial begin
        count = 8'd0;
    end
    
    always @(posedge clk) begin
        out <= count;
        count <= count + 8'd1;
    end
endmodule
```

### Example 2: Mux (Combinational)
**Rust:**
```rust
fn mux(sel: Bit, a: Bits<8>, b: Bits<8>) -> Bits<8> {
    match sel {
        Bit::ZERO => a,
        Bit::ONE => b,
        Bit::X => Bits::<8>::zero(),
    }
}
```

**IR:**
```rust
ModuleIR {
    name: "mux",
    ports: vec![
        PortDecl { name: "sel", direction: Input, width: 1, ty: Wire },
        PortDecl { name: "a", direction: Input, width: 8, ty: Wire },
        PortDecl { name: "b", direction: Input, width: 8, ty: Wire },
        PortDecl { name: "out", direction: Output, width: 8, ty: Wire },
    ],
    logic: Combinational {
        expressions: vec![
            ContinuousAssign {
                target: "out",
                value: Ternary {
                    condition: Var("sel"),
                    then_val: Var("b"),
                    else_val: Var("a"),
                }
            }
        ]
    }
}
```

**Verilog:**
```verilog
module mux(
    input wire sel,
    input wire [7:0] a,
    input wire [7:0] b,
    output wire [7:0] out
);
    assign out = sel ? b : a;
endmodule
```

## Implementation Plan

### Phase 1: Combinational Logic (This Week)
1. Implement IR for combinational modules
2. Parse simple Rust functions → IR
3. Generate Verilog from IR
4. Test with: inverter, mux examples

### Phase 2: Sequential Logic (Next Week)
1. Implement IR for sequential modules
2. Parse async functions with loop/await
3. Generate always @(posedge clk) blocks
4. Test with: simple_counter, async_counter

### Phase 3: Complex FSMs (Week 3)
1. Handle match statements → case statements
2. Handle multiple state variables
3. Generate proper FSM encoding
4. Test with: uart_fsm, mealy, pipeline examples

### Phase 4: Validation (Week 4)
1. Generate Verilog for all 14 examples
2. Run Verilator on generated code
3. Compare Rust sim vs Verilator sim cycle-by-cycle
4. Document any differences/limitations

## Design Decisions

### Why separate Combinational vs Sequential IR?
- Combinational modules use `assign` statements (continuous)
- Sequential modules use `always @(posedge clk)` (triggered)
- Different syntax rules and semantics in Verilog

### How to handle emit!()?
- `emit!(expr)` becomes non-blocking assignment to output
- Position matters: before tick().await means "output this cycle"
- Multiple emit!() in one loop iteration = last one wins

### How to handle state updates?
- Assignments after tick().await become state updates
- Use non-blocking assignments (<= ) for all register updates
- Preserves cycle-accurate semantics from Rust simulation

### Type inference
- Use Rust type information from syn::Type
- Match Bits<N> → [N-1:0] wire/reg
- Match Bit → wire/reg (single bit)
- Tuples become multiple outputs

## Next Steps
1. Implement new IR structure in copper-core/src/ir.rs
2. Build IR parser in copper-codegen/src/ir_builder.rs
3. Build Verilog generator in copper-codegen/src/verilog_gen.rs
4. Add tests for each transformation
