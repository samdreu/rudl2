// use copper_core::{Module, Direction, ModuleIR, Statement, Expression, Signal, UnaryOp, BinaryOp};
mod parser;
mod verilog;
pub mod chir_lower;
pub mod control_extract;
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
    transpile_fir(&fir, hardware_fns, registry, config)
}

/// Transpile an already-captured FIR (CHIR → SHIR → VLIR → text). Use this when
/// the FIR needs enriching first — e.g. injecting file-scope enums, which are not
/// reachable from an `ItemFn` alone.
pub fn transpile_fir(
    fir: &copper_core::FrontendModuleIR,
    hardware_fns: &std::collections::HashSet<String>,
    registry: &ModuleRegistry,
    config: &EmitConfig,
) -> Result<String, String> {
    // Control extraction (FIR→FIR): flatten async control-flow loops whose ticks
    // live inside branches into an explicit single-tick `match pc` FSM — the shape
    // the pipeline below already lowers correctly. No-op for linear modules.
    let mut fir = fir.clone();
    control_extract::extract_control(&mut fir);
    let fir = &fir;

    let chir = lower_to_chir(fir, hardware_fns, registry).map_err(|e| format!("{e}"))?;
    let shir = lower_to_shir(&chir).map_err(|e| format!("{e}"))?;
    let vlir = lower_to_vlir(&shir).map_err(|e| format!("{e}"))?;
    Ok(emit_verilog(&vlir, config))
}

/// Transpile one hardware module out of a Rust *source string*. Finds hardware
/// modules (functions carrying `#[hardware(...)]`), builds a FIR registry
/// across all of them for submodule resolution, and transpiles the selected
/// module. `module` may be `None` when the source has exactly one hardware
/// module. Used by the `copper-transpile` CLI and the test harness.
///
/// A function whose signature has a `Clock`/`In`/`Out` parameter but no
/// `#[hardware(...)]` attribute is rejected rather than silently treated as
/// a hardware module — attributed and non-attributed functions simulate with
/// different timing (the macro injects a pre-edge barrier), so module
/// detection must not paper over the difference.
pub fn transpile_source(
    src: &str,
    module: Option<&str>,
    config: &EmitConfig,
) -> Result<String, String> {
    use std::collections::{HashMap, HashSet};

    let file = syn::parse_file(src).map_err(|e| format!("parse error: {e}"))?;

    let unattributed: Vec<String> = file
        .items
        .iter()
        .filter_map(|it| match it {
            syn::Item::Fn(f) if has_hardware_signature(f) && !is_hardware_fn(f) => {
                Some(f.sig.ident.to_string())
            }
            _ => None,
        })
        .collect();
    if !unattributed.is_empty() {
        return Err(format!(
            "missing #[hardware(...)] attribute on: {} — functions with Clock/In/Out \
             parameters must be marked #[hardware(sequential)] or #[hardware(combinational)]",
            unattributed.join(", ")
        ));
    }

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

    // Enums and other file-scope items are visible to every module in the file
    // but are not reachable from an `ItemFn`, so inject them into each module's
    // FIR. (Nothing consumes the non-enum items yet — capture only, #7a.)
    let file_enums = parser::capture_file_enums(&file);
    let file_scope = parser::capture_file_scope(&file, &hardware_fns);

    let mut registry: ModuleRegistry = HashMap::new();
    for f in &modules {
        if let Ok(mut fir) = capture_frontend_ir(f, &hardware_fns) {
            fir.enums.extend(file_enums.iter().cloned());
            inject_file_scope(&mut fir, &file_scope);
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

    let mut fir = capture_frontend_ir(target, &hardware_fns).map_err(|e| format!("{e:?}"))?;
    fir.enums.extend(file_enums);
    inject_file_scope(&mut fir, &file_scope);
    transpile_fir(&fir, &hardware_fns, &registry, config)
}

/// Copy captured file-scope items into a module's FIR, mirroring the `enums`
/// injection. A clone per module (multi-module files like `uart/system` have a
/// few) — negligible, and matches how enums already work.
fn inject_file_scope(fir: &mut copper_core::FrontendModuleIR, scope: &parser::FileScope) {
    fir.file_fns.extend(scope.fns.iter().cloned());
    fir.file_structs.extend(scope.structs.iter().cloned());
    fir.file_consts.extend(scope.consts.iter().cloned());
    fir.file_impls.extend(scope.impls.iter().cloned());
    fir.file_traits.extend(scope.traits.iter().cloned());
}

/// A function is a hardware module only if it carries `#[hardware(...)]`.
/// A `Clock`/`In`/`Out` signature alone is not enough — see
/// `has_hardware_signature` and `transpile_source`.
pub fn is_hardware_fn(f: &syn::ItemFn) -> bool {
    f.attrs
        .iter()
        .any(|a| a.path().segments.last().map(|s| s.ident == "hardware").unwrap_or(false))
}

/// True if `f`'s signature has a `Clock<D>` / `In<T,D>` / `Out<T,D>` parameter,
/// independent of whether it carries `#[hardware(...)]`. Used to catch
/// functions that look like hardware modules but are missing the attribute.
fn has_hardware_signature(f: &syn::ItemFn) -> bool {
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
