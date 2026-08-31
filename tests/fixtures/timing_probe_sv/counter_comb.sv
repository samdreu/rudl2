module counter_comb (clk, q);
  input clk;
  output [7:0] q;
  reg [7:0] v;
  initial v = 0;
  assign q = v;
  always @(posedge clk) v <= v + 1;
endmodule
