// Hand-written reference for the request/acknowledge handshake counter
// (examples/sequential/handshake.rs; tests/fixtures/handshake_dut.rs). Written
// 2026-09-02 from the module's English description — wait for `req`, take one
// cycle, wait for `ack`, count — as the explicit three-state machine a Verilog
// designer writes, NOT from the transpiler's output (which has four `pc` values,
// one per `clk.tick().await`). Same author as the Copper module, so this is a
// second spelling, not a third-party opinion; it anchors the TIMING the two
// spellings share. Named for the Copper module, as the harness requires.
//
// Timing: `done` is published at TOP, so it shows the count of completed
// handshakes one cycle after `ack` was seen; `ack` is sampled from the cycle
// after `req` was seen. No reset, like the source: state starts at TOP with the
// count at zero.
module handshake (
    input  logic       clk,
    input  logic       req,
    input  logic       ack,
    output logic [7:0] done
);
    localparam logic [1:0] TOP = 2'd0, WAIT_REQ = 2'd1, WAIT_ACK = 2'd2;

    logic [1:0] state;
    logic [7:0] n;

    // The lint pass (-Wall, fatal here) refuses a declaration initializer on a
    // variable an always block also writes (PROCASSINIT), so the power-on state
    // is an `initial` block instead.
    initial begin
        state = TOP;
        n     = 8'd0;
        done  = 8'd0;
    end

    always_ff @(posedge clk) begin
        case (state)
            TOP: begin
                done  <= n;
                state <= req ? WAIT_ACK : WAIT_REQ;
            end
            WAIT_REQ:
                if (req) state <= WAIT_ACK;
            WAIT_ACK:
                if (ack) begin
                    n     <= n + 8'd1;
                    state <= TOP;
                end
            default: state <= TOP;
        endcase
    end
endmodule
