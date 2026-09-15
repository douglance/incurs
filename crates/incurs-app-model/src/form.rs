//! Form model lowered from a tool input schema.
//!
//! [`FormModel`] is the desktop adapter's view of one [`ToolDefinition`]'s
//! input JSON Schema. It is a presentation model only: it never validates a
//! value and never decides whether a call may proceed. Authoritative
//! validation stays in the shared command runtime, which reports failures as
//! structured field errors that [`FormState::apply_field_errors`] maps back
//! onto the originating control.
//!
//! The adapter coerces raw control text into the JSON types the Tool Contract
//! declares. That is a representation conversion, not a validation rule: a
//! number control must produce a JSON number because the contract says the
//! property is a number.
//!
//! [`ToolDefinition`]: incurs::tool::ToolDefinition

use std::collections::BTreeMap;

use serde_json::Value;

use crate::text::humanize;

/// The control used to collect one field value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    /// Free text collected as a JSON string.
    Text,
    /// A numeric value collected as text and coerced to a JSON number.
    Number {
        /// Whether the contract requires a whole number.
        integer: bool,
    },
    /// A boolean collected as an on/off switch.
    Toggle,
    /// A closed set of string values collected as a single choice.
    Select {
        /// The permitted values in schema order.
        options: Vec<String>,
    },
    /// A list of strings collected one per line.
    List,
    /// A value with no simpler control, collected as raw JSON text.
    Json,
}

impl FieldKind {
    /// Returns whether the control stores its value as free text.
    ///
    /// Toggles and selects hold their own state instead of a text buffer.
    pub fn is_text_like(&self) -> bool {
        matches!(
            self,
            FieldKind::Text | FieldKind::Number { .. } | FieldKind::List | FieldKind::Json
        )
    }
}

/// One collectable field of a tool input schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    /// The JSON property name sent to the tool catalog.
    pub name: String,
    /// A human-readable label derived from the property name.
    pub label: String,
    /// The schema description, shown as help text.
    pub help: Option<String>,
    /// The control used to collect the value.
    pub kind: FieldKind,
    /// Whether the schema lists this property as required.
    pub required: bool,
    /// The schema default, rendered as the control's initial value.
    pub default: Option<Value>,
}

/// The ordered fields of one tool input schema.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormModel {
    /// Fields in presentation order: required first, then optional.
    pub fields: Vec<FormField>,
}

impl FormModel {
    /// Lowers a tool input JSON Schema into an ordered form model.
    ///
    /// Required properties come first, in the order the schema's `required`
    /// array lists them, which preserves the authoring order of positional
    /// arguments. Optional properties follow in whatever order the schema's
    /// property map yields.
    ///
    /// A schema that declares no object properties produces an empty model,
    /// which the workbench renders as a command that takes no input.
    pub fn from_input_schema(schema: &Value) -> Self {
        let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
            return Self::default();
        };

        let required: Vec<String> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        let mut fields = Vec::with_capacity(properties.len());

        for name in &required {
            if let Some(property) = properties.get(name) {
                fields.push(build_field(name, property, schema, true));
            }
        }
        for (name, property) in properties {
            if !required.contains(name) {
                fields.push(build_field(name, property, schema, false));
            }
        }

        Self { fields }
    }

    /// Returns the field with the given property name.
    pub fn field(&self, name: &str) -> Option<&FormField> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// Returns whether the model collects no input at all.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// A per-field problem reported before or after a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldIssue {
    /// The JSON property name the problem belongs to.
    pub field: String,
    /// A message written for a person rather than an agent.
    pub message: String,
}

/// The mutable value of one control, plus any problem attached to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldValue {
    /// Text buffer for text-like controls.
    pub text: String,
    /// Switch state for [`FieldKind::Toggle`].
    pub toggle: bool,
    /// Selected option for [`FieldKind::Select`], empty when unset.
    pub choice: String,
    /// The problem currently shown beneath the control.
    pub issue: Option<String>,
}

/// The collected values for one command's form.
#[derive(Debug, Clone, Default)]
pub struct FormState {
    values: BTreeMap<String, FieldValue>,
}

impl FormState {
    /// Creates state seeded from each field's schema default.
    pub fn new(model: &FormModel) -> Self {
        let mut values = BTreeMap::new();
        for field in &model.fields {
            values.insert(field.name.clone(), seed_value(field));
        }
        Self { values }
    }

    /// Returns the current value of one field.
    pub fn value(&self, name: &str) -> Option<&FieldValue> {
        self.values.get(name)
    }

