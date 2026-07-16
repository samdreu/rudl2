//! `copper-transpile` — Milestone-1 CLI driver.
//!
//! Reads a Rust source file, finds `#[hardware]` modules (or functions with a
//! hardware signature: `Clock<D>` / `In<T,D>` / `Out<T,D>` params), and emits
//! SystemVerilog for one of them via the full FIR → CHIR → SHIR → VLIR → text
//! pipeline.
//!
//! Usage:
//!   copper-transpile <input.rs> [-o <out.sv>] [--module <name>] [--profile <p>]
//!
//!   -o <path>         write output to a file (default: stdout)
//!   --module <name>   which module to emit (required if the file has >1)
//!   --profile <p>     verilator (default) | generic | yosys
//!   --list            list the hardware modules found, then exit

use std::process::ExitCode;

use copper_codegen::{is_hardware_fn, transpile_source, EmitConfig};
use copper_core::vlir::ToolchainProfile;
use syn::Item;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

struct Cli {
    input: String,
    output: Option<String>,
    module: Option<String>,
    profile: ToolchainProfile,
    list: bool,
}

fn parse_args(args: &[String]) -> Result<Cli, String> {
    let mut input = None;
    let mut output = None;
    let mut module = None;
    let mut profile = ToolchainProfile::Verilator;
    let mut list = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--output" => {
                i += 1;
                output = Some(args.get(i).ok_or("-o requires a path")?.clone());
            }
            "--module" => {
                i += 1;
                module = Some(args.get(i).ok_or("--module requires a name")?.clone());
            }
            "--profile" => {
                i += 1;
                profile = match args.get(i).map(String::as_str) {
                    Some("verilator") => ToolchainProfile::Verilator,
                    Some("generic") => ToolchainProfile::Generic,
                    Some("yosys") => ToolchainProfile::Yosys,
                    other => return Err(format!("unknown profile: {other:?}")),
                };
            }
            "--list" => list = true,
            "-h" | "--help" => return Err("see header of src/main.rs for usage".to_string()),
            other if other.starts_with('-') => return Err(format!("unknown flag: {other}")),
            other => {
                if input.is_some() {
                    return Err(format!("unexpected extra argument: {other}"));
                }
                input = Some(other.to_string());
            }
        }
        i += 1;
    }

    Ok(Cli {
        input: input.ok_or("missing input .rs file")?,
        output,
        module,
        profile,
        list,
    })
}

fn run(args: &[String]) -> Result<(), String> {
    let cli = parse_args(args)?;

    let src = std::fs::read_to_string(&cli.input)
        .map_err(|e| format!("reading {}: {e}", cli.input))?;

    if cli.list {
        let file = syn::parse_file(&src).map_err(|e| format!("parsing {}: {e}", cli.input))?;
        println!("hardware modules in {}:", cli.input);
        for it in &file.items {
            if let Item::Fn(f) = it {
                if is_hardware_fn(f) {
                    println!("  {}", f.sig.ident);
                }
            }
        }
        return Ok(());
    }

    let config = EmitConfig { profile: cli.profile, ..Default::default() };
    let sv = transpile_source(&src, cli.module.as_deref(), &config)?;

    match &cli.output {
        Some(path) => {
            std::fs::write(path, &sv).map_err(|e| format!("writing {path}: {e}"))?;
            eprintln!("wrote {} ({} bytes)", path, sv.len());
        }
        None => print!("{sv}"),
    }
    Ok(())
}
