/* verilator lint_off DECLFILENAME */
module stage_add_one (
  input wire clk,
  input wire [7:0] in_data,
  output wire [7:0] out_data
);
  reg [7:0] reg_data;

  initial begin
    reg_data = 8'd0;
  end

  assign out_data = reg_data;

  always @(posedge clk) begin
    reg_data <= in_data + 8'd1;
  end
endmodule

module stage_double (
  input wire clk,
  input wire [7:0] in_data,
  output reg [7:0] out_data
);
  reg [7:0] reg_data;

  initial begin
    reg_data = 8'd0;
    out_data = 8'd0;
  end

  always @(posedge clk) begin
    out_data <= reg_data;
    reg_data <= in_data + in_data;
  end
endmodule

module hybrid_pipeline (
  input wire clk,
  input wire [7:0] in_data,
  output wire [7:0] out_data
);
  wire [7:0] stage1_out;

  stage_add_one u_stage_add_one (
    .clk(clk),
    .in_data(in_data),
    .out_data(stage1_out)
  );

  stage_double u_stage_double (
    .clk(clk),
    .in_data(stage1_out),
    .out_data(out_data)
  );
endmodule
/* verilator lint_on DECLFILENAME */
