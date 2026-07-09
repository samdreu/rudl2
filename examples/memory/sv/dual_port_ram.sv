// Simple Dual-Port Block RAM with One Clock
// File: dual_port_ram.v

module dual_port_ram (clk,ena,enb,wea,addra,addrb,dia,dob);

input clk,ena,enb,wea;
input [9:0] addra,addrb;
input [15:0] dia;
output [15:0] dob;
reg [15:0] ram [1023:0];
// reg [15:0] doa,dob;
reg [15:0] dob;

always @(posedge clk) begin
if (ena) begin
if (wea)
ram[addra] <= dia;
end
end

always @(posedge clk) begin
if (enb)
dob <= ram[addrb];
end

endmodule
