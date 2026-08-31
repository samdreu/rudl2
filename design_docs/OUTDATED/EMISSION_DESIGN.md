# Copper Verilog Emission Design — Phase E

**Implementation status: designed, not yet implemented.** This document describes the Phase E text emitter that will operate on `VLIRModule` (Phase D output). It cannot be implemented until Phase D (VLIR legalization) is complete.

## Purpose

This document defines Phase E of the Copper transpilation pipeline: Verilog text emission. Phase E receives a `VLIRModule` (Phase D output) and produces a `String` containing valid, formatted SystemVerilog source text.

Phase E is purely mechanical. It makes no semantic decisions — all hardware intent, timing, naming, and structure were fixed in earlier phases. Phase E is a serializer.

---

## What Phase E Is and Is Not

**Phase E is:**
- A deterministic, mechanical text serializer
- Formatting-aware (indentation, spacing, ordering)
- Source-location-annotatable (optional comment mode)
- Toolchain-profile-aware for syntax choices (e.g. `always_ff` vs `always @(posedge clk)`)

**Phase E is not:**
- Making any semantic decisions — those were finalized in Phase C and D
- Re-interpreting assignment intent — non-blocking is non-blocking, full stop
- Optimizing structure — what Phase D gives is what gets emitted
- Validating correctness — that is Phase F

---

## Emission API

```rust
pub fn emit_verilog(module: &VLIRModule, config: &EmitConfig) -> String
```

```rust
pub struct EmitConfig {
    pub profile: ToolchainProfile,
    pub include_source_locations: bool,  // emit // @<line:col> comments
    pub naming_style: NamingStyle,
    pub indent_width: usize,             // default: 4
}

pub enum NamingStyle {
    Compact,    // short generated names (e.g. w0, w1)
    Readable,   // preserves source names where possible (already done in Phase D)
}
```

---

## Output Structure

Every emitted module has this top-level structure:

```verilog
// Optional: file header comment

module <name> (
    <port declarations>
);

    // Register/wire declarations

    // Submodule instantiations

    // always_comb block(s)

    // always_ff block

    // Continuous assigns (output drive)

endmodule
```

### Ordering rules (for determinism)

Within each section, items are emitted in a fixed order:

1. **Ports:** inputs first (clock first among inputs), then outputs. Within each group, in the order they appear in `VLIRModule::ports`.
2. **Register declarations:** in the order they appear in `VLIRSeqBody::reg_decls`. The auto-generated `phase_r` (if present) is always last.
3. **Submodule instantiations:** in the order they appear in `VLIRSeqBody::submodules` or `VLIRCombBody::submodules`.
4. **always_comb blocks:** one block per `VLIRCombPhase`, in phase index order.
5. **always_ff block:** single block, statements in the order from `VLIRAlwaysFF::stmts`.
6. **Continuous assigns:** output port assign last.

---

## Port Declaration Format

```verilog
module counter (
    input  logic        clk,
    input  logic [7:0]  in_step,
    output logic [7:0]  out
);
```

Rules:
- Use `logic` for all ports (SystemVerilog). For the Yosys profile, use `wire`/`reg` instead.
- Clock ports: `input logic clk`
- Data input ports: `input logic [W-1:0] name` (width bracket omitted if W=1)
- Output ports: `output logic [W-1:0] name`
- Port list uses trailing comma style (last port has no comma)
- Align the signal names: pad direction/type fields to consistent column for readability

---

## Register/Wire Declaration Format

Inside the module body, before always blocks:

```verilog
    logic [7:0] count;
    logic [1:0] phase_r;  // only present for multi-phase modules
```

Rules:
- All internal signals use `logic`
- Width bracket omitted if width is 1
- One declaration per line
- No initial values in production mode (per `VERILOG_OUTPUT_STANDARDS.md` §5)

---

## `always_comb` Block Format

```verilog
    always_comb begin
        stage1_data = in_data + 8'd1;
        stage2_data = stage1_r + stage1_r;
    end
```

For multi-phase modules, each phase gets its own guarded block:

```verilog
    always_comb begin
        if (phase_r == 1'd0) begin
            // phase 0 combinational logic
        end
        if (phase_r == 1'd1) begin
            stage1_data = in_data + 8'd1;
        end
    end
```

