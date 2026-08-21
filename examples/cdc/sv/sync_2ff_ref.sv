// Independent hand-written reference for the Copper standard-library
// synchronizer `copper::sync_2ff` (`src/sync.rs`).
//
// NOT produced by the Copper transpiler — authored directly from the textbook
// two-flip-flop synchronizer idiom, to anchor Copper's *synchronizer latency*
// (pattern 5 of design_docs/TIMING_COVERAGE_MATRIX.md) against an outside
// implementation. The existing `two_domain_hierarchy.sv` anchors a whole
// dual-clock hierarchy; this one isolates the primitive so the latency claim is
// measured directly rather than inferred through a counter and a consumer.
//
// Contract: `d` is an asynchronous signal from some source domain; `q` is `d`
// resynchronized into the `rd_clk` domain through two cascaded flip-flops, so
// metastability on stage 1 has a full destination period to settle before
// stage 2 samples it.

module sync_2ff_ref (
    input  logic rd_clk,
    input  logic d,
    output logic q
);
    // No declaration initializers: 2-state regs zero-initialize (matching
    // Copper's register reset-to-zero), and a declaration init alongside a
    // procedural assignment trips Verilator's PROCASSINIT under -Wall.
    logic ff1;
    logic ff2;

    // Moore output: the stage-2 register, read combinationally.
    assign q = ff2;

    always_ff @(posedge rd_clk) begin
        ff2 <= ff1;   // stage 2 takes the OLD stage-1 value ...
        ff1 <= d;     // ... so the two stages stay distinct (non-blocking)
    end
endmodule
