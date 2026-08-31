// Independent hardware reference for Copper equivalence testing.
//
// Source:  BaseJump STL — bsg_misc/bsg_encode_one_hot.sv
//          https://github.com/bespoke-silicon-group/basejump_stl
// License: Solderpad Hardware License v0.51 (Apache-2.0-based).
//          Copyright 2016 Michael B. Taylor / BaseJump STL contributors.
//
// Adapted for self-contained Verilator use: removed `include "bsg_defines.sv"` and
// the `BSG_SAFE_CLOG2` / `BSG_UNDEFINED_IN_SIM` / `BSG_ABSTRACT_MODULE` macros and
// the debug/SYNTHESIS blocks, pinned width_p=8 (so addr_o is 3 bits), replaced
// `BSG_SAFE_CLOG2(width_p)` with `$clog2(width_p)` (equal for width_p>=2) and the
// don't-care base address with '0. The parallel-prefix encode tree is unchanged.
// Encodes a one-hot input into its binary index (addr_o) with a valid bit (v_o).
/* verilator lint_off GENUNNAMED */
module bsg_encode_one_hot #(parameter width_p = 8, parameter lo_to_hi_p = 1)
(input [width_p-1:0] i
 ,output [$clog2(width_p)-1:0] addr_o
 ,output v_o // whether any bits are set
);

  localparam levels_lp = $clog2(width_p);
  localparam aligned_width_lp = 1 << $clog2(width_p);

  genvar level;
  genvar segment;

  wire [levels_lp:0][aligned_width_lp-1:0] addr;
  wire [levels_lp:0][aligned_width_lp-1:0] v;

  // base case, also handle padding for non-power of two inputs
  assign v   [0] = lo_to_hi_p ? ((aligned_width_lp) ' (i)) :  i << (aligned_width_lp - width_p);
  assign addr[0] = '0;

  for (level = 1; level < levels_lp+1; level=level+1)
    begin : rof
      localparam segments_lp = 2**(levels_lp-level);
      localparam segment_slot_lp = aligned_width_lp/segments_lp;
      localparam segment_width_lp = level; // how many bits are needed at each level

      for (segment = 0; segment < segments_lp; segment=segment+1)
        begin : rof1
          wire [1:0] vs = {
                           v[level-1][segment*segment_slot_lp+(segment_slot_lp >> 1)]
                           , v[level-1][segment*segment_slot_lp]
                          };

          assign v[level][segment*segment_slot_lp] = | vs;

          if (level == 1)
            assign addr[level][(segment*segment_slot_lp)+:segment_width_lp] = { vs[lo_to_hi_p] };
          else
            begin : fi
              assign addr[level][(segment*segment_slot_lp)+:segment_width_lp]
              = { vs[lo_to_hi_p]
                 , addr[level-1][segment*segment_slot_lp+:segment_width_lp-1]
                 | addr[level-1][segment*segment_slot_lp+(segment_slot_lp >> 1)+:segment_width_lp-1]
                };
            end
        end
    end

  assign v_o = v[levels_lp][0];
  assign addr_o = addr[levels_lp][$clog2(width_p)-1:0];

endmodule