Rules:
- Use `always_comb` (SystemVerilog). For Verilator and Yosys profiles, `always_comb` is also supported — keep it.
- All assignments inside are blocking (`=`)
- No `<=` inside `always_comb` — the separate statement types in VLIR enforce this
- `begin`/`end` always present, even for single-statement bodies

---

## `always_ff` Block Format

```verilog
    always_ff @(posedge clk) begin
        count <= count + in_step;
    end
```

For multi-phase modules:

```verilog
    always_ff @(posedge clk) begin
        case (phase_r)
            1'd0: begin
                phase_r <= 1'd1;
            end
            1'd1: begin
                count   <= count + stage1_r;
                phase_r <= 1'd0;
            end
        endcase
    end
```

Rules:
- Use `always_ff @(posedge clk)` (SystemVerilog). For a plain Verilog fallback profile, use `always @(posedge clk)`.
- All assignments inside are non-blocking (`<=`)
- No `=` inside `always_ff` — enforced by `VLIRFFStmt` type
- `begin`/`end` always present
- No reset logic emitted unless a reset port is explicitly declared (reset is out of scope for Milestone 1)

---

## Submodule Instantiation Format

`#[hardware]` submodule instances are emitted as standard Verilog module instantiations with named port connections:

```verilog
    logic [7:0] full_adder_0_out;
    full_adder full_adder_0 (
        .a   (a),
        .b   (b),
        .out (full_adder_0_out)
    );
```

Rules:
- The output wire declaration (`logic [W:0] <output_wire>;`) is emitted immediately before the instantiation
- Named port connections (`.port(expr)`) are always used — never positional
- Input ports listed first, output port last (for readability)
- The instance name is the legalized `VLIRSubmoduleInst::inst_name`
- The module name is the legalized `VLIRSubmoduleInst::module_name`
- All instantiations are emitted before any `always_comb` or `always_ff` blocks

For multiple instances of the same module:

```verilog
    logic [7:0] full_adder_0_out;
    full_adder full_adder_0 (
        .a   (a),
        .b   (b),
        .out (full_adder_0_out)
    );

    logic [7:0] full_adder_1_out;
    full_adder full_adder_1 (
        .a   (full_adder_0_out),
        .b   (full_adder_0_out),
        .out (full_adder_1_out)
    );
```

---

## Continuous Assign Format

```verilog
    assign out = count;
```

Rules:
- Always `assign <port> = <expr>;`
- Always outside any always block
- Emitted after all always blocks, before `endmodule`
- This is how all output ports are driven — never via non-blocking assign

---

## Literal Format

VLIR literals carry an explicit `width` and `value`. Phase E emits them as:

```
N'd<decimal_value>
```

Examples: `8'd0`, `8'd255`, `1'd1`, `2'd3`

Special cases:
- Width 1: prefer `1'b0` / `1'b1` for readability (still correct as `1'd0` / `1'd1`)
- Width ≤ 8 and value is a power of 2 minus 1: use decimal (e.g. `8'd255` not `8'hff`)
- For the Verilator profile: never emit `x` or `z` literals — use `0` instead

---

## Binary/Unary Operator Mapping

| VLIR op | Emitted SV |
|---|---|
| `Add` | `+` |
| `Sub` | `-` |
| `Mul` | `*` |
| `BitAnd` | `&` |
| `BitOr` | `\|` |
| `BitXor` | `^` |
| `Shl` | `<<` |
| `Shr` | `>>` |
| `Eq` | `==` |
| `Neq` | `!=` |
| `Lt` | `<` |
| `Lte` | `<=` |
| `Gt` | `>` |
| `Gte` | `>=` |
| `LogicalAnd` | `&&` |
| `LogicalOr` | `\|\|` |
| `BitNot` | `~` |
| `LogicalNot` | `!` |
| `Neg` | `-` (unary) |
| `ReductionAnd` | `&` (prefix) |
| `ReductionOr` | `\|` (prefix) |
| `ReductionXor` | `^` (prefix) |

