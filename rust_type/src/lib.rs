use core::panic;
use std::collections::VecDeque;
use std::ops::{BitAnd, Not, Add};
use std::fmt::Debug;
use syn::token::Const;
use syn::{parse_str, ItemFn, Stmt, Expr, Pat};
use quote::ToTokens;

pub mod register;
pub mod wire;
pub mod logic;
pub use register::Register;
pub use wire::Wire;
pub use logic::Logic;

pub struct FunctionAst {
    pub name: String,
    pub ast: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
    Internal,
}

pub trait Module {
    // type Input;
    // type Output;
    fn get_design_ast(&self) -> FunctionAst;
    fn design(&mut self);
    // fn design(&mut self, choice: Mode, input: Self::Input, output: &mut Self::Output);
}


// needed?
pub trait New {
    fn new() -> Self;
}

// needed?
pub trait ClockAdvance {
    fn clock_advance(&mut self) -> Self;
}

// needed?
// pub trait RegisterTrait: Debug + Send {
//     fn bit_width(&self) -> usize;
//     fn get_value(&self) -> Vec<Logic>;
// }

// TODO: 
// fn process_expr(var_name: &str, expr: &Expr) -> String {
//     match expr {
//         Expr::Binary
//     }
// })

// TODO: Fix!
// fn expr_to_string(expr: &syn::Expr) -> String {
//     match expr {
//         syn::Expr::Lit(lit) => lit.to_token_stream().to_string(),
//         syn::Expr::Path(path) => path.to_token_stream().to_string(),
//         syn::Expr::Binary(bin) => format!(
//             "({} {} {})",
//             expr_to_string(&bin.left),
//             bin.op.to_token_stream(),
//             expr_to_string(&bin.right)
//         ),
//         syn::Expr::Unary(unary) => format!("{}{}", unary.op.to_token_stream(), expr_to_string(&unary.expr)),
//         _ => panic!("Unsupported expression type: {:?}", expr),
//     }
// }

// TODO:
// fn process_block(block: &syn::Block) -> Vec<String> {
//     // todo
// }

// fn contains_register_assignment(block: &syn::Block) -> bool {
//     // todo
// }

// // TODO!
// fn extract_fields(expr: &Expr, input_fields: &mut Vec<String>, output_fields: &mut Vec<String>, declarations: &mut Vec<String>) {
//     match expr {

//     }
// }

enum Construct {
    Register,
    Wire,
    Logic,
}

// TODO: think about what to support and how to handle this
enum Options {
    If,
    Assign,
}

struct Operation {
    construct: Construct,
    option: Options,
}

struct Initialization {
    name: String,
    construct: Construct,
}

pub fn to_verilog<M: Module>(module: &M) -> String{
    let design_ast = module.get_design_ast();
    let ast_str = &design_ast.ast;

    println!("Debug AST:\n{}", ast_str);

    let ast: ItemFn = parse_str(ast_str).unwrap_or_else(|e| {
        panic!("Failed to parse design AST as ItemFn: {}", e)
    });

    println!("Debug AST (pretty):\n{}", ast.to_token_stream());

    // maybe make this a struct?
    let mut verilog = String::new();
    // let mut declarations = Vec::new();
    // let mut always_comb = Vec::new();
    // let mut always_ff = Vec::new();
    
    // let mut input_fields = Vec::new();
    // let mut output_fields = Vec::new();

    let mut initializations : Vec<Initialization> = Vec::new();
    // // hardware constructs that are made
    let mut operations : VecDeque<Operation> = VecDeque::new();
    
    for stmt in &ast.block.stmts {
        match stmt {
            syn::Stmt::Local(local) => {
                parse_local(local, initializations);
            }
            syn::Stmt::Item(item) => {}
            syn::Stmt::Expr(expr, _) => {}
            syn::Stmt::Macro(mac) => { panic!("Macros are not supported yet in design"); },
        }
    }

    // TODO: finesse queue structure
    fn parse_ast<M: Module>(module: &M) -> VecDeque<Operation> {
        let design_ast = module.get_design_ast();
        let ast_str = &design_ast.ast;
        let design_name = &design_ast.name;

        println!("Debug AST:\n{}", ast_str);

        let ast: ItemFn = parse_str(ast_str).unwrap_or_else(|e| {
            panic!("Failed to parse design AST as ItemFn: {}", e)
        });

        println!("Debug AST Pretty: \n{}", ast.to_token_stream());

        let mut operations : VecDeque<Operation> = VecDeque::new();

        for stmt in &ast.block.stmts {
            match stmt {
                syn::Stmt::Local(local) => {parse_local(local)} // local (let) binding
                syn::Stmt::Item(item) => {}
                syn::Stmt::Expr(expr, _) => {}
                syn::Stmt::Macro(mac) => { panic!("Macros are not supported yet"); },
                
            }
        }

        operations
    }
    
    /*
    plan:
    1. parse AST to get all of the hardware constructs and
    make queue pairing the hardware construct with the methods enacted on them.

    2. parse the queue to generate the corresponding Verilog code.
       2.1. for each parse, add the corresponding struct to the lists of verilog stuff?
    
    3. generate the Verilog code.
    
    
     */

    // parse AST


    verilog

}

use syn::{ PatIdent, PatType};
fn parse_local(local: &syn::Local, initializations: &mut Vec<String>) {
    // pat is whats on last of =
    // init is whats on the right (Option)
    let pattern = &local.pat;
    initializations.append(pattern);
    if let Some((eq_token, init_expr)) = &local.init {

    }
}


// fn parse_local(local: &syn::Local, initializations: &mut Vec<String>) {
//     // Handle `let x: Type = ...;`
//     if let Pat::Type(PatType { pat, ty, .. }) = *local.pat {
//         if let Pat::Ident(PatIdent { ident, .. }) = &**pat {
//             let var_name = ident.to_string();
//             let var_type = ty.to_token_stream().to_string();
//             println!("Variable: {}", var_name);
//             println!("Type: {}", var_type);
//         }
//     }
//     // Handle `let x = ...;`
//     else if let Pat::Ident(PatIdent { ident, .. }) = &local.pat {
//         let var_name = ident.to_string();
//         println!("Variable: {}", var_name);
//         println!("Type: <not specified>");
//     }

