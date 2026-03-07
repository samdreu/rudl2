use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Error, FnArg, ItemFn, Pat, ReturnType, Type};

/// #[hardware] macro for defining hardware modules
/// 
/// Marks an async function as a hardware module.
/// - Function parameters become input ports
/// - Return type becomes the output port  
/// - Local variables crossing .await become registers (implicit)
///
/// Experimental mode:
/// - `#[hardware(function_typed)]` enforces read-only input signatures and
///   a non-unit return type for staged migration to function-typed modules.
/// 
/// Currently a marker; real work happens in the executor.
#[proc_macro_attribute]
pub fn hardware(args: TokenStream, input: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(input as ItemFn);
    let function_typed = match parse_function_typed_flag(args) {
        Ok(flag) => flag,
        Err(err) => return err.to_compile_error().into(),
    };
    
    if let Err(err) = validate_hardware_fn(&input_fn, function_typed) {
        return err.to_compile_error().into();
    }
    
    // Just pass through - executor handles the rest
    quote! {
        #input_fn
    }.into()
}

fn parse_function_typed_flag(args: TokenStream) -> Result<bool, Error> {
    let args_text = args.to_string();
    let normalized = args_text.replace(' ', "");
    if normalized.is_empty() {
        return Ok(false);
    }
    if normalized == "function_typed" {
        return Ok(true);
    }
    Err(Error::new(
        proc_macro2::Span::call_site(),
        "Unsupported #[hardware(...)] argument. Supported: function_typed",
    ))
}

fn validate_hardware_fn(input_fn: &ItemFn, function_typed: bool) -> Result<(), Error> {
    if input_fn.sig.asyncness.is_none() {
        return Err(Error::new_spanned(
            &input_fn.sig,
            "#[hardware] can only be applied to async functions",
        ));
    }

    if !function_typed {
        return Ok(());
    }

    match &input_fn.sig.output {
        ReturnType::Default => {
            return Err(Error::new_spanned(
                &input_fn.sig.output,
                "#[hardware(function_typed)] requires an explicit non-unit return type",
            ));
        }
        ReturnType::Type(_, ty) => {
            if matches!(&**ty, Type::Tuple(tuple) if tuple.elems.is_empty()) {
                return Err(Error::new_spanned(
                    &input_fn.sig.output,
                    "#[hardware(function_typed)] does not allow unit `()` return type",
                ));
            }
        }
    }

    for arg in &input_fn.sig.inputs {
        if let FnArg::Typed(pat_ty) = arg {
            if let Pat::Ident(pat_ident) = &*pat_ty.pat {
                if pat_ident.mutability.is_some() {
                    return Err(Error::new_spanned(
                        &pat_ty.pat,
                        "function_typed inputs must be read-only; remove `mut`",
                    ));
                }
            }

            if let Type::Reference(type_ref) = &*pat_ty.ty {
                if type_ref.mutability.is_some() {
                    return Err(Error::new_spanned(
                        &pat_ty.ty,
                        "function_typed inputs cannot be mutable references",
                    ));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{validate_hardware_fn};
    use syn::parse_quote;

    #[test]
    fn function_typed_requires_return_type() {
        let item_fn: syn::ItemFn = parse_quote! {
            async fn stage(clk: Clock<MainClk>, input: u8) {
                loop {
                    clk.tick().await;
                }
            }
        };

        let result = validate_hardware_fn(&item_fn, true);
        assert!(result.is_err());
    }

    #[test]
    fn function_typed_rejects_mut_parameters() {
        let item_fn: syn::ItemFn = parse_quote! {
            async fn stage(clk: Clock<MainClk>, mut input: u8) -> u8 {
                loop {
                    clk.tick().await;
                }
            }
        };

        let result = validate_hardware_fn(&item_fn, true);
        assert!(result.is_err());
    }

    #[test]
    fn function_typed_accepts_read_only_signature() {
        let item_fn: syn::ItemFn = parse_quote! {
            async fn stage(clk: Clock<MainClk>, input: u8) -> u8 {
                loop {
                    let _ = input;
                    clk.tick().await;
                }
            }
        };

        let result = validate_hardware_fn(&item_fn, true);
        assert!(result.is_ok());
    }
}
