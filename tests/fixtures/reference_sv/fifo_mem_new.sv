// Synchronous-read FIFO backed by a 1-cycle-latency, ReadFirst RAM.
//
// Reference model for `tests/verilog_fifo_memory_new.rs`, matching the pipelined
// `Memory<u8, 1, 1, D>` semantics (READ_LAT = WRITE_LAT = 1, ReadFirst):
//   * write commits at the posedge,
//   * `dout` is a *registered* read of `mem[read_ptr]` captured at the posedge
//     using the pre-write memory contents (ReadFirst via non-blocking assign).
module fifo_mem_new (
    input  wire       clk,
    input  wire       wr_en,
    input  wire       rd_en,
    input  wire [7:0] din,
    output wire [7:0] dout,
    output wire       empty,
    output wire       full,
    output wire       valid,
    output wire [2:0] count
);
    reg [7:0] mem [0:3];
    reg [1:0] read_ptr;
    reg [1:0] write_ptr;
    reg [2:0] count_reg;
    reg [7:0] dout_reg;

    integer i;
    initial begin
        read_ptr  = 2'd0;
        write_ptr = 2'd0;
        count_reg = 3'd0;
        dout_reg  = 8'd0;
        for (i = 0; i < 4; i = i + 1) begin
            mem[i] = 8'd0;
        end
    end

    // Control flags from the pre-edge count.
    wire can_write = wr_en && (count_reg != 3'd4);
    wire can_read  = rd_en && (count_reg != 3'd0);

    always @(posedge clk) begin
        if (can_write) begin
            mem[write_ptr] <= din;
            write_ptr <= write_ptr + 2'd1;
        end

        // Registered synchronous read. Non-blocking RHS uses the pre-write
        // memory and the pre-edge read_ptr → ReadFirst 1-cycle-latency read.
        dout_reg <= mem[read_ptr];

        if (can_read) begin
            read_ptr <= read_ptr + 2'd1;
        end

        case ({can_write, can_read})
            2'b10:   count_reg <= count_reg + 3'd1;
            2'b01:   count_reg <= count_reg - 3'd1;
            default: count_reg <= count_reg;
        endcase
    end

    assign dout  = dout_reg;
    assign count = count_reg;
    assign empty = (count_reg == 3'd0);
    assign full  = (count_reg == 3'd4);
    assign valid = !empty;
endmodule
