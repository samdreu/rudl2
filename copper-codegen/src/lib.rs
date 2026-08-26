// use copper_core::{Module, Direction, ModuleIR, Statement, Expression, Signal, UnaryOp, BinaryOp};
mod parser;
mod verilog;
pub mod file_consts;
pub mod chir_lower;
pub mod control_extract;
pub mod shir_lower;
pub mod vlir_lower;
pub mod emit;

pub use parser::capture_frontend_ir;
pub use chir_lower::{lower_to_chir, ModuleRegistry};
pub use shir_lower::lower_to_shir;
pub use vlir_lower::lower_to_vlir;
pub use vlir_lower::legalized_port_name;
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
    // `while <cond> { … tick; }` is sugar for the repeating wait extraction
    // already handles; rewrite it before the gate looks at the body.
    control_extract::desugar_tick_waits(&mut fir);
    // …and `for <var> in <a>..<b> { … tick; }` is sugar for a counted one. It runs
    // AFTER the `while` rewrite so a `for` nested inside a tick-bearing `while`
    // is reached: that rewrite moves the body into a fresh `loop`, which this pass
    // then walks.
    control_extract::desugar_counted_loops_in(&mut fir);
    // Why extraction is about to decline, if the reason is a construct the linear
    // path downstream cannot name. Computed BEFORE the pass runs, since a declined
    // module is left untouched and there is nothing to inspect afterwards.
    let declined = control_extract::unflattenable_reason(&fir);
    control_extract::extract_control(&mut fir);
    let fir = &fir;

    // A declined module cannot be flattened, so whatever the linear lowering
    // reports is downstream of that decline — and it blames the first unsupported
    // thing it REACHES, which is routinely not the thing at fault. `uart/rx`
    // reported its well-formed repeating wait (line 55) for a `continue` further
    // down the body. Prefer the construct that actually stopped the flattening,
    // with its own span.
    let chir = lower_to_chir(fir, hardware_fns, registry).map_err(|e| match &declined {
        Some(reason) => format!("{reason}"),
        None => format!("{e}"),
    })?;
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
    let prepared = prepare_source(src)?;
    let target = prepared.select_target(module)?;
    transpile_target(target, &prepared, config)
}

/// Like [`transpile_source`], but emits the selected module **plus every
/// `#[hardware]` submodule it transitively instantiates**, deepest-first, into
/// one self-contained SystemVerilog string. This is what a hierarchical design
/// (item 4's structural parent + its clocked children) needs for a standalone
/// Verilator run — a `module` that instantiates `fast_counter`/`sync_2ff`/… would
/// otherwise reference undefined modules. For a leaf module (no submodules) the
/// output equals `transpile_source`'s (just the one module).
pub fn transpile_source_hierarchy(
    src: &str,
    module: Option<&str>,
    config: &EmitConfig,
) -> Result<String, String> {
    let prepared = prepare_source(src)?;
    let target = prepared.select_target(module)?;
    let order = prepared.hierarchy_emit_order(&target.sig.ident.to_string());

    let mut out = String::new();
    for (i, name) in order.iter().enumerate() {
        let f = prepared
            .modules
            .iter()
            .find(|m| &m.sig.ident.to_string() == name)
            .ok_or_else(|| format!("internal error: module '{name}' vanished from registry"))?;
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&transpile_target(f, &prepared, config)?);
    }
    Ok(out)
}

/// Shared setup for the source-string entry points: parse, reject unattributed
/// hardware-signature functions, collect the hardware modules, and build the FIR
/// registry (with file-scope enums/items injected) used for submodule resolution.
struct PreparedSource {
    modules: Vec<syn::ItemFn>,
    names: Vec<String>,
    hardware_fns: std::collections::HashSet<String>,
    registry: ModuleRegistry,
    file_enums: Vec<copper_core::frontend_ir::ItemEnum>,
    file_scope: parser::FileScope,
}

fn prepare_source(src: &str) -> Result<PreparedSource, String> {
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

    Ok(PreparedSource { modules, names, hardware_fns, registry, file_enums, file_scope })
}

impl PreparedSource {
    fn select_target(&self, module: Option<&str>) -> Result<&syn::ItemFn, String> {
        match module {
            Some(name) => self
                .modules
                .iter()
                .find(|f| f.sig.ident == name)
                .ok_or_else(|| format!("module '{name}' not found; available: {}", self.names.join(", "))),
            None if self.modules.len() == 1 => Ok(&self.modules[0]),
            None => Err(format!(
                "{} modules found; specify one. Available: {}",
                self.modules.len(),
                self.names.join(", ")
            )),
        }
    }

    /// The `#[hardware]` submodules `name` directly instantiates, read off its
    /// lowered CHIR (the authoritative submodule set for every body kind).
    fn direct_deps(&self, name: &str) -> Vec<String> {
        use copper_core::chir::CHIRBody;
        let Some(fir) = self.registry.get(name) else { return Vec::new() };
        let Ok(chir) = lower_to_chir(fir, &self.hardware_fns, &self.registry) else {
            return Vec::new();
        };
        let subs = match &chir.body {
            CHIRBody::Combinational(b) => &b.submodules,
            CHIRBody::Sequential(b) => &b.submodules,
            CHIRBody::Structural(b) => &b.submodules,
        };
        let mut names: Vec<String> = subs.iter().map(|s| s.module_name.clone()).collect();
        names.dedup();
        names
    }

