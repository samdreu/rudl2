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
