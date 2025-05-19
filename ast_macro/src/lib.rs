use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, ItemImpl, ImplItemFn, Expr, Stmt, Data, Fields};

#[proc_macro_derive(DescribeModule)]
pub fn describe_module_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let struct_name = &input.ident;

    // Collect field names/types
    let mut field_names = Vec::new();
    let mut field_types = Vec::new();
    if let Data::Struct(data_struct) = &input.data {
        if let Fields::Named(fields_named) = &data_struct.fields {
            for field in &fields_named.named {
                let name = field.ident.as_ref().unwrap();
                let ty = &field.ty;
                field_names.push(quote! { stringify!(#name) });
                field_types.push(quote! { stringify!(#ty) });
            }
        }
    }

    // Generate Verilog register declarations (simple mapping: all as reg [7:0])
    let verilog_fields: Vec<_> = field_names.iter().map(|name| {
        quote! {
            println!("  reg [7:0] {};", #name);
        }
    }).collect();

    let code_gen = quote! {
        impl #struct_name {
            pub fn to_verilog() {
                println!("module {};", stringify!(#struct_name));
                #(#verilog_fields)*
                
            }
        }
    };
    code_gen.into()
}

#[proc_macro_attribute]
pub fn analyze_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemImpl);
    let self_ty = &input.self_ty;

    let mut assignments = Vec::new();

    for item in &input.items {
        if let syn::ImplItem::Fn(ImplItemFn { sig, block, .. }) = item {
            if sig.ident == "update" {
                for stmt in &block.stmts {
                    // Look for self.<field>.set_value(...)
                    if let Stmt::Expr(Expr::MethodCall(mc), _) = stmt {
                        if let Expr::Field(field_expr) = &*mc.receiver {
                            if let Expr::Path(ref base) = *field_expr.base {
                                if base.path.is_ident("self") {
                                    if let syn::Member::Named(ident) = &field_expr.member {
                                        let field_name = ident.to_string();
                                        let method_name = mc.method.to_string();
                                        let args = mc.args.iter()
                                            .map(|arg| quote!(#arg).to_string())
                                            .collect::<Vec<_>>();
                                        let args_str = args.join(", ");
                                        // Only generate assignment for set_value
                                        if method_name == "set_value" {
                                            assignments.push(quote! {
                                                println!("  assign {} = {};", #field_name, #args_str);
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let code_gen = quote! {
        #input
        impl #self_ty {
            pub fn to_verilog_assignments() {
                #(#assignments)*
                println!("endmodule");
            }
        }
    };
    code_gen.into()
}
