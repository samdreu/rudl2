// use copper_core::{Module, Direction, ModuleIR, Statement, Expression, Signal, UnaryOp, BinaryOp};
mod parser;
mod verilog;
pub mod chir_lower;
pub mod shir_lower;
pub mod vlir_lower;
pub mod emit;

pub use parser::capture_frontend_ir;
pub use chir_lower::{lower_to_chir, ModuleRegistry};
pub use shir_lower::lower_to_shir;
pub use vlir_lower::lower_to_vlir;
pub use emit::{emit_verilog, EmitConfig};

/// End-to-end transpile of a single `#[hardware]` module given as a parsed
/// `syn::ItemFn`, plus the set of hardware fn names and their FIR registry
/// (for submodule resolution). Returns SystemVerilog text.
pub fn transpile_item_fn(
    design_fn: &syn::ItemFn,
    hardware_fns: &std::collections::HashSet<String>,
    registry: &ModuleRegistry,
    config: &EmitConfig,
) -> Result<String, String> {
    let fir = capture_frontend_ir(design_fn, hardware_fns).map_err(|e| format!("{e:?}"))?;
    let chir = lower_to_chir(&fir, hardware_fns, registry).map_err(|e| format!("{e}"))?;
    let shir = lower_to_shir(&chir).map_err(|e| format!("{e}"))?;
    let vlir = lower_to_vlir(&shir).map_err(|e| format!("{e}"))?;
    Ok(emit_verilog(&vlir, config))
}

/// Transpile one hardware module out of a Rust *source string*. Finds hardware
/// modules (by `#[hardware]` attribute or `Clock`/`In`/`Out` signature), builds
/// a FIR registry across all of them for submodule resolution, and transpiles
/// the selected module. `module` may be `None` when the source has exactly one
/// hardware module. Used by the `copper-transpile` CLI and the test harness.
pub fn transpile_source(
    src: &str,
    module: Option<&str>,
    config: &EmitConfig,
) -> Result<String, String> {
    use std::collections::{HashMap, HashSet};

    let file = syn::parse_file(src).map_err(|e| format!("parse error: {e}"))?;
    let modules: Vec<syn::ItemFn> = file
        .items
        .iter()
        .filter_map(|it| match it {
            syn::Item::Fn(f) if is_hardware_fn(f) => Some(f.clone()),
            _ => None,
        })
        .collect();

    if modules.is_empty() {
        return Err("no hardware modules found in source".to_string());
    }
    let names: Vec<String> = modules.iter().map(|f| f.sig.ident.to_string()).collect();
    let hardware_fns: HashSet<String> = names.iter().cloned().collect();

    let mut registry: ModuleRegistry = HashMap::new();
    for f in &modules {
        if let Ok(fir) = capture_frontend_ir(f, &hardware_fns) {
            registry.insert(f.sig.ident.to_string(), fir);
        }
    }

    let target = match module {
        Some(name) => modules
            .iter()
            .find(|f| f.sig.ident == name)
            .ok_or_else(|| format!("module '{name}' not found; available: {}", names.join(", ")))?,
        None if modules.len() == 1 => &modules[0],
        None => {
            return Err(format!(
                "{} modules found; specify one. Available: {}",
                modules.len(),
                names.join(", ")
            ))
        }
    };

    transpile_item_fn(target, &hardware_fns, &registry, config)
}

/// A function is a hardware module if it carries `#[hardware(...)]` or has at
/// least one `Clock<D>` / `In<T,D>` / `Out<T,D>` parameter.
pub fn is_hardware_fn(f: &syn::ItemFn) -> bool {
    let has_attr = f
        .attrs
        .iter()
        .any(|a| a.path().segments.last().map(|s| s.ident == "hardware").unwrap_or(false));
    if has_attr {
        return true;
    }
    f.sig.inputs.iter().any(|arg| {
        if let syn::FnArg::Typed(pt) = arg {
            matches!(outer_type_name(&pt.ty).as_deref(), Some("Clock" | "In" | "Out"))
        } else {
            false
        }
    })
}

fn outer_type_name(ty: &syn::Type) -> Option<String> {
    if let syn::Type::Path(tp) = ty {
        tp.path.segments.last().map(|s| s.ident.to_string())
    } else {
        None
    }
}

use copper_core::{Module};
use parser::IRBuilder;
use verilog::VerilogGenerator;
use syn::parse_str;

// Parse AST to extract:
// 1. Input/Output ports (from Wire/Register declarations with Direction)
// 2. Logic operations (assignments, conditionals)
// 3. Sequential logic (Register updates)

pub fn to_verilog<M: Module>(module: &M) -> String {
    let ast_data = module.get_design_ast();
    let ports = module.get_ports();
    
    let design_fn = parse_str(&ast_data.ast).expect("Failed to parse AST");
    
    match IRBuilder::from_ast(&design_fn, ports) {
        Ok(mut ir) => {
            ir.name = ast_data.name;
            VerilogGenerator::generate(&ir)
        }
        Err(e) => format!("// Error: {}\n", e),
    }
}
