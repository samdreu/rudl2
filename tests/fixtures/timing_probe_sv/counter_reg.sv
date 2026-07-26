module counter_reg (clk, q);
  input clk;
  output [7:0] q;
  reg [7:0] q;
  reg [7:0] v;
  initial begin q = 0; v = 0; end
  always @(posedge clk) begin
    q <= v;
    v <= v + 1;
  end
endmodule
