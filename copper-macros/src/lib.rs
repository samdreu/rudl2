use proc_macro::TokenStream;
use quote::quote;
use std::collections::HashSet;
use syn::{parse_macro_input, visit::Visit, visit_mut::VisitMut, Error, FnArg, ItemFn, Pat, ReturnType, Type};

enum HardwareMode {
    Sequential,
    Combinational,
}

fn parse_hardware_mode(args: TokenStream) -> Result<HardwareMode, Error> {
    let text = args.to_string().replace(' ', "");
    match text.as_str() {
        "sequential" => Ok(HardwareMode::Sequential),
        "combinational" => Ok(HardwareMode::Combinational),
        _ => Err(Error::new(
            proc_macro2::Span::call_site(),
            "Unsupported #[hardware(...)] argument. Supported: sequential, combinational",
        )),
    }
}

/// Returns the outer type name (last path segment ident) for a type like `Clock<D>`, `In<T>`, `Out<T>`.
fn outer_type_name(ty: &Type) -> Option<String> {
    if let Type::Path(type_path) = ty {
        type_path.path.segments.last().map(|s| s.ident.to_string())
    } else {
        None
    }
}

/// Visits an async fn body and records which clock parameter names have a `.tick().await` call.
struct TickAwaitVisitor<'a> {
    clock_names: &'a HashSet<String>,
    found: HashSet<String>,
}

impl<'ast, 'a> Visit<'ast> for TickAwaitVisitor<'a> {
    fn visit_expr_await(&mut self, node: &'ast syn::ExprAwait) {
        if let syn::Expr::MethodCall(method) = &*node.base {
            if method.method == "tick" && method.args.is_empty() {
                if let syn::Expr::Path(path) = &*method.receiver {
                    if let Some(ident) = path.path.get_ident() {
                        self.found.insert(ident.to_string());
                    }
                }
            }
        }
        syn::visit::visit_expr_await(self, node);
    }
}

/// Returns true if `block` directly contains a `.tick().await` (does not recurse into
/// nested `loop` blocks — those are handled independently by `LoopDeltaInjector`).
fn block_has_tick_await_direct(block: &syn::Block) -> bool {
    struct DirectTickFinder(bool);
    impl<'ast> Visit<'ast> for DirectTickFinder {
        fn visit_expr_await(&mut self, node: &'ast syn::ExprAwait) {
            if let syn::Expr::MethodCall(method) = &*node.base {
                if method.method == "tick" && method.args.is_empty() {
                    self.0 = true;
                }
            }
            syn::visit::visit_expr_await(self, node);
        }
        fn visit_expr_loop(&mut self, _node: &'ast syn::ExprLoop) {
            // stop here — inner loops are processed independently
        }
    }
    let mut finder = DirectTickFinder(false);
    finder.visit_block(block);
    finder.0
}

/// Walks a sequential function body and appends `::copper_sim::delta_yield().await;`
/// to the end of every `loop` block that directly contains a `.tick().await`.
/// This ensures the task suspends between post-tick output writes and the next
/// pre-tick staging, so each pre-tick runs in the pre-edge settle phase with
/// fresh inputs — matching real hardware's setup-time → clock-edge → propagation cycle.
struct LoopDeltaInjector;

impl VisitMut for LoopDeltaInjector {
    fn visit_expr_loop_mut(&mut self, node: &mut syn::ExprLoop) {
        let needs_delta = block_has_tick_await_direct(&node.body);
        syn::visit_mut::visit_expr_loop_mut(self, node); // transform nested loops first
        if needs_delta {
            let barrier: syn::Stmt = syn::parse_quote! {
                ::copper_sim::pre_edge_barrier().await;
            };
            node.body.stmts.push(barrier);
        }
    }
}

