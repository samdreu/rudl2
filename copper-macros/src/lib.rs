use proc_macro::TokenStream;
use quote::{format_ident, quote};
use std::collections::HashSet;
use syn::{parse_macro_input, visit::Visit, visit_mut::VisitMut, Error, FnArg, ItemFn, Pat, ReturnType, Type};

enum HardwareMode {
    Sequential,
    Combinational,
    /// A clock-domain synchronizer: behaves like `Sequential` but is the
    /// sanctioned domain-crossing point, so it is *exempt* from the CDC check and
    /// may declare a foreign-domain input. See `sync_2ff` and `copper-core/src/cdc.rs`.
    Synchronizer,
}

impl HardwareMode {
    /// Modes that are clocked, async, tick-bearing loops (everything but comb).
    fn is_sequential_like(&self) -> bool {
        matches!(self, HardwareMode::Sequential | HardwareMode::Synchronizer)
    }
}

fn parse_hardware_mode(args: TokenStream) -> Result<HardwareMode, Error> {
    let text = args.to_string().replace(' ', "");
    match text.as_str() {
        "sequential" => Ok(HardwareMode::Sequential),
        "combinational" => Ok(HardwareMode::Combinational),
        "synchronizer" => Ok(HardwareMode::Synchronizer),
        _ => Err(Error::new(
            proc_macro2::Span::call_site(),
            "Unsupported #[hardware(...)] argument. Supported: sequential, combinational, synchronizer",
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

// ── Clock-domain-crossing (CDC) enforcement ─────────────────────────────────
//
// A regular `#[hardware(sequential)]` module is single-domain: every port must be
// in the module's own clock domain. A signal from another domain may only be
// brought in through a *synchronizer* — a `#[hardware(synchronizer)]` module such
// as the library `sync_2ff`, which is exempt from this rule. Combined with the
// phantom-domain types (which already reject wiring a foreign wire into a native
// port), this makes every clock-domain crossing a real, honest synchronizer
// module and forbids crossing domains implicitly inside ordinary logic. See the
// audit in `copper-core/src/cdc.rs`.

mod cdc_check {
    use super::*;

/// The domain type name at the end of a generic type, but only when there are at
/// least `min_args` type arguments. Returns `None` for a non-`Path` domain (e.g.
/// the `()` unit domain) or too few arguments.
///
/// Used two ways: `Clock<D>` has one type arg (`min_args = 1` → the domain),
/// while `In<T, D>` / `Out<T, D>` need two (`min_args = 2` → the *last* is the
/// domain). A one-argument `In<T>` therefore carries the *default* `()` domain,
/// not an explicit one, and is treated as domain-agnostic.
fn type_domain_at(ty: &Type, min_args: usize) -> Option<String> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    let syn::PathArguments::AngleBracketed(ab) = &seg.arguments else { return None };
    let type_args: Vec<&Type> = ab
        .args
        .iter()
        .filter_map(|a| match a {
            syn::GenericArgument::Type(t) => Some(t),
            _ => None,
        })
        .collect();
    if type_args.len() < min_args {
        return None;
    }
    match type_args.last() {
        Some(Type::Path(p)) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None, // e.g. a `()` unit domain
    }
}

/// Domain of a `Clock<D>` (its single type argument).
fn clock_domain(ty: &Type) -> Option<String> {
    type_domain_at(ty, 1)
}

/// *Explicit* domain of an `In<T, D>` / `Out<T, D>` — `None` when the domain is
/// left default (`In<T>` → `()`), which is not considered a crossing.
fn port_domain(ty: &Type) -> Option<String> {
    type_domain_at(ty, 2)
}

fn param_ident(a: &FnArg) -> Option<String> {
    if let FnArg::Typed(pt) = a {
        if let Pat::Ident(pi) = &*pt.pat {
            return Some(pi.ident.to_string());
        }
    }
    None
}

/// Reject foreign-domain ports on a regular sequential module (signature-level).
///
/// Not called for `#[hardware(synchronizer)]` modules — those are the sanctioned
/// crossing points and may declare a foreign-domain input.
pub(crate) fn check_cdc(f: &ItemFn) -> Result<(), Error> {
    // Native domain = the module's clock domain.
    let clocks: Vec<String> = f
        .sig
        .inputs
        .iter()
        .filter_map(|a| match a {
            FnArg::Typed(pt) if outer_type_name(&pt.ty).as_deref() == Some("Clock") => {
                clock_domain(&pt.ty)
            }
            _ => None,
        })
        .collect();

    // Multi-clock: a module with several clocks has no single native domain;
    // richer analysis (which crossing belongs to which edge) is out of scope, so
    // skip rather than guess.
    let native = match clocks.as_slice() {
        [d] => d.clone(),
        _ => return Ok(()),
    };

    for a in &f.sig.inputs {
        let FnArg::Typed(pt) = a else { continue };
        let outer = outer_type_name(&pt.ty);
        let Some(kind) = (match outer.as_deref() {
            Some("In") => Some("input"),
            Some("Out") => Some("output"),
            _ => None,
        }) else {
            continue;
        };
        if let Some(d) = port_domain(&pt.ty) {
            if d != native {
                let name = param_ident(a).unwrap_or_default();
                return Err(Error::new_spanned(a, format!(
                    "clock-domain crossing: {kind} `{name}` is in domain `{d}`, but this module is \
                     clocked on `{native}`. A regular module may not cross clock domains — bring \
                     the signal across with a synchronizer (`copper::sync_2ff`, or your own \
                     `#[hardware(synchronizer)]` module), then use its output in this domain.",
                )));
            }
        }
    }
    Ok(())
}

} // mod cdc_check

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

/// Finds a direct `.tick().await` in a block or statement — does not recurse into
/// nested `loop` blocks (those are handled independently by `LoopDeltaInjector`).
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

/// Returns true if `block` directly contains a `.tick().await`.
fn block_has_tick_await_direct(block: &syn::Block) -> bool {
    let mut finder = DirectTickFinder(false);
    finder.visit_block(block);
    finder.0
}

/// Returns the names of every `In<T, D>`-typed parameter, in declaration order.
fn in_param_names(sig: &syn::Signature) -> Vec<String> {
    sig.inputs
        .iter()
        .filter_map(|arg| {
            if let FnArg::Typed(pat_ty) = arg {
                if outer_type_name(&pat_ty.ty).as_deref() == Some("In") {
                    if let Pat::Ident(pi) = &*pat_ty.pat {
                        return Some(pi.ident.to_string());
                    }
                }
            }
            None
        })
        .collect()
}

/// Checks every `In<T, D>` parameter is used *only* as the direct receiver of
/// a zero-arg `.read()` call — e.g. `step.read()` — anywhere else it appears
/// (through a `.clone()`, a reassignment to a new binding, passed to a helper
/// function, stored in a collection, etc.) is a pattern the per-read freshness
/// check (`SyncedReadRewriter`, below) cannot place correctly, since it can
/// only rewrite call sites it can see directly in this function's own body.
/// Rather than silently leaving such a read unprotected, this is a hard
/// compile error: `#[hardware(sequential)]`'s guarantee (a read only ever
/// blocks when it would otherwise race a testbench-driven update from a
/// premature loop iteration) requires knowing about every read.
///
/// Returns every offending identifier so the caller can report all of them at
/// once, not just the first.
fn find_unprotectable_in_uses(block: &syn::Block, in_params: &HashSet<String>) -> Vec<syn::Ident> {
    struct Finder<'a> {
        in_params: &'a HashSet<String>,
        found: Vec<syn::Ident>,
    }

    impl<'a, 'ast> Visit<'ast> for Finder<'a> {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            let is_recognized_read = node.method == "read"
                && node.args.is_empty()
                && matches!(&*node.receiver, syn::Expr::Path(p)
                    if p.path.get_ident().is_some_and(|id| self.in_params.contains(&id.to_string())));

            if !is_recognized_read {
                // Not the pattern we rewrite — its receiver is fair game for
                // being an unprotectable use (e.g. `a.clone().read()`: this
                // outer call isn't recognized, so we walk into `a.clone()`,
                // whose *own* receiver `a` is a bare path and gets caught by
                // visit_expr_path below).
                self.visit_expr(&node.receiver);
            }
            for arg in &node.args {
                self.visit_expr(arg);
            }
        }

        fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
            if let Some(ident) = node.path.get_ident() {
                if self.in_params.contains(&ident.to_string()) {
                    self.found.push(ident.clone());
                    return;
                }
            }
            syn::visit::visit_expr_path(self, node);
        }
    }

    let mut finder = Finder { in_params, found: Vec::new() };
    finder.visit_block(block);
    finder.found
}

