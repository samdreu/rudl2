module bit_not_bits (input logic clk, input logic [31:0] a, output logic [31:0] o);
    // The harness always drives a port named `clk`, so a purely combinational
    // reference still declares one — and an unused input is UNUSEDSIGNAL under
    // -Wall, which is fatal. An empty `always_ff` consumes it, which is what the
    // transpiler emits for the same reason.
    always_ff @(posedge clk) begin end
    assign o = ~a;
endmodule
