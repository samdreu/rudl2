// Adapted from BaseJump STL — bsg_misc/bsg_adder_ripple_carry.sv
//   https://github.com/bespoke-silicon-group/basejump_stl
//   Solderpad Hardware License v0.51 (Apache-2.0-based). Copyright 2016
//   Michael B. Taylor / BaseJump STL contributors.
// Adapted for self-contained Verilator use (bsg_defines/macros stripped, concrete
// parameter default); the adder logic is unchanged.
//
// width_p default set to 8 for Verilator testing.
module bsg_adder_ripple_carry #(parameter width_p = 8)
  (
    input  [width_p-1:0] a_i,
    input  [width_p-1:0] b_i,
    output logic [width_p-1:0] s_o,
    output logic c_o
  );

  assign {c_o, s_o} = a_i + b_i;

endmodule