Parenthesization: emit explicit parentheses for all compound expressions. Do not rely on operator precedence in the emitter — always emit `(a + b)` not `a + b` when part of a larger expression. This avoids precedence bugs and tool-specific quirks.

---

## Concat and Slice Format

```verilog
{j, k}         // VLIRExpr::Concat([Var("j"), Var("k")])
expr[7:4]      // VLIRExpr::Slice { expr, high: 7, low: 4 }
expr[0]        // VLIRExpr::Slice { expr, high: 0, low: 0 }
```

---

## Case Statement Format

```verilog
    case ({j, k})
        2'b00: begin
            q <= q;
        end
        2'b01: begin
            q <= 1'b0;
        end
        default: begin
            q <= 1'b0;
        end
    endcase
```

Rules:
- `case` (not `casez` or `casex`) unless a wildcard arm with don't-care bits exists — then use `casez`
- `default` arm always last
- `begin`/`end` always present for each arm body even if single statement
- `endcase` on its own line

---

## Source Location Comments (Optional Mode)

When `EmitConfig::include_source_locations` is true, Phase E emits `// @<line>:<col>` comments on generated lines, tracing back to the original Rust source span from `SourceSpan`.

```verilog
    always_ff @(posedge clk) begin
        count <= count + in_step;   // @12:8
    end
    assign out = count;             // @10:8
```

This is off by default. It is useful for debugging the transpiler itself — correlating emitted Verilog lines with Rust source locations.

---

## Indentation and Whitespace

- Default indent: 4 spaces
- Module body: 1 level of indentation
- always block body: 2 levels
- Nested if/case: +1 level per nesting depth
- No tabs — spaces only
- No trailing whitespace
- Single blank line between major sections (port list end, declarations, always_comb, always_ff, assigns)
- `endmodule` at column 0 with a trailing newline

---

## Determinism Requirements

For identical `VLIRModule` input and identical `EmitConfig`, the output string must be byte-for-byte identical across runs:

- No `HashMap` iteration order in output (use sorted order for any map-derived output)
- No platform-specific newline behavior — always `\n`
- No timestamp or host information in output

---

## Worked Example: Counter

**Input VLIR (simplified):**
```
module: "counter"
ports: [clk: Input Clock, in_step: Input Logic[8], out: Output Logic[8]]
reg_decls: [count: 8]
comb_phases: [phase 0: no stmts]
always_ff: [count <= count + in_step]
output_assign: out = count
```

**Phase E output:**
```verilog
module counter (
    input  logic       clk,
    input  logic [7:0] in_step,
    output logic [7:0] out
);

    logic [7:0] count;

    always_ff @(posedge clk) begin
        count <= (count + in_step);
    end

    assign out = count;

endmodule
```

---

## Worked Example: JK Flip-Flop (case statement)

**Phase E output:**
```verilog
module jk_ff (
    input  logic clk,
    input  logic j,
    input  logic k,
    output logic out
);

    logic q;

    always_ff @(posedge clk) begin
        case ({j, k})
            2'b00: begin
                q <= q;
            end
            2'b01: begin
                q <= 1'b0;
            end
            2'b10: begin
                q <= 1'b1;
            end
            2'b11: begin
                q <= ((q == 1'b0) ? 1'b1 : 1'b0);
            end
            default: begin
                q <= 1'b0;
            end
        endcase
    end

    assign out = q;

endmodule
```

---

## Phase E Contract

**Input:** `VLIRModule` (from Phase D legalization)

**Output:** `String` containing valid, formatted SystemVerilog

**Invariants:**
1. Output is syntactically valid SystemVerilog parseable by the target toolchain profile
2. Output is deterministic for identical input + config
3. No semantic decisions are made — the emitter is mechanical
4. Assignment style matches VLIR type: `VLIRStmt` → `=`, `VLIRFFStmt` → `<=`, continuous → `assign`
5. All port names, signal names, module names, and instance names match exactly what Phase D legalized
6. Each `VLIRSubmoduleInst` is emitted as a named-port module instantiation with its output wire declared immediately before it
7. No `initial` blocks appear in the output (per `VERILOG_OUTPUT_STANDARDS.md` §5)
8. Output passes Phase F lint checks for the target profile
