// use proc_macro::TokenStream;
// use quote::quote;
// use syn::{parse_macro_input, Data, Fields, DeriveInput};

// #[proc_macro_attribute]
// pub fn print_ast(_attr: TokenStream, input: TokenStream) -> TokenStream {
//     let ast = parse_macro_input!(input as Item);
//     println!("{:#?}", ast);

//     quote!(#ast).into()
// }

// #[proc_macro_attribute]
// pub fn design_module_struct(_attr: TokenStream, input: TokenStream) -> TokenStream {
//     let input = parse_macro_input!(input as Item);

//     let module_name = input.ident.to_string();
// }

// #[proc_macro_attribute]
// pub fn design_module_impl(_attr: TokenStream, input: TokenStream) -> TokenStream {
//     let input = parse_macro_input!(input as Item);

//     // for impl_item
// }

// #[proc_macro_derive(DescribeModule)]
// pub fn describe_module_derive(input: TokenStream) -> TokenStream {
//     let input = parse_macro_input!(input as DeriveInput);
//     let struct_name = input.ident;

//     let mut field_names = Vec::new();
//     let mut field_types = Vec::new();

//     if let Data::Struct(data_struct) = &input.data {
//         if let Fields::Named(fields_named) = &data_struct.fields {
//             for field in &fields_named.named {
//                 let name = field.ident.as_ref().unwrap();
//                 let ty = &field.ty;
//                 field_names.push(quote! { stringify!(#name) });
//                 field_types.push(quote! { stringify!(#ty) });
//             }
//         }
//     }

//     let verilog = quote! {
//         impl #struct_name {
//             pub fn describe() {
//                 println!("Module: {}", stringify!(#struct_name));
//                 #(
//                     println!("  Field: {}: {}", #field_names, #field_types);
//                 )*
//             }
//         }
//     };
//     verilog.into()
// }

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemStruct, ItemImpl, Fields};
mod model;
use model::{FieldAction, MethodAction};


#[proc_macro_attribute]
pub fn describe_module(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemStruct);
    let struct_name = &input.ident;

    // let mut field_names = Vec::new();
    // let mut field_types = Vec::new();

    let mut field_actions = Vec::new();


    if let Fields::Named(fields_named) = &input.fields {
        for field in &fields_named.named {
            let name = field.ident.as_ref().unwrap();
            let ty = &field.ty;
            // field_names.push(quote! { stringify!(#name) });
            // field_types.push(quote! { stringify!(#ty) });
            
            let field_action = quote! {
                FieldAction {
                    field_name: #name.to_string(),
                    field_type: stringify!(#ty).to_string(),
                    actions: Vec::new(),
                    
                }
            };
            field_actions.push(field_action);
            }
        }
    }

    // Generate a function that returns the list at runtime
    let new_gen = quote! {
        #input
        impl #struct_name {
            pub fn field_actions() -> Vec<FieldAction> {
                vec![
                    #(#field_actions),*
                ]
            }
            pub fn describe_module() {
                println!("Module: {}", stringify!(#struct_name));
                for f in Self::field_actions() {
                    println!("  Field: {}: {}", f.field_name, f.field_type);
                }
            }
        }
    };
    new_gen.into()
}

#[proc_macro_attribute]
pub fn describe_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);
    let self_ty = &input.self_ty;
    let mut method_names = Vec::new();

    for item in &input.items {
        if let syn::ImplItem::Fn(m) = item {
            let name = &m.sig.ident;
            method_names.push(quote! { stringify!(#name) });
        }
    }

    let verilog = quote! {
        #input
        impl #self_ty {
            pub fn describe_impl_methods() {
                println!("Methods for {}:", stringify!(#self_ty));
                #(
                    println!("  Method: {}", #method_names);
                )*
            }
        }
    };
    verilog.into()
}