    /// Post-order DFS from `target`: every transitively-instantiated child
    /// appears before its parent, `target` last, each module once. A dependency
    /// not present in the file (an external module) is skipped — emit only what
    /// this file defines.
    fn hierarchy_emit_order(&self, target: &str) -> Vec<String> {
        let mut order = Vec::new();
        let mut visited = std::collections::HashSet::new();
        self.visit_deps(target, &mut visited, &mut order);
        order
    }

    fn visit_deps(&self, name: &str, visited: &mut std::collections::HashSet<String>, order: &mut Vec<String>) {
        if !self.hardware_fns.contains(name) || !visited.insert(name.to_string()) {
            return;
        }
        for dep in self.direct_deps(name) {
            self.visit_deps(&dep, visited, order);
        }
        order.push(name.to_string());
    }
}

/// Does this module carry `#[hardware(sequential, allow_pretick_alignment)]`?
///
/// Read from the first token of the attribute list rather than parsed as an ident
/// list: `parse_args::<syn::Ident>()` fails outright once a flag is present, which
/// is how modules carrying one have silently vanished from corpus scans before.
fn opts_out_of_pretick_alignment(f: &syn::ItemFn) -> bool {
    f.attrs.iter().any(|a| {
        a.path().segments.last().is_some_and(|s| s.ident == "hardware")
            && a.meta
                .require_list()
                .ok()
                .is_some_and(|l| l.tokens.to_string().contains("allow_pretick_alignment"))
    })
}

/// Run the per-module transpile: the shared reachability well-formedness check,
/// register inference (logged), then FIR → SV.
fn transpile_target(
    target: &syn::ItemFn,
    prepared: &PreparedSource,
    config: &EmitConfig,
) -> Result<String, String> {
    // c2 (item 2): the transpiler enforces the SAME reachability well-formedness
    // the sim macro does, from the SAME shared analysis on the SAME `&syn::ItemFn`
    // — a tickless loop path is rejected here too (one authoritative check, both
    // front-ends). In practice a module reaching the transpiler already compiled
    // through the macro's check; this keeps the standalone CLI honest.
    copper_analysis::check_reachability(target).map_err(|e| e.to_string())?;
    // A plain `Out` driven in more than one clock phase. `shir_lower` refuses this
    // on the multi-tick path, but control extraction rewrites the body into a
    // single-tick `match pc` FSM first — so by the time that check runs, the phases
    // it counts are gone. Check the SOURCE, where the ticks are still visible.
    //
    // `allow_pretick_alignment` opts out here exactly as it does in the macro: the
    // flag silences the ERROR, not the detection, and a module that exists to
    // DEMONSTRATE the divergence has to be transpilable or there is nothing to
    // measure it against.
    if !opts_out_of_pretick_alignment(target) {
        if let Some(port) = copper_analysis::multi_phase_out_write(target).first() {
            return Err(format!(
                "output port '{port}' is driven in more than one clock phase (across \
                 clk.tick().await boundaries) — the simulator runs one of those segments \
                 a phase early and disagrees with the synthesized hardware. Declare it as \
                 RegOut<…>, or drive it in exactly one phase"
            ));
        }
    }

    // The memory-port staging rules, for the same reason and from the same place:
    // they are questions about clock EDGES, and control extraction rewrites the
    // ticks that delimit them into `pc` states. `chir_lower` used to ask them of
    // the lowered body's tick-delimited segments, which made an ordinary memory
    // design fail the moment anything else in the module needed extraction.
    copper_analysis::check_memory_staging(target).map_err(|e| e.to_string())?;
    // …and the plain-`Out`-from-a-read-result rule, for the same reason:
    // `vlir_lower::reject_memory_driven_comb_outputs` gives up on `phases.len() < 2`,
    // and an extracted module always has exactly one lowered phase. That check was
    // unreachable-by-accident while the staging rules rejected every extracted
    // memory design; it must not become unreachable-in-fact now they do not.
    if let Some(port) = copper_analysis::memory_result_drives_plain_out(target).first() {
        return Err(format!(
            "output port '{port}' is driven from a memory read result in a multi-phase \
             module. The read pipeline re-captures on every clock edge, so a plain `Out` \
             either tracks it into the phases that do not observe it, or holds one edge \
             late — neither matches the simulator. Declare the port `RegOut<T, D>`, or \
             latch the result into a register and drive the output from that"
        ));
    }

    // c2 (gate G6): the transpiler consumes the SAME shared analysis, on the SAME
    // input (`&syn::ItemFn`), that the sim macro consumes — one authoritative
    // register/CFG analysis, not two that must agree. Read-only for now; item 2
    // routes register inference and item 3 routes read-timing through this.
    let inferred_registers = copper_analysis::infer_registers(target);
    log::debug!(
        "copper-analysis inferred registers for `{}`: {:?}",
        target.sig.ident,
        inferred_registers
    );

    let mut fir = capture_frontend_ir(target, &prepared.hardware_fns).map_err(|e| format!("{e:?}"))?;
    fir.enums.extend(prepared.file_enums.iter().cloned());
    inject_file_scope(&mut fir, &prepared.file_scope);
    transpile_fir(&fir, &prepared.hardware_fns, &prepared.registry, config)
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