fn wrap_combinational(mut f: ItemFn) -> TokenStream {
    let body = &f.block;
    let new_body: syn::Block = syn::parse_quote! {
        {
            loop {
                #body
                ::copper_sim::delta_yield().await;
            }
        }
    };
    f.sig.asyncness = Some(syn::token::Async {
        span: proc_macro2::Span::call_site(),
    });
    f.block = Box::new(new_body);
    quote! { #f }.into()
}

/// #[hardware] macro for defining hardware modules
///
/// Marks a function as a hardware module.
/// - `#[hardware(sequential)]` — async fn, must have Clock<D>, In<T>, Out<T> params, no return value
/// - `#[hardware(combinational)]` — non-async fn, must have In<T> and Out<T> params, no Clock, no return value.
///   The macro wraps the body in `loop { <body>; delta_yield().await; }` automatically for combinational modules.
#[proc_macro_attribute]
pub fn hardware(args: TokenStream, input: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(input as ItemFn);
    let hardware_mode = match parse_hardware_mode(args) {
        Ok(mode) => mode,
        Err(err) => return err.to_compile_error().into(),
    };

    if let Err(err) = validate_hardware_fn(&input_fn, &hardware_mode) {
        return err.to_compile_error().into();
    }

    match hardware_mode {
        HardwareMode::Sequential => {
            let mut f = input_fn;
            LoopDeltaInjector.visit_item_fn_mut(&mut f);
            quote! { #f }.into()
        }
        HardwareMode::Combinational => wrap_combinational(input_fn),
    }
}

/// Validates a hardware function based on its signature and hardware mode.
fn validate_hardware_fn(input_fn: &ItemFn, hardware_mode: &HardwareMode) -> Result<(), Error> {
    // Sequential must be async; combinational must not be async (the macro adds async)
    if input_fn.sig.asyncness.is_none() && matches!(hardware_mode, HardwareMode::Sequential) {
        return Err(Error::new_spanned(
            &input_fn.sig,
            "#[hardware(sequential)] can only be applied to async functions",
        ));
    }
    if input_fn.sig.asyncness.is_some() && matches!(hardware_mode, HardwareMode::Combinational) {
        return Err(Error::new_spanned(
            &input_fn.sig.asyncness,
            "#[hardware(combinational)] must not be async — the macro adds the async wrapper",
        ));
    }

    // Must have no return value
    if let ReturnType::Type(_, ty) = &input_fn.sig.output {
        if !matches!(&**ty, Type::Tuple(t) if t.elems.is_empty()) {
            return Err(Error::new_spanned(
                &input_fn.sig.output,
                "hardware functions must not have a return value — outputs go through Out<T> parameters",
            ));
        }
    }

    // Walk parameters: enforce types and track presence of Clock / In / Out
    let mut has_clock = false;
    let mut has_in = false;
    let mut has_out = false;

    for arg in &input_fn.sig.inputs {
        match arg {
            FnArg::Receiver(_) => {
                return Err(Error::new_spanned(
                    arg,
                    "hardware functions cannot have a self parameter",
                ));
            }
            FnArg::Typed(pat_ty) => {
                // Parameters must be named (no destructuring)
                if !matches!(&*pat_ty.pat, Pat::Ident(_)) {
                    return Err(Error::new_spanned(
                        &pat_ty.pat,
                        "hardware function parameters must be named",
                    ));
                }

                // All parameters must be Clock<D>, In<T>, or Out<T>
                match outer_type_name(&pat_ty.ty).as_deref() {
                    Some("Clock") => has_clock = true,
                    Some("In")    => has_in = true,
                    Some("Out")   => has_out = true,
                    // Some("Vec")   => {
                    //     // Handle Vec<In<T>> or Vec<Out<T>> if needed
                    // }
                    _ => {
                        return Err(Error::new_spanned(
                            &pat_ty.ty,
                            "hardware function parameters must be Clock<D>, In<T>, or Out<T>",
                        ));
                    }
                }
            }
        }
    }

    // Sequential must have at least one Clock parameter
    if matches!(hardware_mode, HardwareMode::Sequential) && !has_clock {
        return Err(Error::new_spanned(
            &input_fn.sig,
            "#[hardware(sequential)] must have at least one Clock<D> parameter",
        ));
    }

    // Combinational cannot have a Clock parameter
    if matches!(hardware_mode, HardwareMode::Combinational) && has_clock {
        return Err(Error::new_spanned(
            &input_fn.sig,
            "#[hardware(combinational)] cannot have a Clock parameter",
        ));
    }

    // Both modes must have at least one In<T> and one Out<T>
    if !has_in {
        return Err(Error::new_spanned(
            &input_fn.sig,
            "hardware functions must have at least one In<T> parameter",
        ));
    }
    if !has_out {
        return Err(Error::new_spanned(
            &input_fn.sig,
            "hardware functions must have at least one Out<T> parameter",
        ));
    }

    // Sequential: every Clock<D> parameter must have at least one `name.tick().await` in the body.
    if matches!(hardware_mode, HardwareMode::Sequential) {
        let clock_names: HashSet<String> = input_fn.sig.inputs.iter()
            .filter_map(|arg| {
                if let FnArg::Typed(pat_ty) = arg {
                    if outer_type_name(&pat_ty.ty).as_deref() == Some("Clock") {
                        if let Pat::Ident(pi) = &*pat_ty.pat {
                            return Some(pi.ident.to_string());
                        }
                    }
                }
                None
            })
            .collect();

        let mut visitor = TickAwaitVisitor { clock_names: &clock_names, found: HashSet::new() };
        visitor.visit_block(&input_fn.block);

        for arg in &input_fn.sig.inputs {
            if let FnArg::Typed(pat_ty) = arg {
                if outer_type_name(&pat_ty.ty).as_deref() == Some("Clock") {
                    if let Pat::Ident(pi) = &*pat_ty.pat {
                        let name = pi.ident.to_string();
                        if !visitor.found.contains(&name) {
                            return Err(Error::new_spanned(
                                arg,
                                format!("#[hardware(sequential)] body must contain `{name}.tick().await`"),
                            ));
                        }
                    }
                }
            }
        }
    }

    // TODO: for sequential functions, verify the body contains a top-level infinite loop

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate_hardware_fn, HardwareMode};
    use syn::parse_quote;

    #[test]
    fn sequential_must_be_async() {
        let f: syn::ItemFn = parse_quote! {
            fn counter(clk: Clock<MainClk>, input: In<u8>, out: Out<u8>) {}
        };
        assert!(validate_hardware_fn(&f, &HardwareMode::Sequential).is_err());
    }

    #[test]
    fn combinational_must_not_be_async() {
        let f: syn::ItemFn = parse_quote! {
            async fn gate(a: In<Logic>, out: Out<Logic>) {}
        };
        assert!(validate_hardware_fn(&f, &HardwareMode::Combinational).is_err());
    }

    #[test]
    fn rejects_return_value() {
        let f: syn::ItemFn = parse_quote! {
            async fn counter(clk: Clock<MainClk>, input: In<u8>, out: Out<u8>) -> u8 { loop {} }
        };
        assert!(validate_hardware_fn(&f, &HardwareMode::Sequential).is_err());
    }

    #[test]
    fn rejects_raw_parameter_type() {
        let f: syn::ItemFn = parse_quote! {
            async fn counter(clk: Clock<MainClk>, input: u8, out: Out<u8>) { loop {} }
        };
        assert!(validate_hardware_fn(&f, &HardwareMode::Sequential).is_err());
    }

    #[test]
    fn sequential_requires_clock() {
        let f: syn::ItemFn = parse_quote! {
            async fn counter(input: In<u8>, out: Out<u8>) { loop {} }
        };
        assert!(validate_hardware_fn(&f, &HardwareMode::Sequential).is_err());
    }

    #[test]
    fn combinational_rejects_clock() {
        let f: syn::ItemFn = parse_quote! {
            fn gate(clk: Clock<MainClk>, a: In<Logic>, out: Out<Logic>) {}
        };
        assert!(validate_hardware_fn(&f, &HardwareMode::Combinational).is_err());
    }

    #[test]
    fn requires_at_least_one_in() {
        let f: syn::ItemFn = parse_quote! {
            async fn counter(clk: Clock<MainClk>, out: Out<u8>) { loop {} }
        };
        assert!(validate_hardware_fn(&f, &HardwareMode::Sequential).is_err());
    }

    #[test]
    fn requires_at_least_one_out() {
        let f: syn::ItemFn = parse_quote! {
            async fn counter(clk: Clock<MainClk>, input: In<u8>) { loop {} }
        };
        assert!(validate_hardware_fn(&f, &HardwareMode::Sequential).is_err());
    }

    #[test]
    fn valid_sequential() {
        let f: syn::ItemFn = parse_quote! {
            async fn counter(clk: Clock<MainClk>, input: In<u8>, out: Out<u8>) {
                loop { clk.tick().await; }
            }
        };
        assert!(validate_hardware_fn(&f, &HardwareMode::Sequential).is_ok());
    }

    #[test]
    fn sequential_missing_tick_await() {
        let f: syn::ItemFn = parse_quote! {
            async fn counter(clk: Clock<MainClk>, input: In<u8>, out: Out<u8>) { loop {} }
        };
        assert!(validate_hardware_fn(&f, &HardwareMode::Sequential).is_err());
    }

    #[test]
    fn valid_combinational() {
        let f: syn::ItemFn = parse_quote! {
            fn gate(a: In<Logic>, b: In<Logic>, out: Out<Logic>) {}
        };
        assert!(validate_hardware_fn(&f, &HardwareMode::Combinational).is_ok());
    }
}
