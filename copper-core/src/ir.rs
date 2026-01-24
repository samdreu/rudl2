use crate::{Port, Logic};

#[derive(Debug, Clone)]
pub enum HardwareIR {
    Module(ModuleIR),
}

#[derive(Debug, Clone)]
pub struct ModuleIR {
    pub name: String,
    pub ports: Vec<Port>,
    pub statements: Vec<Statement>,
    pub submodules: Vec<ModuleIR>,
}

#[derive(Debug, Clone)]
pub enum Statement {
    Assign {
        target: Signal,
        value: Expression,
    },

    AlwaysFF {
        clock: Signal,
        reset: Option<Signal>,
        assignments: Vec<Assignment>,
    },

    // think i need to add another thing here
    // always comb shouldnt just be assignments
    AlwaysComb {
        assignments: Vec<Assignment>,
    },

    If {
        condition: Expression,
        then_branch: Vec<Statement>,
        else_branch: Option<Vec<Statement>>,
    },

    Case {
        selector: Expression,
        arms: Vec<CaseArm>,
    },
}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub target: Signal,
    pub value: Expression,
}

#[derive(Debug, Clone)]
pub struct CaseArm {
    pub pattern: Pattern,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Literal(LogicValue),
    Range(LogicValue, LogicValue),
    Default,
}

#[derive(Debug, Clone)]
pub enum Expression {
    Signal(Signal),
    Literal(String),
    UnaryOp(UnaryOp, Box<Expression>),
    BinaryOp(Box<Expression>, BinaryOp, Box<Expression>),
    Index(Box<Expression>, Box<Expression>),
    Slice(Box<Expression>, Box<Expression>, Box<Expression>),
    Concat(Vec<Expression>),
}

#[derive(Debug, Clone)]
pub struct Signal {
    pub name: String,
    pub width: usize,
}

#[derive(Debug, Clone)]
pub enum LogicValue {
    Bit(Logic),
    Vector(Vec<Logic>),
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
    Not,
    And, // reduction AND
    Or,  // reduction OR
    Xor, // reduction XOR
}

#[derive(Debug, Clone, Copy)]
pub enum BinaryOp {
    And,
    Or,
    Xor,
    Add,
    Sub,
    Eq,
    Neq,
    Lt,
    Gt,
}
