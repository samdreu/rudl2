/// Top-level hardware IR
#[derive(Debug, Clone)]
pub struct ModuleIR {
    pub name: String,
    pub ports: Vec<PortDecl>,
    pub internal_wires: Vec<WireDecl>,
    pub submodules: Vec<SubmoduleInstance>,
    pub logic: ModuleLogic,
    // internal state (registers, wires)???
}

#[derive(Debug, Clone)]
pub struct WireDecl {
    pub name: String,
    pub width: usize,
}

#[derive(Debug, Clone)]
pub struct PortConnection {
    pub port: String,
    pub signal: String,
}

#[derive(Debug, Clone)]
pub struct SubmoduleInstance {
    pub module_name: String,
    pub instance_name: String,
    pub connections: Vec<PortConnection>,
}

/// Port declaration (input/output)
#[derive(Debug, Clone)]
pub struct PortDecl {
    pub name: String,
    pub direction: Direction,
    pub width: usize,
    pub ty: PortType,
}

/// Direction of a port
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
}

/// Type of a port (wire or register)
/// maybe add logic type catch all
/// or just have wire or reg for clarity?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortType {
    Wire,  // combinational
    Reg,   // sequential
}

/// Main module logic: combinational, sequential, or mixed
#[derive(Debug, Clone)]
pub enum ModuleLogic {
    /// Pure combinational logic (no state, no clock)
    Combinational {
        expressions: Vec<ContinuousAssign>,
    },

    /// Sequential logic (async fn with loop/await)
    Sequential {
        clock: String,
        reset: Option<String>,
        registers: Vec<RegisterDecl>,
        always_block: AlwaysBlock,
        /// Continuous output assignments derived from emit!() calls.
        ///
        /// emit!(reg) in Rust means "output always equals reg right now", which
        /// maps to `assign out = reg_sig;` in Verilog — a continuous wire, not a
        /// clocked register.  These are rendered outside the always block so that
        /// the output reflects the current (post-edge) register value immediately,
        /// matching the Rust executor semantics where tick_clock() polls the task
        /// to completion before returning.
        output_assigns: Vec<ContinuousAssign>,
    },

    /// Mixed combinational + sequential
    /// not sure how i feel about mixed combinational and sequential
    Mixed {
        clock: String,
        registers: Vec<RegisterDecl>,
        combinational: Vec<ContinuousAssign>,
        sequential: AlwaysBlock,
    },
}

/// Register declaration inside a module
#[derive(Debug, Clone)]
pub struct RegisterDecl {
    pub name: String,
    pub width: usize,
    pub initial: Option<Expression>,
}

/// Continuous assignment (assign target = value;)
#[derive(Debug, Clone)]
pub struct ContinuousAssign {
    pub target: String,
    pub value: Expression,
}

/// Always block (always @(posedge clk) or always @(*))
#[derive(Debug, Clone)]
pub struct AlwaysBlock {
    pub statements: Vec<Statement>,
}

/// Statements inside always blocks
#[derive(Debug, Clone)]
pub enum Statement {
    /// Non-blocking assignment (<=)
    NonBlockingAssign {
        target: String,
        value: Expression,
    },

    /// Blocking assignment (=)
    BlockingAssign {
        target: String,
        value: Expression,
    },

    /// If/else statement
    If {
        condition: Expression,
        then_branch: Vec<Statement>,
        else_branch: Option<Vec<Statement>>,
    },

    /// Case statement
    Case {
        selector: Expression,
        arms: Vec<CaseArm>,
        default: Option<Vec<Statement>>,
    },
}

/// Arm of a case statement
#[derive(Debug, Clone)]
pub struct CaseArm {
    pub pattern: Pattern,
    pub body: Vec<Statement>,
}

/// Pattern in case statements
#[derive(Debug, Clone)]
pub enum Pattern {
    /// Literal value (e.g., 8'h42)
    Literal(String),
    /// Range of values (e.g., [0:7])
    Range(String, String),
    /// Default case
    Default,
}

/// Expressions used in assignments and conditions
#[derive(Debug, Clone)]
pub enum Expression {
    /// Variable reference
    Var(String),

    /// Literal value (width in bits, value)
    Literal {
        width: usize,
        value: u128,
    },

    /// String literal for complex values
    StringLiteral(String),

    /// Binary operation
    BinaryOp {
        left: Box<Expression>,
        op: BinaryOp,
        right: Box<Expression>,
    },

    /// Unary operation
    UnaryOp {
        op: UnaryOp,
        operand: Box<Expression>,
    },

    /// Bit selection (sig[index])
    BitSelect {
        signal: Box<Expression>,
        index: Box<Expression>,
    },

    /// Range selection (sig[high:low])
    RangeSelect {
        signal: Box<Expression>,
        high: Box<Expression>,
        low: Box<Expression>,
    },

    /// Concatenation {a, b, c}
    Concat(Vec<Expression>),

    /// Ternary conditional (condition ? true_val : false_val)
    Ternary {
        condition: Box<Expression>,
        then_val: Box<Expression>,
        else_val: Box<Expression>,
    },
}

/// Binary operators
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,

    // Bitwise
    And,
    Or,
    Xor,
    LeftShift,
    RightShift,

    // Logical
    LogicalAnd,
    LogicalOr,

    // Comparison
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
}

/// Unary operators
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Bitwise NOT (~)
    BitwiseNot,
    /// Logical NOT (!)
    LogicalNot,
    /// Reduction AND (&)
    ReductionAnd,
    /// Reduction OR (|)
    ReductionOr,
    /// Reduction XOR (^)
    ReductionXor,
}

impl ModuleIR {
    /// Create a new combinational module
    pub fn combinational(name: String, ports: Vec<PortDecl>, exprs: Vec<ContinuousAssign>) -> Self {
        Self {
            name,
            ports,
            internal_wires: Vec::new(),
            submodules: Vec::new(),
            logic: ModuleLogic::Combinational {
                expressions: exprs,
            },
        }
    }

    /// Create a new sequential module
    pub fn sequential(
        name: String,
        ports: Vec<PortDecl>,
        clock: String,
        registers: Vec<RegisterDecl>,
        always_block: AlwaysBlock,
        output_assigns: Vec<ContinuousAssign>,
    ) -> Self {
        Self {
            name,
            ports,
            internal_wires: Vec::new(),
            submodules: Vec::new(),
            logic: ModuleLogic::Sequential {
                clock,
                reset: None,
                registers,
                always_block,
                output_assigns,
            },
        }
    }
}

impl Expression {
    /// Create a literal with inferred width from value
    pub fn literal(width: usize, value: u128) -> Self {
        Expression::Literal { width, value }
    }

    /// Create a variable reference
    pub fn var(name: impl Into<String>) -> Self {
        Expression::Var(name.into())
    }

    /// Create a binary operation
    pub fn binary(left: Expression, op: BinaryOp, right: Expression) -> Self {
        Expression::BinaryOp {
            left: Box::new(left),
            op,
            right: Box::new(right),
        }
    }

    /// Create a ternary conditional
    pub fn ternary(cond: Expression, then_val: Expression, else_val: Expression) -> Self {
        Expression::Ternary {
            condition: Box::new(cond),
            then_val: Box::new(then_val),
            else_val: Box::new(else_val),
        }
    }
}
