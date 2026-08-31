// Independent hand-written hardware reference for the "010" sequence detector.
//
// This is the golden that fills the G1 variable-iteration-loop gap in
// design_docs/SYNCHRONOUS_SEMANTICS_IMPL_PLAN.md: the "010" detector previously
// had NO independent hardware reference — the only check was two Copper codings
// (det_010 vs det_010_awaits) against each other, which cannot adjudicate which
// cycle-by-cycle behavior is correct.
//
// Written directly from the 4-state Moore transition table below (NOT derived
// from Copper's transpiler output), so it is an independent adjudicator when run
// under Verilator against the Copper simulator's trace. `out` is combinational
// from a registered state (Moore); the state advances on posedge clk with a
// synchronous active-low reset — the same shape as pattern_detector.sv (det_110101).
//
// Transition table (state, in) -> next; output high only in D:
//   A: 0 -> B,  1 -> A          (waiting for a leading 0)
//   B: 0 -> B,  1 -> C          (have seen 0…0; a 1 gives "01")
//   C: 0 -> D,  1 -> A          ("01" then 0 completes "010")
//   D: 0 -> B,  1 -> A          (overlap: the trailing 0 of "010" is a new leading 0)
module det_010 ( input clk,
                 input rstn,
                 input in,
                 output out );

  parameter A = 0,
            B = 1,
            C = 2,
            D = 3;

  reg [1:0] cur_state, next_state;

  assign out = (cur_state == D) ? 1'b1 : 1'b0;

  always @ (posedge clk) begin
    if (!rstn)
      cur_state <= A;
    else
      cur_state <= next_state;
  end

  always @ (cur_state or in) begin
    case (cur_state)
      A: next_state = in ? A : B;
      B: next_state = in ? C : B;
      C: next_state = in ? A : D;
      D: next_state = in ? A : B;
      default: next_state = A;
    endcase
  end
endmodule
