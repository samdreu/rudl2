// Faithful hand-written translation of the Copper mac_fsm 3-state machine.
// Load=0, Mul=1, Out=2.  Each arm's assignments are registered; out <= result
// only in the Out state (held otherwise).
module mac_fsm (clk, a, b, c, out);
  input clk;
  input [7:0] a, b, c;
  output reg [7:0] out;
  reg [1:0] stage;
  reg [7:0] product, c_latch, result;
  initial begin
    stage = 0; out = 0; product = 0; c_latch = 0; result = 0;
  end
  always @(posedge clk) begin
    case (stage)
      2'd0: begin product <= a * b; c_latch <= c; stage <= 2'd1; end
      2'd1: begin result <= product + c_latch; stage <= 2'd2; end
      2'd2: begin out <= result; stage <= 2'd0; end
      default: stage <= 2'd0;
    endcase
  end
endmodule