    /// Returns the mutable value of one field, inserting a default when absent.
    pub fn value_mut(&mut self, name: &str) -> &mut FieldValue {
        self.values.entry(name.to_string()).or_default()
    }

    /// Clears every field problem before a new attempt.
    pub fn clear_issues(&mut self) {
        for value in self.values.values_mut() {
            value.issue = None;
        }
    }

    /// Attaches runtime field errors to their originating controls.
    ///
    /// Returns the errors whose path matched no field, so the caller can show
    /// them where a person will still see them.
    pub fn apply_field_errors(
        &mut self,
        model: &FormModel,
        errors: &[incurs::output::FieldErrorOutput],
    ) -> Vec<String> {
        let mut unmatched = Vec::new();
        for error in errors {
            match resolve_error_field(model, &error.path) {
                Some(name) => {
                    self.value_mut(&name).issue = Some(error.message.clone());
                }
                None => unmatched.push(error.message.clone()),
            }
        }
        unmatched
    }

    /// Coerces the collected controls into flat tool-call arguments.
    ///
    /// An empty optional control is omitted so the command's own defaults and
    /// configured values still apply. Coercion problems are returned instead of
    /// being sent to the runtime as an ill-typed value.
    pub fn arguments(&self, model: &FormModel) -> Result<BTreeMap<String, Value>, Vec<FieldIssue>> {
        let mut arguments = BTreeMap::new();
        let mut issues = Vec::new();

        for field in &model.fields {
            let value = match self.values.get(&field.name) {
                Some(value) => value,
                None => continue,
            };

            match &field.kind {
                FieldKind::Toggle => {
                    arguments.insert(field.name.clone(), Value::Bool(value.toggle));
                }
                FieldKind::Select { .. } => {
                    if !value.choice.is_empty() {
                        arguments.insert(field.name.clone(), Value::String(value.choice.clone()));
                    }
                }
                FieldKind::Text => {
                    let text = value.text.trim();
                    if !text.is_empty() {
                        arguments.insert(field.name.clone(), Value::String(text.to_string()));
                    }
                }
                FieldKind::Number { integer } => {
                    let text = value.text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    match parse_number(text, *integer) {
                        Some(number) => {
                            arguments.insert(field.name.clone(), number);
                        }
                        None => issues.push(FieldIssue {
                            field: field.name.clone(),
                            message: if *integer {
                                "Enter a whole number.".to_string()
                            } else {
                                "Enter a number.".to_string()
                            },
                        }),
                    }
                }
                FieldKind::List => {
                    let items: Vec<Value> = value
                        .text
                        .split(['\n', ','])
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .map(|line| Value::String(line.to_string()))
                        .collect();
                    if !items.is_empty() {
                        arguments.insert(field.name.clone(), Value::Array(items));
                    }
                }
                FieldKind::Json => {
                    let text = value.text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Value>(text) {
                        Ok(parsed) => {
                            arguments.insert(field.name.clone(), parsed);
                        }
                        Err(error) => issues.push(FieldIssue {
                            field: field.name.clone(),
                            message: format!("This needs to be valid JSON: {error}"),
                        }),
                    }
                }
            }
        }

        if issues.is_empty() {
            Ok(arguments)
        } else {
            Err(issues)
        }
    }
}

/// Builds one field from a resolved schema property.
fn build_field(name: &str, property: &Value, root: &Value, required: bool) -> FormField {
    let resolved = resolve(property, root);
    FormField {
        name: name.to_string(),
        label: humanize(name),
        help: resolved
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        kind: field_kind(&resolved),
        required,
        default: resolved.get("default").cloned(),
    }
}

/// Chooses the control for one resolved schema property.
fn field_kind(schema: &Value) -> FieldKind {
    if let Some(options) = string_enum(schema) {
        return FieldKind::Select { options };
    }

    match schema_type(schema).as_deref() {
        Some("boolean") => FieldKind::Toggle,
        Some("integer") => FieldKind::Number { integer: true },
        Some("number") => FieldKind::Number { integer: false },
        Some("array") => {
            let items_are_scalar = schema
                .get("items")
                .map(|items| {
                    matches!(
                        schema_type(items).as_deref(),
                        Some("string") | Some("number") | Some("integer") | None
                    )
                })
                .unwrap_or(true);
            if items_are_scalar {
                FieldKind::List
            } else {
                FieldKind::Json
            }
        }
        Some("object") => FieldKind::Json,
        Some("string") => FieldKind::Text,
        _ => FieldKind::Text,
    }
}

