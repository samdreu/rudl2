// Independent hardware reference for the mid-phase-read investigation.
//
// Distilled from BaseJump STL — bsg_dataflow/bsg_serial_in_parallel_out.sv (and
// the bsg_serial_in_parallel_out_* family). Those modules sample `data_i` into
// registers on every cycle while assembling a parallel word; the valid/ready +
// multi-deque FIFO wrapper is removed here to isolate the sampling timing, leaving
// a plain BLOCK serial-in-parallel-out (deserializer): it groups every els_p=4
// input words and presents them in parallel.
//   https://github.com/bespoke-silicon-group/basejump_stl
//   Solderpad Hardware License v0.51 (Apache-2.0-based). Copyright 2016
//   Michael B. Taylor / BaseJump STL contributors.
//
// Word i of each block is the value of data_i on the i-th cycle of the block, so
// this is the reference for whether a multi-tick Copper deserializer samples each
// word on the right cycle (words 1..3 are read AFTER a tick — "mid-phase").
/* verilator lint_off GENUNNAMED */
module sipo_block (clk, data_i, data_o);
  input clk;
  input  [3:0]  data_i;
  output reg [15:0] data_o;

  reg [1:0] cnt;
  reg [3:0] w0, w1, w2;

  initial begin cnt = 0; data_o = 0; w0 = 0; w1 = 0; w2 = 0; end

  always @(posedge clk) begin
    case (cnt)
      2'd0: w0 <= data_i;
      2'd1: w1 <= data_i;
      2'd2: w2 <= data_i;
      2'd3: data_o <= {data_i, w2, w1, w0}; // {w3, w2, w1, w0}
    endcase
    cnt <= cnt + 2'd1;
  end
endmodule
