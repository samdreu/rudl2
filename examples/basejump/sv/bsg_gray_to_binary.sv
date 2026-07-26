// Independent hardware reference for Copper equivalence testing.
//
// Source:  BaseJump STL — bsg_misc/bsg_gray_to_binary.sv (top) and its dependency
//          bsg_misc/bsg_scan.sv, bundled here so the reference is self-contained.
//          https://github.com/bespoke-silicon-group/basejump_stl
// License: Solderpad Hardware License v0.51 (Apache-2.0-based).
//          Copyright 2016 Michael B. Taylor / BaseJump STL contributors.
//
// Adapted for self-contained Verilator use: removed `include "bsg_defines.sv"`, the
// `BSG_INV_PARAM` / `BSG_ABSTRACT_MODULE` macros, the sim-only `initial` assertion
// and debug block, and (in bsg_scan) the AND-specific width 2/3/4 fast paths that
// this XOR instance does not use. The Kogge-Stone prefix-scan logic is unchanged.
// gray_to_binary is a prefix-XOR of the gray code, realized via bsg_scan(xor).

/* verilator lint_off GENUNNAMED */
module bsg_scan #(parameter width_p = 8
                  , parameter xor_p = 0
                  , parameter and_p = 0
                  , parameter or_p = 0
                  , parameter lo_to_hi_p = 0)
   (input    [width_p-1:0] i
    , output logic [width_p-1:0] o
    );

   genvar j;

   wire [$clog2(width_p):0][width_p-1:0] t;

   if (lo_to_hi_p)
     assign t[0] = {<< {i}};
   else
     assign t[0] = i;

   for (j = 0; j < $clog2(width_p); j = j + 1)
     begin : row
        wire [width_p-1:0] fill;
        wire [width_p-1:0] shifted = width_p ' ({fill, t[j]} >> (1 << j));

        if (xor_p)
          begin
             assign fill = { width_p {1'b0} };
             assign t[j+1] = t[j] ^ shifted;
          end
        else if (and_p)
          begin
             assign fill = { width_p {1'b1} };
             assign t[j+1] = t[j] & shifted;
          end
        else if (or_p)
          begin
             assign fill = { width_p {1'b0} };
             assign t[j+1] = t[j] | shifted;
          end
     end

   if (lo_to_hi_p)
     for (j = 0; j < width_p; j++)
       assign o[j] = t[$clog2(width_p)][width_p-1-j];
   else
     assign o = t[$clog2(width_p)];

endmodule

module bsg_gray_to_binary #(parameter width_p = 8)
   (input    [width_p-1:0] gray_i
    , output [width_p-1:0] binary_o
    );

   bsg_scan #(.width_p(width_p)
              ,.xor_p(1)
              ) scan_xor
        (.i(gray_i)
        ,.o(binary_o));

endmodule
