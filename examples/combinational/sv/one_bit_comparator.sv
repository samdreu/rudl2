module one_bit_comparator 
( 
    input logic i0, i1,
    output logic eq
);

logic p0, p1;

// sum of two product terms
assign eq = p0 | p1;

// Product terms
assign p0 = ~i0 & ~i1;
assign p1 = i0 & i1;

endmodule
