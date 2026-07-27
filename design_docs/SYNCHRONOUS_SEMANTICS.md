pre-edge settle
clock edge
post-edge settle
post-edge observation
Include examples for:

simple counter,
combinational passthrough,
register with enable,
two clk.tick().awaits,
Out vs RegOut,
memory read timing.


Every clk.tick().await is a clock-cycle boundary; every suspension point becomes an FSM state; every value live across an await becomes a register; every path through a hardware loop must eventually reach an await; and simulation must be independent of Rust async poll order.

Hardware timing must be defined by Copper’s FSM/cycle semantics, not by Rust future poll order.

poll_tasks order must be an implementation detail.
Well-formed Copper designs must simulate identically under any task order.

In #[hardware] code, the only allowed await should be direct clk.tick().await
or a small set of Copper-defined hardware waits such as channel.read().await.

Be careful about non-blocking channels and FIFOs

## Maybe??
Rust async syntax is frontend notation.
FSM IR is the semantic core.
Simulator and Verilog backend both consume FSM IR.

On tick:
    state_reg and data_regs commit
    output combinational logic for the new state settles
    observations see the settled post-edge outputs

Might want to add
- blocking reads
- nonblocking reads

Async syntax is the frontend; FSM IR is the semantics.

