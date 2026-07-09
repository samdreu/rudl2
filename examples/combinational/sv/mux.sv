// Defaults (width_p=8, els_p=4) are set for Verilator testing.
// In a real design, override these at instantiation.
module bsg_mux #(
    parameter width_p    = 8,
    parameter els_p      = 4,
    /* verilator lint_off UNUSEDPARAM */
    parameter harden_p   = 0,
    parameter balanced_p = 0,
    /* verilator lint_on UNUSEDPARAM */
    parameter lg_els_lp  = $clog2(els_p > 1 ? els_p : 2)
)(
    input  [els_p-1:0][width_p-1:0] data_i,
    input  [lg_els_lp-1:0]          sel_i,
    output [width_p-1:0]            data_o
);

    if (els_p == 1) begin : gen_passthrough
        assign data_o = data_i;
    end else begin : gen_mux
        assign data_o = data_i[sel_i];
    end

endmodule