/// Returns the schema's non-null type name, if it declares exactly one.
///
/// Handles both the `["string", "null"]` union form and a plain string.
fn schema_type(schema: &Value) -> Option<String> {
    match schema.get("type") {
        Some(Value::String(name)) => Some(name.clone()),
        Some(Value::Array(names)) => names
            .iter()
            .filter_map(Value::as_str)
            .find(|name| *name != "null")
            .map(str::to_string),
        _ => None,
    }
}

/// Returns the property's permitted string values, when it is a closed set.
fn string_enum(schema: &Value) -> Option<Vec<String>> {
    let values = schema.get("enum").and_then(Value::as_array)?;
    let options: Vec<String> = values
        .iter()
        .filter(|value| !value.is_null())
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    (!options.is_empty() && options.len() == values.iter().filter(|v| !v.is_null()).count())
        .then_some(options)
}

/// Resolves references and nullable wrappers into one effective schema.
///
/// `schemars` emits optional values as `anyOf` unions with a null branch and
/// named types as `$ref` pointers into `$defs`. Both are unwrapped here so the
/// control chooser only sees a plain property schema.
fn resolve(schema: &Value, root: &Value) -> Value {
    let mut current = schema.clone();

    for _ in 0..8 {
        if let Some(pointer) = current.get("$ref").and_then(Value::as_str)
            && let Some(target) = resolve_pointer(root, pointer)
        {
            let description = current.get("description").cloned();
            let default = current.get("default").cloned();
            current = target;
            if let Some(object) = current.as_object_mut() {
                if let Some(description) = description {
                    object.insert("description".to_string(), description);
                }
                if let Some(default) = default {
                    object.insert("default".to_string(), default);
                }
            }
            continue;
        }

        if let Some(branch) = sole_non_null_branch(&current) {
            let carried = current.clone();
            current = branch;
            if let Some(object) = current.as_object_mut() {
                for key in ["description", "default"] {
                    if !object.contains_key(key)
                        && let Some(value) = carried.get(key)
                    {
                        object.insert(key.to_string(), value.clone());
                    }
                }
            }
            continue;
        }

        break;
    }

    current
}

/// Returns the single meaningful branch of an `anyOf` or `oneOf` union.
fn sole_non_null_branch(schema: &Value) -> Option<Value> {
    let branches = schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(Value::as_array)?;
    let mut meaningful = branches
        .iter()
        .filter(|branch| schema_type(branch).as_deref() != Some("null"));
    let first = meaningful.next()?.clone();
    meaningful.next().is_none().then_some(first)
}

/// Resolves a local JSON pointer such as `#/$defs/Priority`.
fn resolve_pointer(root: &Value, pointer: &str) -> Option<Value> {
    let path = pointer.strip_prefix("#/")?;
    let mut current = root;
    for segment in path.split('/') {
        let segment = segment.replace("~1", "/").replace("~0", "~");
        current = current.get(&segment)?;
    }
    Some(current.clone())
}

/// Maps a runtime field-error path onto a form field name.
///
/// Runtime paths may be qualified, such as `options.priority`, so the last
/// segment is matched when the whole path does not name a field.
fn resolve_error_field(model: &FormModel, path: &str) -> Option<String> {
    if model.field(path).is_some() {
        return Some(path.to_string());
    }
    let leaf = path.rsplit('.').next()?;
    model.field(leaf).map(|field| field.name.clone())
}

/// Parses a numeric control's text into a JSON number.
fn parse_number(text: &str, integer: bool) -> Option<Value> {
    if integer {
        return text.parse::<i64>().ok().map(Value::from);
    }
    let parsed = text.parse::<f64>().ok()?;
    parsed.is_finite().then(|| Value::from(parsed))
}

