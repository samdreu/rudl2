pub mod ir;
pub mod frontend_ir;
pub mod chir;
pub mod memory;
pub mod port;
pub mod shir;
pub mod vlir;
pub mod cdc;
pub mod types;

pub use ir::{ModuleIR};
pub use frontend_ir::{
    ClockParamMeta,
    FrontendClassification,
    FrontendFnIR,
    FrontendImplIR,
    FrontendModuleIR,
    FrontendTraitIR,
    GenericParamKind,
    GenericParamMeta,
    HardwareMode,
    Receiver,
    FrontendSignature,
    RawParam,
    RawStmt,
    RawStmtKind,
    RawTypeRef,
    SourceSpan,
};
pub use types::{Bits, Clock, ClockDomain, HasUnknown, Logic};
pub use memory::{Memory, ReadMode, WriteMode};
pub use port::{WireId, WireKind};

// pub mod types;
// pub mod verification;
// pub use verification::Module;

// // Public API exports
// pub use types::{
//     Bit, Bits, Clock, ClockDomain, Logic, Signal, State,
// };


#[derive(Debug, Clone, PartialEq, Eq, Hash)]

pub struct Wire<const N: usize> {
    name: String,
    value: [Logic; N],
    dir: Direction,
}

impl<const N: usize> Wire<N> {
    pub fn new(name: &str, dir: Direction) -> Self {
        Self {
            name: name.to_string(),
            value: [Logic::Zero; N],
            dir,
        }
    }

    pub fn get_value(&self) -> &[Logic; N] {
        &self.value
    }

    pub fn set_value(&mut self, value: [Logic; N]) {
        self.value = value;
    }

    pub fn get_direction(&self) -> Direction {
        self.dir
    }
}

pub struct Register<const N: usize> {
    value: [Logic; N],
    dir: Direction,
}

impl<const N: usize> Register<N> {
    pub fn new(dir: Direction) -> Self {
        Self {
            value: [Logic::X; N],
            dir,
        }
    }

    pub fn get_value(&self) -> &[Logic; N] {
        &self.value
    }

    // maybe change to next_value for clocked behavior
    pub fn set_value(&mut self, value: [Logic; N]) {
        self.value = value;
    }

    pub fn set_value_from(&mut self, src: &[Logic; N]) {
        self.value = *src;
    }

    pub fn get_direction(&self) -> Direction {
        self.dir
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Input,
    Output,
    Internal,
}

#[derive(Debug, Clone)]
pub struct Port {
    pub name: String,
    pub width: usize,
    pub direction: Direction,
}

pub trait Module {
    fn execute(&mut self);
    fn get_design_ast(&self) -> FunctionAst;
    fn get_ports(&self) -> Vec<Port>;
    fn to_ir(&self) -> ModuleIR;
}

pub struct FunctionAst {
    pub name: String,
    pub ast: String,
}
