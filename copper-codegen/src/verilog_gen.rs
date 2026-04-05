/// Verilog Code Generator
/// 
/// Transforms Copper IR into synthesizable Verilog code.

use copper_core::ir::*;
use log::{debug, info};

/// Generates Verilog code from Copper IR
pub struct VerilogGenerator;

impl VerilogGenerator {
    /// Generate Verilog module from IR
    pub fn generate(ir: &ModuleIR) -> String {
        info!("Generating Verilog for module: {}", ir.name);
        debug!("Port count: {}", ir.ports.len());

        let mut verilog = String::new();

        // Module header
        verilog.push_str(&Self::generate_module_header(ir));

        // Port declarations
        verilog.push_str(&Self::generate_port_declarations(ir));

        // Module body
        verilog.push_str(&Self::generate_module_body(ir));

        // Module footer
        verilog.push_str("endmodule\n");

        verilog
    }

    /// Generate module header: "module name ("
    fn generate_module_header(ir: &ModuleIR) -> String {
        format!("module {} (\n", ir.name)
    }

    /// Generate port declarations
    fn generate_port_declarations(ir: &ModuleIR) -> String {
        let mut ports = String::new();

        for (i, port) in ir.ports.iter().enumerate() {
            debug!("Port: {} width={} dir={:?}", port.name, port.width, port.direction);

            let direction = match port.direction {
                Direction::Input => "input",
                Direction::Output => "output",
            };

            let port_type = match port.ty {
                PortType::Wire => "wire",
                PortType::Reg => "reg",
            };

            let width_str = if port.width > 1 {
                format!("[{}:0] ", port.width - 1)
            } else {
                String::new()
            };

            ports.push_str(&format!(
                "    {} {} {}{}",
                direction, port_type, width_str, port.name
            ));

            if i < ir.ports.len() - 1 {
                ports.push_str(",\n");
            } else {
                ports.push_str("\n");
            }
        }

        ports.push_str(");\n\n");
        ports
    }

