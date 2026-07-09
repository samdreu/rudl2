// note XOR mask starts at bit 0; which may
// be shifted from mathematician's notation.

module lfsr (input clk
    , input reset_i
    , input yumi_i
    , output logic [32-1:0] o
    );

  logic [32-1:0] o_r, o_n, xor_mask;

  assign o = o_r;

  // auto mask value
  assign xor_mask = (1 << 31) | (1 << 29) | (1 << 26) | (1 << 25);

  always @(posedge clk)
    begin
      if (reset_i)
        o_r <= (32) ' (1);
      else if (yumi_i)
        o_r <= o_n;
    end

  assign o_n = (o_r >> 1) ^ ({32 {o_r[0]}} & xor_mask);
   

endmodule // bsg_lfsr