/// Renders a property name as a label a person can read.
///
/// Produces the initial control state for one field.
fn seed_value(field: &FormField) -> FieldValue {
    let mut value = FieldValue::default();
    match (&field.kind, &field.default) {
        (FieldKind::Toggle, Some(Value::Bool(on))) => value.toggle = *on,
        (FieldKind::Select { .. }, Some(Value::String(choice))) => value.choice = choice.clone(),
        (FieldKind::List, Some(Value::Array(items))) => {
            value.text = items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ");
        }
        (FieldKind::Json, Some(default)) => {
            value.text = serde_json::to_string_pretty(default).unwrap_or_default();
        }
        (_, Some(Value::String(text))) => value.text = text.clone(),
        (_, Some(Value::Null)) | (_, None) => {}
        (_, Some(other)) => value.text = other.to_string(),
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(schema: Value) -> FormModel {
        FormModel::from_input_schema(&schema)
    }

    #[test]
    fn required_fields_keep_schema_order_and_precede_optional_fields() {
        let model = model(json!({
            "type": "object",
            "properties": {
                "zebra": {"type": "string"},
                "title": {"type": "string"},
                "alpha": {"type": "string"},
                "count": {"type": "integer"}
            },
            "required": ["title", "count"]
        }));

        // Required fields lead, in the order the schema's `required` array
        // lists them. The relative order of the optional fields follows the
        // schema's property map and is not a promise of this adapter.
        let names: Vec<&str> = model.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(&names[..2], ["title", "count"]);
        let optional: std::collections::BTreeSet<&str> = names[2..].iter().copied().collect();
        assert_eq!(
            optional,
            std::collections::BTreeSet::from(["alpha", "zebra"])
        );
        assert!(model.field("title").unwrap().required);
        assert!(!model.field("alpha").unwrap().required);
    }

    #[test]
    fn control_is_chosen_from_the_declared_type() {
        let model = model(json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "count": {"type": "integer"},
                "ratio": {"type": "number"},
                "force": {"type": "boolean"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "meta": {"type": "object"}
            }
        }));

        assert_eq!(model.field("name").unwrap().kind, FieldKind::Text);
        assert_eq!(
            model.field("count").unwrap().kind,
            FieldKind::Number { integer: true }
        );
        assert_eq!(
            model.field("ratio").unwrap().kind,
            FieldKind::Number { integer: false }
        );
        assert_eq!(model.field("force").unwrap().kind, FieldKind::Toggle);
        assert_eq!(model.field("tags").unwrap().kind, FieldKind::List);
        assert_eq!(model.field("meta").unwrap().kind, FieldKind::Json);
    }

    #[test]
    fn a_closed_string_set_becomes_a_choice() {
        let model = model(json!({
            "type": "object",
            "properties": {
                "priority": {"type": "string", "enum": ["low", "medium", "high"]}
            }
        }));

        assert_eq!(
            model.field("priority").unwrap().kind,
            FieldKind::Select {
                options: vec!["low".into(), "medium".into(), "high".into()]
            }
        );
    }

    #[test]
    fn a_referenced_definition_resolves_to_its_control() {
        let model = model(json!({
            "type": "object",
            "properties": {
                "priority": {"$ref": "#/$defs/Priority", "description": "How urgent."}
            },
            "$defs": {
                "Priority": {"type": "string", "enum": ["low", "high"]}
            }
        }));

        let field = model.field("priority").unwrap();
        assert_eq!(
            field.kind,
            FieldKind::Select {
                options: vec!["low".into(), "high".into()]
            }
        );
        assert_eq!(field.help.as_deref(), Some("How urgent."));
    }

    #[test]
    fn a_nullable_union_resolves_to_its_value_control() {
        let union = model(json!({
            "type": "object",
            "properties": {
                "limit": {"anyOf": [{"type": "integer"}, {"type": "null"}]}
            }
        }));
        assert_eq!(
            union.field("limit").unwrap().kind,
            FieldKind::Number { integer: true }
        );

        let listed = model(json!({
            "type": "object",
            "properties": {"note": {"type": ["string", "null"]}}
        }));
        assert_eq!(listed.field("note").unwrap().kind, FieldKind::Text);
    }

    #[test]
    fn labels_read_as_words_rather_than_identifiers() {
        let model = model(json!({
            "type": "object",
            "properties": {"output_dir": {"type": "string"}}
        }));
        assert_eq!(model.field("output_dir").unwrap().label, "Output dir");
    }

    #[test]
    fn a_schema_without_properties_collects_no_input() {
        assert!(model(json!({"type": "object"})).is_empty());
    }

    #[test]
    fn state_seeds_controls_from_schema_defaults() {
        let model = model(json!({
            "type": "object",
            "properties": {
                "priority": {"type": "string", "enum": ["low", "high"], "default": "high"},
                "force": {"type": "boolean", "default": true},
                "name": {"type": "string", "default": "Ada"},
                "count": {"type": "integer", "default": 3}
            }
        }));
        let state = FormState::new(&model);

        assert_eq!(state.value("priority").unwrap().choice, "high");
        assert!(state.value("force").unwrap().toggle);
        assert_eq!(state.value("name").unwrap().text, "Ada");
        assert_eq!(state.value("count").unwrap().text, "3");
    }

    #[test]
    fn arguments_coerce_each_control_to_its_contract_type() {
        let model = model(json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "count": {"type": "integer"},
                "ratio": {"type": "number"},
                "force": {"type": "boolean"},
                "tags": {"type": "array", "items": {"type": "string"}},
                "meta": {"type": "object"}
            },
            "required": ["name"]
        }));
        let mut state = FormState::new(&model);
        state.value_mut("name").text = "  Ada  ".into();
        state.value_mut("count").text = "7".into();
        state.value_mut("ratio").text = "1.5".into();
        state.value_mut("force").toggle = true;
        state.value_mut("tags").text = "one, ,  two  \n".into();
        state.value_mut("meta").text = r#"{"k":1}"#.into();

        let arguments = state.arguments(&model).expect("coercion should succeed");

        assert_eq!(arguments["name"], json!("Ada"));
        assert_eq!(arguments["count"], json!(7));
        assert_eq!(arguments["ratio"], json!(1.5));
        assert_eq!(arguments["force"], json!(true));
        assert_eq!(arguments["tags"], json!(["one", "two"]));
        assert_eq!(arguments["meta"], json!({"k": 1}));
    }

    #[test]
    fn an_empty_optional_control_is_omitted_so_command_defaults_apply() {
        let model = model(json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "count": {"type": "integer"},
                "tags": {"type": "array", "items": {"type": "string"}}
            }
        }));
        let state = FormState::new(&model);

        let arguments = state.arguments(&model).expect("coercion should succeed");

        assert!(!arguments.contains_key("name"));
        assert!(!arguments.contains_key("count"));
        assert!(!arguments.contains_key("tags"));
    }

    #[test]
    fn a_toggle_is_always_sent_so_turning_it_off_is_explicit() {
        let model = model(json!({
            "type": "object",
            "properties": {"force": {"type": "boolean", "default": true}}
        }));
        let mut state = FormState::new(&model);
        state.value_mut("force").toggle = false;

        let arguments = state.arguments(&model).expect("coercion should succeed");

        assert_eq!(arguments["force"], json!(false));
    }

    #[test]
    fn an_uncoercible_number_is_reported_instead_of_being_sent() {
        let model = model(json!({
            "type": "object",
            "properties": {"count": {"type": "integer"}}
        }));
        let mut state = FormState::new(&model);
        state.value_mut("count").text = "seven".into();

        let issues = state.arguments(&model).expect_err("coercion should fail");

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].field, "count");
        assert!(issues[0].message.contains("whole number"));
    }

    #[test]
    fn a_fractional_value_is_rejected_for_a_whole_number_field() {
        let model = model(json!({
            "type": "object",
            "properties": {"count": {"type": "integer"}}
        }));
        let mut state = FormState::new(&model);
        state.value_mut("count").text = "1.5".into();

        let issues = state.arguments(&model).expect_err("coercion should fail");

        assert_eq!(issues[0].field, "count");
    }

    #[test]
    fn invalid_json_is_reported_instead_of_being_sent() {
        let model = model(json!({
            "type": "object",
            "properties": {"meta": {"type": "object"}}
        }));
        let mut state = FormState::new(&model);
        state.value_mut("meta").text = "{not json".into();

        let issues = state.arguments(&model).expect_err("coercion should fail");

        assert_eq!(issues[0].field, "meta");
        assert!(issues[0].message.contains("valid JSON"));
    }

    #[test]
    fn runtime_field_errors_attach_to_their_control() {
        let model = model(json!({
            "type": "object",
            "properties": {"priority": {"type": "string"}}
        }));
        let mut state = FormState::new(&model);

        let unmatched = state.apply_field_errors(
            &model,
            &[incurs::output::FieldErrorOutput {
                path: "options.priority".into(),
                expected: "low|high".into(),
                received: "urgent".into(),
                message: "Expected low or high".into(),
            }],
        );

        assert!(unmatched.is_empty());
        assert_eq!(
            state.value("priority").unwrap().issue.as_deref(),
            Some("Expected low or high")
        );
    }

    #[test]
    fn a_field_error_for_an_unknown_path_is_returned_rather_than_dropped() {
        let model = model(json!({"type": "object", "properties": {}}));
        let mut state = FormState::new(&model);

        let unmatched = state.apply_field_errors(
            &model,
            &[incurs::output::FieldErrorOutput {
                path: "somewhere.else".into(),
                expected: "string".into(),
                received: "number".into(),
                message: "Bad value".into(),
            }],
        );

        assert_eq!(unmatched, vec!["Bad value".to_string()]);
    }

    #[test]
    fn clearing_issues_removes_every_attached_problem() {
        let model = model(json!({
            "type": "object",
            "properties": {"name": {"type": "string"}}
        }));
        let mut state = FormState::new(&model);
        state.value_mut("name").issue = Some("bad".into());

        state.clear_issues();

        assert!(state.value("name").unwrap().issue.is_none());
    }
}
