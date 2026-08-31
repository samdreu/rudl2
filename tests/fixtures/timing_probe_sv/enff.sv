module enff (clk, sel, d, q);
  input clk;
  input [7:0] sel, d;
  output [7:0] q;
  reg [7:0] q;
  initial q = 0;
  always @(posedge clk) if (sel == 8'd1) q <= d;
endmodule
