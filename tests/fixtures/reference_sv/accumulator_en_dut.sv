module accumulator_en (
    input logic clk,
    input logic en,
    input logic [7:0] data,
    output logic [7:0] acc
);

    always_ff @(posedge clk) begin
        if (en) begin
            acc <= acc + data;
        end
    end

endmodule
