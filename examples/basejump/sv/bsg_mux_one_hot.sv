// Independent hardware reference for Copper equivalence testing.
//
// Source:  BaseJump STL — bsg_misc/bsg_mux_one_hot.sv
//          https://github.com/bespoke-silicon-group/basejump_stl
// License: Solderpad Hardware License v0.51 (Apache-2.0-based).
//          Copyright 2016 Michael B. Taylor / BaseJump STL contributors.
//
// Adapted for self-contained Verilator use: removed `include "bsg_defines.sv"`
// and the `BSG_INV_PARAM` / `BSG_ABSTRACT_MODULE` macros, dropped the unused
// `harden_p` and the `els_p == 0` degenerate branch. The mask-then-OR-reduce logic
// is unchanged. Parameters (width_p=4, els_p=3) match BaseJump's own testbench
// testing/bsg_misc/bsg_mux_one_hot/test_bsg.sv, whose stimulus this example ports.
module bsg_mux_one_hot #(parameter width_p = 4, parameter els_p = 3)
   (
    input  [els_p-1:0][width_p-1:0] data_i
    ,input [els_p-1:0]              sel_one_hot_i
    ,output [width_p-1:0]           data_o
    );

   wire [els_p-1:0][width_p-1:0]   data_masked;

   genvar                          i,j;

   for (i = 0; i < els_p; i++)
     begin : mask
        assign data_masked[i] = data_i[i] & { width_p { sel_one_hot_i[i] } };
     end

   for (i = 0; i < width_p; i++)
     begin: reduce
        wire [els_p-1:0] gather;

        for (j = 0; j < els_p; j++)
          begin : reduce2
            assign gather[j] = data_masked[j][i];
          end

        assign data_o[i] = | gather;
     end

endmodule
