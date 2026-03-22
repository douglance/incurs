//! Shared utilities for the incur derive macros.
//!
//! Provides helpers for extracting doc comments, parsing `#[incur(...)]` attributes,
//! mapping Rust types to `FieldType` variants, and string case conversion.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Attribute, Expr, GenericArgument, Lit, Meta, PathArguments, Type};

/// Extracts the concatenated doc-comment text from a slice of attributes.
///
/// Returns `None` if no `#[doc = "..."]` attributes are present.
pub fn doc_description(attrs: &[Attribute]) -> Option<String> {
    let lines: Vec<String> = attrs
        .iter()
        .filter_map(|attr| {
            if !attr.path().is_ident("doc") {
                return None;
            }
            if let Meta::NameValue(meta) = &attr.meta
                && let Expr::Lit(expr_lit) = &meta.value
                && let Lit::Str(lit_str) = &expr_lit.lit
            {
                return Some(lit_str.value().trim().to_string());
            }
            None
        })
        .collect();

    if lines.is_empty() {
        None
    } else {
        Some(lines.join(" "))
    }
}

/// Returns `true` if the type is `Option<T>`.
pub fn is_option_type(ty: &Type) -> bool {
    extract_inner_type(ty, "Option").is_some()
}

/// Returns `true` if the type is `Vec<T>`.
pub fn is_vec_type(ty: &Type) -> bool {
    extract_inner_type(ty, "Vec").is_some()
}

/// Extracts the inner type `T` from `Wrapper<T>` (e.g. `Option<String>` -> `String`).
fn extract_inner_type<'a>(ty: &'a Type, wrapper: &str) -> Option<&'a Type> {
    if let Type::Path(type_path) = ty {
        let segment = type_path.path.segments.last()?;
        if segment.ident != wrapper {
            return None;
        }
        if let PathArguments::AngleBracketed(args) = &segment.arguments
            && let Some(GenericArgument::Type(inner)) = args.args.first()
        {
            return Some(inner);
        }
    }
    None
}

/// Generates a `quote` token stream that constructs the appropriate `FieldType` variant
/// for the given Rust type.
pub fn field_type_tokens(ty: &Type) -> TokenStream2 {
    // Unwrap Option<T> to get the inner type for field_type determination.
    let effective_ty = extract_inner_type(ty, "Option").unwrap_or(ty);

    // Check for Vec<T>
    if let Some(inner) = extract_inner_type(effective_ty, "Vec") {
        let inner_ft = scalar_field_type(inner);
        return quote! { ::incur::schema::FieldType::Array(Box::new(#inner_ft)) };
    }

    scalar_field_type(effective_ty)
}

/// Maps a scalar (non-Vec, non-Option) Rust type to a `FieldType` token stream.
fn scalar_field_type(ty: &Type) -> TokenStream2 {
    let type_name = type_ident_string(ty);
    match type_name.as_deref() {
        Some("String") | Some("str") | Some("PathBuf") => {
            quote! { ::incur::schema::FieldType::String }
        }
        Some("bool") => quote! { ::incur::schema::FieldType::Boolean },
        Some("u8") | Some("u16") | Some("u32") | Some("u64") | Some("u128") | Some("usize")
        | Some("i8") | Some("i16") | Some("i32") | Some("i64") | Some("i128")
        | Some("isize") | Some("f32") | Some("f64") => {
            quote! { ::incur::schema::FieldType::Number }
        }
        _ => quote! { ::incur::schema::FieldType::Value },
    }
}

/// Extracts the final segment identifier of a type path as a string.
fn type_ident_string(ty: &Type) -> Option<String> {
    if let Type::Path(type_path) = ty {
        type_path
            .path
            .segments
            .last()
            .map(|seg| seg.ident.to_string())
    } else {
        None
    }
}

/// Converts a Rust `snake_case` identifier to CLI `kebab-case`.
pub fn snake_to_kebab(name: &str) -> String {
    name.replace('_', "-")
}

/// Parsed `#[incur(...)]` attributes for a single field.
#[derive(Default, Debug)]
pub struct IncurAttr {
    /// Short alias character (e.g. `alias = "n"` -> Some('n')).
    pub alias: Option<char>,
    /// Default value expression as a string literal or number literal.
    pub default: Option<Lit>,
    /// Whether the field is a count flag (`#[incur(count)]`).
    pub count: bool,
    /// Whether the field is deprecated (`#[incur(deprecated)]`).
    pub deprecated: bool,
    /// Environment variable name (`#[incur(env = "VAR_NAME")]`).
    pub env_name: Option<String>,
}

/// Parses all `#[incur(...)]` attributes on a field into an `IncurAttr`.
pub fn parse_incur_attrs(attrs: &[Attribute]) -> IncurAttr {
    let mut result = IncurAttr::default();

    for attr in attrs {
        if !attr.path().is_ident("incur") {
            continue;
        }
        // Parse the comma-separated list inside #[incur(...)].
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("alias") {
                let value = meta.value()?;
                let lit: Lit = value.parse()?;
                if let Lit::Str(s) = &lit {
                    let alias_str = s.value();
                    if let Some(ch) = alias_str.chars().next() {
                        result.alias = Some(ch);
                    }
                }
            } else if meta.path.is_ident("default") {
                let value = meta.value()?;
                let lit: Lit = value.parse()?;
                result.default = Some(lit);
            } else if meta.path.is_ident("count") {
                result.count = true;
            } else if meta.path.is_ident("deprecated") {
                result.deprecated = true;
            } else if meta.path.is_ident("env") {
                let value = meta.value()?;
                let lit: Lit = value.parse()?;
                if let Lit::Str(s) = &lit {
                    result.env_name = Some(s.value());
                }
            }
            Ok(())
        });
    }

    result
}

/// Converts a `syn::Lit` default value into a `quote` token stream that produces
/// a `serde_json::Value`.
pub fn default_value_tokens(lit: &Lit) -> TokenStream2 {
    match lit {
        Lit::Str(s) => {
            let val = s.value();
            quote! { Some(serde_json::Value::String(#val.to_string())) }
        }
        Lit::Int(i) => {
            let val = i.base10_digits().to_string();
            // Parse as i64 at runtime for the JSON number.
            quote! {
                Some(serde_json::json!(#val.parse::<i64>().unwrap_or(0)))
            }
        }
        Lit::Float(f) => {
            let val = f.base10_digits().to_string();
            quote! {
                Some(serde_json::json!(#val.parse::<f64>().unwrap_or(0.0)))
            }
        }
        Lit::Bool(b) => {
            let val = b.value();
            quote! { Some(serde_json::Value::Bool(#val)) }
        }
        _ => quote! { None },
    }
}
