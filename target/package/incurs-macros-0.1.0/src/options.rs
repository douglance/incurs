//! `#[derive(IncurOptions)]` — generates `IncurSchema` for named options/flags.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

use crate::common::{
    default_value_tokens, doc_description, field_type_tokens, is_option_type, is_vec_type,
    parse_incur_attrs, snake_to_kebab,
};

pub fn derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                return syn::Error::new_spanned(
                    &input.ident,
                    "IncurOptions can only be derived for structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                &input.ident,
                "IncurOptions can only be derived for structs",
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
            let attrs = parse_incur_attrs(&f.attrs);

            // A field is required if it is not Option<T>, not Vec<T>, not bool,
            // has no default, and is not a count flag.
            let is_optional = is_option_type(&f.ty);
            let is_vec = is_vec_type(&f.ty);
            let is_bool = type_is_bool(&f.ty);
            let has_default = attrs.default.is_some();
            let required = !is_optional && !is_vec && !is_bool && !has_default && !attrs.count;

            // If the field is marked as `count`, override the field type.
            let field_type = if attrs.count {
                quote! { ::incurs::schema::FieldType::Count }
            } else {
                field_type_tokens(&f.ty)
            };

            let desc_tokens = match description {
                Some(desc) => quote! { Some(#desc) },
                None => quote! { None },
            };

            let default_tokens = match &attrs.default {
                Some(lit) => default_value_tokens(lit),
                None => quote! { None },
            };

            let alias_tokens = match attrs.alias {
                Some(ch) => quote! { Some(#ch) },
                None => quote! { None },
            };

            let deprecated = attrs.deprecated;

            quote! {
                ::incurs::schema::FieldMeta {
                    name: #field_name_str,
                    cli_name: #cli_name_str.to_string(),
                    description: #desc_tokens,
                    field_type: #field_type,
                    required: #required,
                    default: #default_tokens,
                    alias: #alias_tokens,
                    deprecated: #deprecated,
                    env_name: None,
                }
            }
        })
        .collect();

    let expanded = quote! {
        impl ::incurs::schema::IncurSchema for #name {
            fn fields() -> Vec<::incurs::schema::FieldMeta> {
                vec![
                    #( #field_meta_entries ),*
                ]
            }

            fn from_raw(
                raw: &std::collections::BTreeMap<String, serde_json::Value>,
            ) -> std::result::Result<Self, ::incurs::errors::ValidationError> {
                let obj = serde_json::Value::Object(
                    raw.iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<serde_json::Map<String, serde_json::Value>>(),
                );
                serde_json::from_value::<Self>(obj).map_err(|e| {
                    ::incurs::errors::ValidationError {
                        message: format!("Failed to parse options: {}", e),
                        field_errors: Vec::new(),
                        cause: Some(Box::new(e)),
                    }
                })
            }
        }
    };

    TokenStream::from(expanded)
}

/// Returns `true` if the type's last path segment is `bool`.
fn type_is_bool(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty
        && let Some(segment) = type_path.path.segments.last()
    {
        return segment.ident == "bool";
    }
    false
}
