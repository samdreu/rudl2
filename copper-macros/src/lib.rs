use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn};

/// #[hardware] macro for defining hardware modules
/// 
/// Marks an async function as a hardware module.
/// - Function parameters become input ports
/// - Return type becomes the output port  
/// - Local variables crossing .await become registers (implicit)
/// 
/// Currently a marker; real work happens in the executor.
#[proc_macro_attribute]
pub fn hardware(_args: TokenStream, input: TokenStream) -> TokenStream {
    let input_fn = parse_macro_input!(input as ItemFn);
    
    // Validate it's async
    if input_fn.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            &input_fn.sig,
            "#[hardware] can only be applied to async functions"
        )
        .to_compile_error()
        .into();
    }
    
    // Just pass through - executor handles the rest
    quote! {
        #input_fn
    }.into()
}