/// Increments a hidden `__copper_wrap` counter at the top of every `loop`
/// block that directly contains a `.tick().await`. `__copper_wrap` is
/// function-scoped (declared once, by the caller, before this runs) and never
/// reset — even loops entered many times (e.g. a "wait for ready" loop nested
/// inside an outer per-instruction loop) just keep counting up, which is what
/// keeps `SyncedReadTracker`'s "has this port's reader wrapped since its last
/// success" check correct on re-entry: a counter that reset per-entry could
/// go backwards relative to a tracker's last-seen value and block a read that
/// should succeed immediately.
struct WrapCounterInjector;

impl VisitMut for WrapCounterInjector {
    fn visit_expr_loop_mut(&mut self, node: &mut syn::ExprLoop) {
        let needs_wrap = block_has_tick_await_direct(&node.body);
        syn::visit_mut::visit_expr_loop_mut(self, node); // nested loops handled independently
        if needs_wrap {
            let incr: syn::Stmt = syn::parse_quote! { __copper_wrap += 1; };
            node.body.stmts.insert(0, incr);
        }
    }
}

/// Rewrites every `<param>.read()` call — for `<param>` one of this function's
/// `In<T, D>` parameters — into a call through the per-port freshness check,
/// using that parameter's hidden tracker and the enclosing loop's
/// `__copper_wrap` counter. Only call after confirming (via
/// `find_unprotectable_in_uses`) that every use of every `In` parameter is one
/// of these recognized call sites.
struct SyncedReadRewriter<'a> {
    in_params: &'a HashSet<String>,
}

