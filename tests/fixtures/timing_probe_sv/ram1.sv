// 1-cycle synchronous-read RAM, preloaded ram[i] = i + 100.
module ram1 (clk, enb, addrb, dob);
  input clk, enb;
  input [7:0] addrb;
  output reg [15:0] dob;
  reg [15:0] ram [0:255];
  integer i;
  initial begin
    dob = 16'd0;
    for (i = 0; i < 256; i = i + 1) ram[i] = 16'(i + 100);
  end
  always @(posedge clk) if (enb) dob <= ram[addrb];
endmodule
