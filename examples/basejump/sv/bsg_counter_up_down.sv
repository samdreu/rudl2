// Independent hardware reference for Copper equivalence testing.
//
// Source:  BaseJump STL — bsg_misc/bsg_counter_up_down.sv
//          https://github.com/bespoke-silicon-group/basejump_stl
// License: Solderpad Hardware License v0.51 (Apache-2.0-based).
//          Copyright 2016 Michael B. Taylor / BaseJump STL contributors.
//
// Adapted for self-contained Verilator use: removed `include "bsg_defines.sv"` and
// the `BSG_INV_PARAM` / `BSG_WIDTH` / `BSG_ABSTRACT_MODULE` macros, pinned the
// derived widths to BaseJump's own testbench instance (max_val_p=7 ->
// ptr_width_lp=BSG_WIDTH(7)=3, max_step_p=1 -> step_width_lp=BSG_WIDTH(1)=1; see
// testing/bsg_misc/bsg_counter_up_down/test_bsg.sv), renamed clk_i -> clk, and
// dropped the synthesis-excluded negedge `$display` overflow/underflow assertions
// (simulation diagnostics, not hardware behavior). The registered count logic is
// unchanged, including its wrap-around (no saturation).
module bsg_counter_up_down #(
                            /* verilator lint_off UNUSEDPARAM */
                              parameter max_val_p     = 7   // documents the instance; width is pinned below
                            , parameter max_step_p    = 1
                            /* verilator lint_on UNUSEDPARAM */
                            , parameter init_val_p    = 0
                            , parameter step_width_lp = 1
                            , parameter ptr_width_lp  = 3 )
   ( input                          clk
   , input                          reset_i
   , input        [step_width_lp-1:0] up_i
   , input        [step_width_lp-1:0] down_i
   , output logic [ptr_width_lp-1:0]  count_o
    );

  always_ff @(posedge clk)
    begin
      if (reset_i)
        count_o <= init_val_p;
      else
        // Original BaseJump expression; the 1-bit up_i/down_i widen to the count
        // width. Benign, so silence Verilator's width-expand lint.
        /* verilator lint_off WIDTHEXPAND */
        count_o <= count_o - down_i + up_i;
        /* verilator lint_on WIDTHEXPAND */
    end

endmodule
