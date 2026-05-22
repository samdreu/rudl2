# RV32I CPU: Latency-Insensitive Memory Interface Design

## Design Principle

The CPU never assumes fixed memory latencies. Instead, it uses **ready/valid handshaking** (like real hardware):

```
CPU: Issue request → Poll is_ready() → When ready, capture data
Memory: Accept request → Process → Signal is_ready() → Provide data
```

This design automatically works with **ANY memory latency configuration**.

## How to Change Latencies

Change only the Memory type parameters. CPU logic never changes:

```rust
// Current (2-cycle IMEM, 2-cycle DMEM reads, 1-cycle writes)
let imem = Memory::<u32, 1, 0, MainClk, 2, 1>::from_contents(clk.clone(), program);
let dmem = Memory::<u32, 1, 1, MainClk, 2, 1>::new(clk.clone(), 1024);
let regfile = Memory::<u32, 2, 1, MainClk, 1, 1>::new(clk.clone(), 32);

// To change to 5-cycle reads, just modify the READ_LAT parameter:
let imem = Memory::<u32, 1, 0, MainClk, 5, 1>::from_contents(clk.clone(), program);

// CPU code doesn't change - it automatically polls is_ready() until satisfied
```

## Core Pattern: is_ready() Polling

Every memory operation follows this pattern:

```rust
// Issue read request
memory.read_port::<0>().read(address);

// Poll until ready (latency-insensitive)
loop {
    clk.tick().await;
    if memory.read_port::<0>().is_ready() {
        break;
    }
}

// Now data is available
let data = memory.read_port::<0>().data();
```

This works identically whether the memory has 1-cycle or 10-cycle latency.

## ECE437 Design Pattern

This design follows ECE437's modular interface-based approach:

### ECE437 (SystemVerilog):
```systemverilog
interface alu_if;
    logic [31:0] a, b, result;
    // modports define CPU-side vs ALU-side views
endinterface

module cpu (alu_if.cpu alu_port);
    alu_port.a = operand_a;
    alu_port.b = operand_b;
    result = alu_port.result;
endmodule
```

### Copper (Rust equivalent):
```rust
// Memory interface traits (like modports)
pub trait ReadOp { 
    fn issue_read(&self, addr: usize);
    fn read_ready(&self) -> bool;
    fn read_data(&self) -> u32;
}

// CPU uses interface, not implementation
match decoded.opcode {
    Opcode::LOAD => {
        regfile.read_port::<0>().read(rs1);
        loop {
            clk.tick().await;
            if regfile.read_port::<0>().is_ready() { break; }
        }
        let value = regfile.read_port::<0>().data();
        // ... CPU logic continues identically for ANY regfile latency
    }
}
```

## Key Advantages

1. **Latency Independence**: CPU works with ANY memory latency without modification
2. **Hardware Realism**: Uses ready/valid handshaking like real hardware
3. **Modularity**: Memory implementation details hidden from CPU
4. **Correctness**: No hardcoded cycle counts (no off-by-one errors)
5. **Flexibility**: Easy to experiment with different latencies

## Memory Type Parameters Explained

```rust
Memory::<ValueType, ReadPorts, WritePorts, ClockDomain, READ_LAT, WRITE_LAT>
```

- **ValueType** (`u32`): Data width
- **ReadPorts** (`1`, `2`): Number of simultaneous read ports
- **WritePorts** (`0`, `1`): Number of simultaneous write ports
- **ClockDomain** (`MainClk`): Clock used for pipeline stages
- **READ_LAT** (`1`, `2`, `5`): Cycles from request to data available
- **WRITE_LAT** (`1`, `2`): Cycles from request to write complete

Example IMEM configurations:
- `Memory::<u32, 1, 0, MainClk, 1, 1>` - 1-cycle read latency (fast)
- `Memory::<u32, 1, 0, MainClk, 2, 1>` - 2-cycle read latency (current)
- `Memory::<u32, 1, 0, MainClk, 10, 1>` - 10-cycle read latency (slow)

**CPU code changes in none of these cases.**

## Implementation Details

The CPU uses Copper's built-in Memory ports:
- `read_port::<N>()` - Access read port N
- `write_port::<N>()` - Access write port N
- `.read(addr)` - Stage address for reading
- `.is_ready()` - Poll if data is available (works regardless of latency)
- `.data()` - Get data when ready
- `.write(addr, value)` - Stage write request

Internally, Copper maintains a pipeline:
- Request enters at stage 0
- Data emerges at stage [READ_LAT-1]
- CPU polls `is_ready()` which checks if output stage has valid data

## Next Steps

To experiment with different latencies:

1. Change `Memory` type parameters (e.g., `2, 1` → `5, 1` for 5-cycle reads)
2. Run the same CPU code
3. CPU automatically adapts via `is_ready()` polling
4. Verify correctness unchanged

Example modification in [examples/rv32i_cpu_latency_insensitive.rs](rv32i_cpu_latency_insensitive.rs):

```rust
// Try different latencies here:
let imem    = Memory::<u32, 1, 0, MainClk, 5, 1>::from_contents(clk.clone(), program);  // 5-cycle reads
let dmem    = Memory::<u32, 1, 1, MainClk, 3, 2>::new(clk.clone(), 1024);               // 3-cycle reads, 2-cycle writes
let regfile = Memory::<u32, 2, 1, MainClk, 2, 1>::new(clk.clone(), 32);                 // 2-cycle register reads
```

The CPU will automatically work correctly with these new latencies.
