//! `#[derive(IncurArgs)]` — generates `IncurSchema` for positional arguments.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

use crate::common::{doc_description, field_type_tokens, is_option_type, snake_to_kebab};

pub fn derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new_spanned(
                    &input.ident,
                    "IncurArgs can only be derived for structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                &input.ident,
                "IncurArgs can only be derived for structs",
            )
            .to_compile_error()
            .into();
        }
    };

    let field_meta_entries: Vec<TokenStream2> = fields
        .iter()
        .map(|f| {
            let field_name = f.ident.as_ref().unwrap();
            let field_name_str = field_name.to_string();
            let cli_name_str = snake_to_kebab(&field_name_str);
            let description = doc_description(&f.attrs);
            let is_optional = is_option_type(&f.ty);
            let required = !is_optional;
            let field_type = field_type_tokens(&f.ty);

            let desc_tokens = match description {
                Some(desc) => quote! { Some(#desc) },
                None => quote! { None },
            };

            quote! {
                ::incur::schema::FieldMeta {
                    name: #field_name_str,
                    cli_name: #cli_name_str.to_string(),
                    description: #desc_tokens,
                    field_type: #field_type,
                    required: #required,
                    default: None,
                    alias: None,
                    deprecated: false,
                    env_name: None,
                }
            }
        })
        .collect();

    let expanded = quote! {
        impl ::incur::schema::IncurSchema for #name {
            fn fields() -> Vec<::incur::schema::FieldMeta> {
                vec![
                    #( #field_meta_entries ),*
                ]
            }

            fn from_raw(
                raw: &std::collections::BTreeMap<String, serde_json::Value>,
            ) -> std::result::Result<Self, ::incur::errors::ValidationError> {
                let obj = serde_json::Value::Object(
                    raw.iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<serde_json::Map<String, serde_json::Value>>(),
                );
                serde_json::from_value::<Self>(obj).map_err(|e| {
                    ::incur::errors::ValidationError {
                        message: format!("Failed to parse args: {}", e),
                        field_errors: Vec::new(),
                        cause: Some(Box::new(e)),
                    }
                })
            }
        }
    };

    TokenStream::from(expanded)
}