    /// Generate module body (logic, registers, always blocks)
    fn generate_module_body(ir: &ModuleIR) -> String {
        let mut body = String::new();

        for wire in &ir.internal_wires {
            if wire.width > 1 {
                body.push_str(&format!("    wire [{}:0] {};\n", wire.width - 1, wire.name));
            } else {
                body.push_str(&format!("    wire {};\n", wire.name));
            }
        }

        for inst in &ir.submodules {
            body.push_str(&format!("    {} {} (\n", inst.module_name, inst.instance_name));
            for (idx, conn) in inst.connections.iter().enumerate() {
                let comma = if idx + 1 < inst.connections.len() { "," } else { "" };
                body.push_str(&format!("        .{}({}){}\n", conn.port, conn.signal, comma));
            }
            body.push_str("    );\n");
        }

        if !ir.internal_wires.is_empty() || !ir.submodules.is_empty() {
            body.push('\n');
        }

        match &ir.logic {
            ModuleLogic::Combinational { expressions } => {
                debug!("Generating combinational body with {} assign(s)", expressions.len());
                for assign in expressions {
                    body.push_str(&Self::generate_continuous_assign(assign));
                }
            }

            ModuleLogic::Sequential {
                clock,
                reset: _,
                registers,
                always_block,
                output_assigns,
            } => {
                debug!(
                    "Generating sequential body: clock={}, registers={}, statements={}",
                    clock,
                    registers.len(),
                    always_block.statements.len()
                );

                // Register declarations
                for reg in registers {
                    body.push_str(&Self::generate_register_declaration(reg));
                }

                // Initial block for register initialization
                if registers.iter().any(|r| r.initial.is_some()) {
                    body.push_str("    initial begin\n");
                    for reg in registers {
                        if let Some(init) = &reg.initial {
                            let init_expr = Self::expression_to_verilog(init);
                            body.push_str(&format!("        {} = {};\n", reg.name, init_expr));
                        }
                    }
                    body.push_str("    end\n\n");
                }

                // Separate blocking (combinational temporaries) from non-blocking
                // (register updates).  Mixing them in one always_ff block triggers
                // Verilator's BLKSEQ warning-as-error under -Wall.
                let comb_stmts: Vec<_> = always_block.statements.iter()
                    .filter(|s| matches!(s, Statement::BlockingAssign { .. }))
                    .collect();
                let seq_stmts: Vec<_> = always_block.statements.iter()
                    .filter(|s| !matches!(s, Statement::BlockingAssign { .. }))
                    .collect();

                // Combinational always block for blocking-assign temporaries
                if !comb_stmts.is_empty() {
                    body.push_str("    always @(*) begin\n");
                    for stmt in &comb_stmts {
                        body.push_str(&Self::generate_statement(stmt, 2));
                    }
                    body.push_str("    end\n\n");
                }

                // Sequential always block for non-blocking register updates
                if !seq_stmts.is_empty() {
                    body.push_str(&format!("    always @(posedge {}) begin\n", clock));
                    for stmt in &seq_stmts {
                        body.push_str(&Self::generate_statement(stmt, 2));
                    }
                    body.push_str("    end\n");
                }

                // Continuous output assigns (from emit! calls).
                // These come after the always block so that `out` reflects the
                // current (post-edge) register value, matching Rust tick_clock()
                // semantics where the task runs to completion before returning.
                if !output_assigns.is_empty() {
                    body.push('\n');
                    for assign in output_assigns {
                        body.push_str(&Self::generate_continuous_assign(assign));
                    }
                }
            }

            ModuleLogic::Mixed {
                clock,
                registers,
                combinational,
                sequential,
            } => {
                debug!(
                    "Generating mixed body: clock={}, registers={}, combinational={}, sequential={}",
                    clock,
                    registers.len(),
                    combinational.len(),
                    sequential.statements.len()
                );

                // Register declarations
                for reg in registers {
                    body.push_str(&Self::generate_register_declaration(reg));
                }

                // Combinational logic
                for assign in combinational {
                    body.push_str(&Self::generate_continuous_assign(assign));
                }

                // Always block
                body.push_str(&format!("    always @(posedge {}) begin\n", clock));
                for stmt in &sequential.statements {
                    body.push_str(&Self::generate_statement(stmt, 2));
                }
                body.push_str("    end\n");
            }
        }

        body
    }

    /// Generate a continuous assignment
    fn generate_continuous_assign(assign: &ContinuousAssign) -> String {
        let value = Self::expression_to_verilog(&assign.value);
        format!("    assign {} = {};\n", assign.target, value)
    }

    /// Generate a register declaration
    fn generate_register_declaration(reg: &RegisterDecl) -> String {
        if reg.width > 1 {
            format!("    reg [{}:0] {};\n", reg.width - 1, reg.name)
        } else {
            format!("    reg {};\n", reg.name)
        }
    }