impl<'a> VisitMut for SyncedReadRewriter<'a> {
    fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
        syn::visit_mut::visit_expr_mut(self, expr);

        let matched_name = if let syn::Expr::MethodCall(mc) = &*expr {
            (mc.method == "read" && mc.args.is_empty())
                .then(|| match &*mc.receiver {
                    syn::Expr::Path(p) => p.path.get_ident().map(|id| id.to_string()),
                    _ => None,
                })
                .flatten()
                .filter(|name| self.in_params.contains(name))
        } else {
            None
        };

        if let Some(name) = matched_name {
            let port_ident = format_ident!("{}", name);
            let tracker_ident = format_ident!("__copper_tracker_{}", name);
            *expr = syn::parse_quote! {
                ::copper_sim::synced_read::__private::synced_read(
                    &#port_ident, &#tracker_ident, __copper_wrap,
                ).await
            };
        }
    }
}

/// Injects the per-read freshness check into a `#[hardware(sequential)]`
/// function body: a hidden wrap counter, one hidden tracker per `In<T, D>`
/// parameter, and a rewrite of every recognized `.read()` call site to go
/// through them. No-op if the function has no `In` parameters at all — a
/// free-running module has nothing to protect. Returns an error (without
/// modifying `f`) if any `In` parameter is used in a way that can't be
/// rewritten — see `find_unprotectable_in_uses`.
fn inject_synced_reads(f: &mut ItemFn) -> Result<(), Error> {
    let in_params: HashSet<String> = in_param_names(&f.sig).into_iter().collect();
    if in_params.is_empty() {
        return Ok(());
    }

    let bad = find_unprotectable_in_uses(&f.block, &in_params);
    if !bad.is_empty() {
        let mut iter = bad.into_iter();
        let mut err = Error::new_spanned(
            &iter.next().unwrap(),
            "this `In<T, D>` parameter is used in a way #[hardware(sequential)] can't verify \
             is protected against stale/premature reads — only `<param>.read()`, called \
             directly on the parameter with no intervening `.clone()`, reassignment, or \
             indirection, is supported. Read it directly at each place it's needed instead.",
        );
        for ident in iter {
            err.combine(Error::new_spanned(
                &ident,
                "...and here (same parameter, same restriction)",
            ));
        }
        return Err(err);
    }

    let mut wrap_decl: Vec<syn::Stmt> = vec![syn::parse_quote! { let mut __copper_wrap: u64 = 0; }];
    for name in &in_params {
        let tracker_ident = format_ident!("__copper_tracker_{}", name);
        wrap_decl.push(syn::parse_quote! {
            let #tracker_ident = ::copper_sim::synced_read::__private::ReadTracker::new();
        });
    }
    f.block.stmts.splice(0..0, wrap_decl);

    WrapCounterInjector.visit_block_mut(&mut f.block);
    SyncedReadRewriter { in_params: &in_params }.visit_block_mut(&mut f.block);

    Ok(())
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
        HardwareMode::Sequential | HardwareMode::Synchronizer => {
            // Regular sequential modules must be single-domain; synchronizers are
            // the sanctioned crossing point and are exempt.
            if matches!(hardware_mode, HardwareMode::Sequential) {
                if let Err(err) = cdc_check::check_cdc(&input_fn) {
                    return err.to_compile_error().into();
                }
            }
            let mut f = input_fn;
            if let Err(err) = inject_synced_reads(&mut f) {
                return err.to_compile_error().into();
            }
            quote! { #f }.into()
        }
        HardwareMode::Combinational => wrap_combinational(input_fn),
    }
}

