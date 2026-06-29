// Adapted from Greg Stitt, University of Florida.
// NUM_INPUTS default set to 8 for Verilator testing.
module priority_encode #(parameter int NUM_INPUTS = 8) (
    input  logic [        NUM_INPUTS-1:0] inputs,
    output logic [$clog2(NUM_INPUTS)-1:0] result,
    output logic                          valid
);
    localparam int NUM_OUTPUTS = $clog2(NUM_INPUTS);

    always_comb begin
        result = '0;
        valid  = 1'b0;

        for (int i = NUM_INPUTS - 1; i >= 0; i--) begin
            if (inputs[i] == 1'b1) begin
                result = NUM_OUTPUTS'(i);
                valid  = 1'b1;
                break;
            end
        end
    end
endmodule
