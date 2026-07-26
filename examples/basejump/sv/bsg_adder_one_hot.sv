// Independent hardware reference for Copper equivalence testing.
//
// Source:  BaseJump STL — bsg_misc/bsg_adder_one_hot.sv
//          https://github.com/bespoke-silicon-group/basejump_stl
// License: Solderpad Hardware License v0.51 (Apache-2.0-based).
//          Copyright 2016 Michael B. Taylor / BaseJump STL contributors.
//
// Adapted for self-contained Verilator use: removed `include "bsg_defines.sv"`,
// the `BSG_INV_PARAM` / `BSG_ABSTRACT_MODULE` macros, and the sim-only `initial`
// assertion; pinned width_p=4/output_width_p=7 to BaseJump's own testbench
// (testing/bsg_misc/bsg_adder_one_hot/test_bsg.sv). The one-hot add logic is
// unchanged. Adds two one-hot inputs, producing a one-hot output at the sum of
// their indices (non-modulo, since output_width_p = 2*width_p-1).
/* verilator lint_off GENUNNAMED */
module bsg_adder_one_hot #(parameter width_p = 4, parameter output_width_p = 7)
   (input    [width_p-1:0] a_i
    , input  [width_p-1:0] b_i
    , output [output_width_p-1:0] o
    );

   genvar i,j;

   for (i=0; i < output_width_p; i++) // for each output wire
     begin: rof
        wire [width_p-1:0] aggregate;

        for (j=0; j < width_p; j=j+1)
          begin: rof2
             if (i < j)
               begin: rof3
                  if (output_width_p+i-j < width_p)
                    assign aggregate[j] = a_i[j] & b_i[output_width_p+i-j];
                  else
                    assign aggregate[j] = 1'b0;
               end
             else
               if (i-j < width_p)
                 assign aggregate[j] = a_i[j] & b_i[i-j];
               else
                 assign aggregate[j] = 1'b0;
          end // block: rof2

        assign o[i] = | aggregate;

     end // block: rof

endmodule
