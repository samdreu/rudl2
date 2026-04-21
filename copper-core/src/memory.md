# Memory Parameters

## Addressing and Timing
- Address space
- Latency
    - Would simulating latency be useful?
    - Should latency between different modules also be modeled?
- Synchronization modes

## Port Types
- Read/write ports
- How many ports are needed?
- Can ports be used simultaneously?

## Programmer-Facing Abstraction
- How should synchronization be managed?
- What should the interface look like?
    - Module
    - Function call
    - Array access
- What are the pros and cons of the chosen interface?

## Simulation Model
- Model memory as a Rust function for simulation.
- The transpilation process should select the appropriate instantiation.

## Verilog Modeling
- How are memories currently modeled in Verilog?
    - Arrays of registers?
    - Separate modules?
- What are the issues with the current approach?

dont think about how many accesses at a time