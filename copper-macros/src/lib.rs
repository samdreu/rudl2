use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemStruct, ItemImpl, ImplItem, Fields, Type};

#[proc_macro_attribute]
pub fn module_struct(_args: TokenStream, input: TokenStream) -> TokenStream {
    eprintln!("Processing module_struct macro");
    let input_struct = parse_macro_input!(input as ItemStruct);
    let struct_name = &input_struct.ident;
    let generics = &input_struct.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let mut port_entries = Vec::new();

    // ItemStruct.fields is directly accessible
    if let Fields::Named(fields) = &input_struct.fields {
        for field in &fields.named {
            let field_name = field.ident.as_ref().unwrap();
            
            // Parse type to extract Wire/Register and width
            if let Type::Path(type_path) = &field.ty {
                let type_name = &type_path.path.segments.last().unwrap().ident;
                eprint!("Found field: {} of type {}\n", field_name, type_name);
                // Check if it's Wire or Register
                if type_name == "Wire" || type_name == "Register" {
                    eprint!("  It's a port of type {}\n", type_name);
                    // Extract const generic (the width)
                    if let syn::PathArguments::AngleBracketed(args) = 
                        &type_path.path.segments.last().unwrap().arguments {
                        eprintln!("  Has {} angle bracketed args", args.args.len());

                        // Try both Type and Const variants
                        let width_expr = match args.args.first() {
                            Some(syn::GenericArgument::Const(expr)) => {
                                eprintln!("  Found Const width: {}", quote!(#expr));
                                quote!(#expr)
                            }
                            Some(syn::GenericArgument::Type(ty)) => {
                                eprintln!("  Found Type width: {}", quote!(#ty));
                                quote!(#ty)
                            }
                            _ => {
                                eprintln!("  Unexpected generic argument type");
                                continue;
                            }
                        };
                        
                        port_entries.push(quote! {
                            copper_core::Port {
                                name: stringify!(#field_name).to_string(),
                                width: #width_expr,
                                direction: self.#field_name.get_direction(),
                            }
                        });
                    }
                }
            }
        }
    }
    
    // Generate the struct and impl with __get_ports
    quote! {
        #input_struct
        
        impl #impl_generics #struct_name #ty_generics #where_clause {
            pub fn __get_ports(&self) -> Vec<copper_core::Port> {
                vec![
                    #(#port_entries),*
                ]
            }
        }
    }.into()
}

#[proc_macro_attribute]
pub fn module(args: TokenStream, input: TokenStream) -> TokenStream {
    let module_name = args.to_string().trim_matches('"').to_string();
    let input_impl = parse_macro_input!(input as ItemImpl);
    
    let self_ty = &input_impl.self_ty;
    let generics = &input_impl.generics;
    
    let design_method = input_impl.items.iter().find_map(|item| {
        if let ImplItem::Fn(m) = item {
            if m.sig.ident == "design" {
                return Some(m.clone());
            }
        }
        None
    }).expect("design() method required");

    let fn_ast = quote! { #design_method }.to_string();

    quote! {
        #input_impl

        impl #generics copper_core::Module for #self_ty {
            fn get_design_ast(&self) -> copper_core::FunctionAst {
                copper_core::FunctionAst {
                    name: #module_name.to_string(),
                    ast: #fn_ast.to_string(),
                }
            }
            
            fn execute(&mut self) {
                self.design();
            }
            
            fn get_ports(&self) -> Vec<copper_core::Port> {
                self.__get_ports()
            }
            
            fn to_ir(&self) -> copper_core::ModuleIR {
                // Default: empty IR with ports
                copper_core::ModuleIR {
                    name: #module_name.to_string(),
                    ports: self.get_ports(),
                    statements: Vec::new(),
                    submodules: Vec::new(),
                }
            }
        }
    }.into()
}
