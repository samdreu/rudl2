// Independent hardware reference for `tests/fixtures/write_first_ram_dut.rs::ram_read_first`.
//
// PROVENANCE: hand-written for this repository. It is therefore evidence that the
// emitted SystemVerilog does what a hardware engineer would write by hand for a
// read-first 1r1w synchronous RAM — a different and weaker claim than the
// third-party anchoring in `examples/basejump/sv/`, which is independent of anyone
// here. A misconception shared between this file and the Copper module would
// survive. Read `bsg_mem_1r1w_sync` (BaseJump STL) if you want the stronger form.
//
// READ-FIRST is the whole point of the module under test: the read captures the
// value held BEFORE the edge, so a write to the same address on the same edge is
// not visible to it. In SystemVerilog that falls out of non-blocking assignment —
// both statements below read pre-edge state regardless of the order they appear
// in — which is precisely why a reference written this way is worth having: it
// arrives at the behaviour from the language's own semantics rather than from the
// transpiler's enable/address nets.
//
// `mem` is deliberately left uninitialised. Verilator zero-fills by default
// (`--x-initial 0`) and the transpiler emits no `initial` block either, so the two
// start from the same state; a preloaded memory is a separate fixture.

module ram_read_first (
    input  logic       clk,
    input  logic [3:0] waddr,
    input  logic [7:0] wdata,
    input  logic       we,
    input  logic [3:0] raddr,
    output logic [7:0] data
);

    logic [7:0] mem [0:15];

    always_ff @(posedge clk) begin
        data <= mem[raddr];
        if (we) begin
            mem[waddr] <= wdata;
        end
    end

endmodule
