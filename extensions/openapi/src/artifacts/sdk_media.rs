//! Generated request body choices retain the selected media and byte representation.
use super::{pascal, rust_string, symbols::SdkSymbols, type_for_schema, unique_names};
use crate::{OpenApiResult, Operation, media::BodyCodec};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) struct MediaBody {
    pub(super) name: String,
    optional: bool,
    variants: Vec<Variant>,
}

struct Variant {
    name: String,
    media: String,
    ty: String,
    bytes: bool,
    unsupported: Option<&'static str>,
}

pub(super) fn model(
    operation: &Operation,
    schemas: &BTreeMap<String, Value>,
    symbols: &SdkSymbols,
) -> OpenApiResult<Option<MediaBody>> {
    let Some(body) = &operation.request_body else {
        return Ok(None);
    };
    let mut variants = Vec::new();
    let mut needs_enum = body.content.len() > 1;
    for (media, schema) in &body.content {
        let codec = crate::media::classify(media, schema, schemas)?;
        let base = pascal(media);
        match codec {
            BodyCodec::Binary { text_alternative } => {
                needs_enum = true;
                variants.push(Variant {
                    name: if text_alternative {
                        format!("{base}Bytes")
                    } else {
                        base.clone()
                    },
                    media: media.clone(),
                    ty: "Vec<u8>".into(),
                    bytes: true,
                    unsupported: None,
                });
                if text_alternative {
                    variants.push(Variant {
                        name: format!("{base}Text"),
                        media: media.clone(),
                        ty: "String".into(),
                        bytes: false,
                        unsupported: None,
                    });
                }
            }
            BodyCodec::JsonLines { binary: true, .. } => {
                needs_enum = true;
                variants.push(Variant {
                    name: base,
                    media: media.clone(),
                    ty: "Vec<u8>".into(),
                    bytes: true,
                    unsupported: None,
                });
            }
            BodyCodec::Unsupported(reason) => {
                let forbidden =
                    crate::schema::binding_view(schema, schemas)?.as_ref() == &Value::Bool(false);
                needs_enum |= !forbidden;
                variants.push(Variant {
                    name: base,
                    media: media.clone(),
                    ty: if forbidden { "Never" } else { "JsonValue" }.into(),
                    bytes: false,
                    unsupported: Some(reason),
                });
            }
            _ => variants.push(Variant {
                name: base,
                media: media.clone(),
                ty: type_for_schema(schema, None, schemas, symbols)?,
                bytes: false,
                unsupported: None,
            }),
        }
    }
    if !needs_enum {
        return Ok(None);
    }
    let names = unique_names(
        variants
            .iter()
            .map(|variant| variant.name.clone())
            .collect(),
        &["Missing"],
        "",
    );
    for (variant, name) in variants.iter_mut().zip(names) {
        variant.name = name;
    }
    Ok(Some(MediaBody {
        name: symbols.operation(operation).body_type.clone().unwrap(),
        optional: !body.required,
        variants,
    }))
}

pub(super) fn needs_enum(
    operation: &Operation,
    schemas: &BTreeMap<String, Value>,
) -> OpenApiResult<bool> {
    let Some(body) = &operation.request_body else {
        return Ok(false);
    };
    let mut needs_enum = body.content.len() > 1;
    for (media, schema) in &body.content {
        match crate::media::classify(media, schema, schemas)? {
            BodyCodec::Binary { .. } | BodyCodec::JsonLines { binary: true, .. } => {
                needs_enum = true;
            }
            BodyCodec::Unsupported(_) => {
                let forbidden =
                    crate::schema::binding_view(schema, schemas)?.as_ref() == &Value::Bool(false);
                needs_enum |= !forbidden;
            }
            _ => {}
        }
    }
    Ok(needs_enum)
}

impl MediaBody {
    pub(super) fn declaration(&self) -> String {
        let mut out = format!(
            "/// Select a declared request media type before supplying its body.\n#[derive(Clone, Debug, PartialEq)]\npub enum {} {{\n",
            self.name
        );
        if self.optional {
            out.push_str("    /// Omit the optional body and Content-Type.\n    Missing,\n");
        }
        for variant in &self.variants {
            let description = if let Some(reason) = variant.unsupported {
                format!(
                    "Declared {}; HTTP binding rejects this representation: {reason}.",
                    variant.media
                )
            } else {
                format!("Request body encoded as {}.", variant.media)
            };
            let ty = if variant.bytes {
                variant.ty.clone()
            } else {
                format!("Field<{}>", variant.ty)
            };
            out.push_str(&format!(
                "    /// {description}\n    {}({ty}),\n",
                variant.name
            ));
        }
        out.push_str("}\n\n");
        out
    }

    pub(super) fn binding(&self) -> String {
        let mut out =
            String::from("        let (body, body_bytes, media_type) = match self.body {\n");
        if self.optional {
            out.push_str(&format!(
                "            {}::Missing => (Field::Missing, None, Field::Missing),\n",
                self.name
            ));
        }
        for variant in &self.variants {
            let media = rust_string(&variant.media);
            if variant.ty == "Never" {
                out.push_str(&format!(
                    "            {}::{}(_value) => (Field::Value(JsonValue::Invalid), None, Field::Value({media}.to_string())),\n",
                    self.name, variant.name
                ));
            } else if variant.bytes {
                out.push_str(&format!("            {}::{}(bytes) => (Field::Missing, Some(bytes), Field::Value({media}.to_string())),\n",self.name,variant.name));
            } else {
                out.push_str(&format!("            {}::{}(value) => {{\n                let body = value.into_json_field();\n                let body = if matches!(body, Field::Missing) {{ Field::Value(JsonValue::Invalid) }} else {{ body }};\n                (body, None, Field::Value({media}.to_string()))\n            }},\n",self.name,variant.name));
            }
        }
        out.push_str("        };\n");
        out
    }
}
