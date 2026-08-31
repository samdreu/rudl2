// Independent hardware reference for Copper equivalence testing.
//
// Source:  BaseJump STL — bsg_misc/bsg_dff_en.sv
//          https://github.com/bespoke-silicon-group/basejump_stl
// License: Solderpad Hardware License v0.51 (Apache-2.0-based).
//          Copyright 2016 Michael B. Taylor / BaseJump STL contributors.
//
// Adapted for self-contained Verilator use: removed `include "bsg_defines.sv"`
// and the `BSG_INV_PARAM` / `BSG_ABSTRACT_MODULE` macros, gave width_p a concrete
// default (8) for Verilator, and renamed clk_i -> clk to match the equivalence
// harness (which drives a port literally named `clk`). The enabled-register logic
// is unchanged from the original.
module bsg_dff_en #(parameter width_p = 8)
(
  input clk
  ,input [width_p-1:0] data_i
  ,input en_i
  ,output logic [width_p-1:0] data_o
);

  logic [width_p-1:0] data_r;

  assign data_o = data_r;

  always_ff @ (posedge clk) begin
    if (en_i) begin
      data_r <= data_i;
    end
  end

endmodule