    /// Generate a statement (indented)
    fn generate_statement(stmt: &Statement, indent_level: usize) -> String {
        let indent = " ".repeat(indent_level * 4);
        match stmt {
            Statement::NonBlockingAssign { target, value } => {
                let val_str = Self::expression_to_verilog(value);
                format!("{}{} <= {};\n", indent, target, val_str)
            }

            Statement::BlockingAssign { target, value } => {
                let val_str = Self::expression_to_verilog(value);
                format!("{}{} = {};\n", indent, target, val_str)
            }

            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let cond_str = Self::expression_to_verilog(condition);
                let mut result = format!("{}if ({}) begin\n", indent, cond_str);

                for stmt in then_branch {
                    result.push_str(&Self::generate_statement(stmt, indent_level + 1));
                }

                if let Some(else_stmts) = else_branch {
                    result.push_str(&format!("{}end else begin\n", indent));
                    for stmt in else_stmts {
                        result.push_str(&Self::generate_statement(stmt, indent_level + 1));
                    }
                }

                result.push_str(&format!("{}end\n", indent));
                result
            }

            Statement::Case {
                selector,
                arms,
                default,
            } => {
                let sel_str = Self::expression_to_verilog(selector);
                let mut result = format!("{}case ({})\n", indent, sel_str);

                for arm in arms {
                    let pattern_str = Self::pattern_to_verilog(&arm.pattern);
                    result.push_str(&format!("{}{}: begin\n", " ".repeat((indent_level + 1) * 4), pattern_str));

                    for stmt in &arm.body {
                        result.push_str(&Self::generate_statement(stmt, indent_level + 2));
                    }

                    result.push_str(&format!("{}end\n", " ".repeat((indent_level + 1) * 4)));
                }

                if let Some(default_stmts) = default {
                    result.push_str(&format!("{}default: begin\n", " ".repeat((indent_level + 1) * 4)));
                    for stmt in default_stmts {
                        result.push_str(&Self::generate_statement(stmt, indent_level + 2));
                    }
                    result.push_str(&format!("{}end\n", " ".repeat((indent_level + 1) * 4)));
                }

                result.push_str(&format!("{}endcase\n", indent));
                result
            }
        }
    }

    /// Convert expression to Verilog
    fn expression_to_verilog(expr: &Expression) -> String {
        match expr {
            Expression::Var(name) => name.clone(),

            Expression::Literal { width, value } => {
                format!("{}'d{}", width, value)
            }

            Expression::StringLiteral(s) => s.clone(),

            Expression::BinaryOp { left, op, right } => {
                let left_str = Self::expression_to_verilog(left);
                let right_str = Self::expression_to_verilog(right);
                let op_str = Self::binary_op_to_verilog(op);
                format!("({} {} {})", left_str, op_str, right_str)
            }

            Expression::UnaryOp { op, operand } => {
                let operand_str = Self::expression_to_verilog(operand);
                let op_str = Self::unary_op_to_verilog(op);
                format!("({}{})", op_str, operand_str)
            }

            Expression::BitSelect { signal, index } => {
                let sig_str = Self::expression_to_verilog(signal);
                let idx_str = Self::expression_to_verilog(index);
                format!("{}[{}]", sig_str, idx_str)
            }

            Expression::RangeSelect { signal, high, low } => {
                let sig_str = Self::expression_to_verilog(signal);
                let high_str = Self::expression_to_verilog(high);
                let low_str = Self::expression_to_verilog(low);
                format!("{}[{}:{}]", sig_str, high_str, low_str)
            }

            Expression::Concat(exprs) => {
                let parts: Vec<String> = exprs
                    .iter()
                    .map(Self::expression_to_verilog)
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }

            Expression::Ternary {
                condition,
                then_val,
                else_val,
            } => {
                let cond_str = Self::expression_to_verilog(condition);
                let then_str = Self::expression_to_verilog(then_val);
                let else_str = Self::expression_to_verilog(else_val);
                format!("({} ? {} : {})", cond_str, then_str, else_str)
            }
        }
    }

    /// Convert binary operator to Verilog
    fn binary_op_to_verilog(op: &BinaryOp) -> &'static str {
        match op {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Mod => "%",
            BinaryOp::And => "&",
            BinaryOp::Or => "|",
            BinaryOp::Xor => "^",
            BinaryOp::LeftShift => "<<",
            BinaryOp::RightShift => ">>",
            BinaryOp::LogicalAnd => "&&",
            BinaryOp::LogicalOr => "||",
            BinaryOp::Eq => "==",
            BinaryOp::Neq => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Lte => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::Gte => ">=",
        }
    }

    /// Convert unary operator to Verilog
    fn unary_op_to_verilog(op: &UnaryOp) -> &'static str {
        match op {
            UnaryOp::BitwiseNot => "~",
            UnaryOp::LogicalNot => "!",
            UnaryOp::ReductionAnd => "&",
            UnaryOp::ReductionOr => "|",
            UnaryOp::ReductionXor => "^",
        }
    }

    /// Convert pattern to Verilog
    fn pattern_to_verilog(pattern: &Pattern) -> String {
        match pattern {
            Pattern::Literal(lit) => lit.clone(),
            Pattern::Range(high, low) => format!("[{}:{}]", high, low),
            Pattern::Default => "default".to_string(),
        }
    }
}
