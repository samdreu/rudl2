// Independent hand-written reference for the item-4 dual-clock hierarchy
// (`examples/cdc/two_domain_hierarchy.rs`). NOT produced by the Copper
// transpiler — authored directly from the design intent to anchor Copper's
// dual-clock / CDC timing against an outside implementation (fills the G1
// pattern-5 gap). Standard synthesizable posedge-FF idiom.
//
// Design: a fast-domain up-counter latches a threshold flag (count >= 8), which
// crosses into the slow domain through a 2-flip-flop synchronizer; the slow side
// exposes the synchronized flag combinationally.

// Fast-domain producer: free-running counter + sticky threshold latch.
module ref_fast_counter (
    input  logic       wr_clk,
    output logic [7:0] count_o,
    output logic       flag_o
);
    // No declaration initializer: 2-state regs zero-initialize (matching Copper's
    // register reset-to-zero), and a declaration init alongside a procedural
    // assignment trips Verilator's PROCASSINIT under -Wall.
    logic [7:0] cnt;
    logic       latch;

    // Moore outputs: the current register values (read combinationally).
    assign count_o = cnt;
    assign flag_o  = latch;

    always_ff @(posedge wr_clk) begin
        if (cnt[3]) latch <= 1'b1;   // sticky: once set, stays set
        cnt <= cnt + 8'd1;
    end
endmodule

// 2-flip-flop synchronizer, clocked by the destination (slow) domain.
module ref_sync2 (
    input  logic rd_clk,
    input  logic d,
    output logic q
);
    logic ff1;
    logic ff2;

    assign q = ff2;

    always_ff @(posedge rd_clk) begin
        ff2 <= ff1;   // stage 2 takes the OLD stage-1 value
        ff1 <= d;     // stage 1 samples the (metastable) source signal
    end
endmodule

// Top: pure hierarchy — instantiate the two children on their own clocks and
// wire the flag crossing. Mirrors the Copper `two_domain_top` structure.
module two_domain_ref (
    input  logic       wr_clk,
    input  logic       rd_clk,
    output logic [7:0] count_out,
    output logic       flag_sync_out
);
    logic flag;

    ref_fast_counter u_fast (
        .wr_clk  (wr_clk),
        .count_o (count_out),
        .flag_o  (flag)
    );

    ref_sync2 u_sync (
        .rd_clk (rd_clk),
        .d      (flag),
        .q      (flag_sync_out)
    );
endmodule
