use proc_macro::TokenStream;
use quote::quote;
use syn::{parse2, ItemImpl, ImplItem, LitStr};
use syn::parse::{Parse, ParseStream};

// Define the macro arguments (e.g., module name)
struct ModuleArgs {
    name: String,
}

impl Parse for ModuleArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let lit: LitStr = input.parse()?;
        Ok(ModuleArgs {
            name: lit.value(),
        })
    }
}

// The updated #[module] macro
#[proc_macro_attribute]
pub fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    // Parse the macro arguments (e.g., "test")
    let args = parse2::<ModuleArgs>(args.into()).unwrap_or_else(|e| {
        panic!("Failed to parse module args: {}", e);
    });

    // Parse the input as an impl block
    let input_impl = parse2::<ItemImpl>(input.into()).unwrap_or_else(|e| {
        panic!("Failed to parse impl block: {}", e);
    });

    // Find the `design` method within the impl block
    let design_method = input_impl.items.iter().find_map(|item| {
        if let ImplItem::Fn(method) = item {
            if method.sig.ident == "design" {
                Some(method.clone())
            } else {
                None
            }
        } else {
            None
        }
    }).expect("No `design` method found in impl block");

    let fn_name = &design_method.sig.ident; // Should be "design"
    let module_name = args.name;

    // Stringify only the `design` function, not the entire impl block
    let fn_ast = quote! { #design_method };

    // Generate the expanded code
    let expanded = quote! {
        #input_impl

        impl<const N: usize> rust_type::Module for Counter<N> {
            // type Input = TestInput<N>;
            // type Output = TestOutput<N>;

            fn get_design_ast(&self) -> rust_type::FunctionAst {
                rust_type::FunctionAst {
                    name: #module_name.to_string(),
                    ast: stringify!(#fn_ast).to_string(),
                }
            }

            fn design(&mut self) {
                self.#fn_name();
            }
        }
    };

    TokenStream::from(expanded)
}