/// Validates a hardware function based on its signature and hardware mode.
fn validate_hardware_fn(input_fn: &ItemFn, hardware_mode: &HardwareMode) -> Result<(), Error> {
    // Sequential must be async; combinational must not be async (the macro adds async)
    if input_fn.sig.asyncness.is_none() && hardware_mode.is_sequential_like() {
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

    // Walk parameters: enforce types and track presence of Clock / Out
    let mut has_clock = false;
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
                    Some("In")    => {}
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
    if hardware_mode.is_sequential_like() && !has_clock {
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

    // Both modes must have at least one Out<T>. In<T> is not required — a
    // free-running module (e.g. a counter with no reset/enable) legitimately
    // has no inputs at all.
    if !has_out {
        return Err(Error::new_spanned(
            &input_fn.sig,
            "hardware functions must have at least one Out<T> parameter",
        ));
    }

    // Sequential-like: every Clock<D> parameter must have at least one `name.tick().await` in the body.
    if hardware_mode.is_sequential_like() {
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
    use super::cdc_check::check_cdc;
    use super::{validate_hardware_fn, HardwareMode};
    use syn::parse_quote;

    // ── CDC enforcement (check_cdc — signature-level) ───────────────────────
    //
    // A regular sequential module must be single-domain; foreign-domain crossings
    // go through a `#[hardware(synchronizer)]` module (which is not passed to
    // `check_cdc` at all). So the check is purely: reject any foreign-domain port.

    #[test]
    fn cdc_accepts_native_only_module() {
        let f: syn::ItemFn = parse_quote! {
            async fn m(clk: Clock<Slow>, step: In<Bits<8>, Slow>, q: Out<Bits<8>, Slow>) {
                let mut c = Bits::zero();
                loop { q.write(c); clk.tick().await; c = c + step.read(); }
            }
        };
        assert!(check_cdc(&f).is_ok());
    }

    #[test]
    fn cdc_rejects_foreign_input() {
        let f: syn::ItemFn = parse_quote! {
            async fn m(clk: Clock<Slow>, fast: In<Logic, Fast>, q: Out<Logic, Slow>) {
                loop { q.write(fast.read()); clk.tick().await; }
            }
        };
        assert!(check_cdc(&f).is_err());
    }

    #[test]
    fn cdc_rejects_foreign_output() {
        let f: syn::ItemFn = parse_quote! {
            async fn m(clk: Clock<Slow>, d: In<Logic, Slow>, fq: Out<Logic, Fast>) {
                loop { fq.write(d.read()); clk.tick().await; }
            }
        };
        assert!(check_cdc(&f).is_err());
    }

    #[test]
    fn cdc_accepts_foreign_generic_domain_input_is_still_foreign() {
        // A generic source domain `Src` differs from the concrete clock domain, so
        // it is foreign — a regular module still may not take it directly. (Only a
        // synchronizer may, and synchronizers are never passed to `check_cdc`.)
        let f: syn::ItemFn = parse_quote! {
            async fn m<Src>(clk: Clock<Slow>, d: In<Logic, Src>, q: Out<Logic, Slow>) {
                loop { q.write(d.read()); clk.tick().await; }
            }
        };
        assert!(check_cdc(&f).is_err());
    }

    #[test]
    fn cdc_skips_multiclock_modules() {
        // Two clocks → no unambiguous native domain → not checked here.
        let f: syn::ItemFn = parse_quote! {
            async fn m(cf: Clock<Fast>, cs: Clock<Slow>, fast: In<Logic, Fast>, q: Out<Logic, Slow>) {
                loop { q.write(fast.read()); cf.tick().await; cs.tick().await; }
            }
        };
        assert!(check_cdc(&f).is_ok());
    }

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
    fn allows_missing_in() {
        // A free-running module (no reset/enable) legitimately has no In<T>.
        let f: syn::ItemFn = parse_quote! {
            async fn counter(clk: Clock<MainClk>, out: Out<u8>) { loop { clk.tick().await; } }
        };
        assert!(validate_hardware_fn(&f, &HardwareMode::Sequential).is_ok());
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
