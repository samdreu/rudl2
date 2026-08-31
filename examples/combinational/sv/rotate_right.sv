// Adapted from BaseJump STL — bsg_misc/bsg_rotate_right.sv
//   https://github.com/bespoke-silicon-group/basejump_stl
//   Solderpad Hardware License v0.51 (Apache-2.0-based). Copyright 2016
//   Michael B. Taylor / BaseJump STL contributors.
// Adapted for self-contained Verilator use (bsg_defines/macros stripped, concrete
// parameter default); the rotate logic is unchanged.
//
// width_p default set to 8 for Verilator testing.
module bsg_rotate_right #(parameter width_p = 8)
   (input [width_p-1:0] data_i
   ,input [$clog2(width_p > 1 ? width_p : 2)-1:0] rot_i
   ,output [width_p-1:0] o
   );

   /* verilator lint_off UNUSEDSIGNAL */
   wire [width_p*2-1:0] temp = { 2 { data_i } } >> rot_i;
   /* verilator lint_on UNUSEDSIGNAL */
   assign o = temp[0+:width_p];

endmodule
