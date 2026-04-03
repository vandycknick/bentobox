use std::fs;
use std::path::PathBuf;

use openapiv3::OpenAPI;
use progenitor::{GenerationSettings, InterfaceStyle};
use serde_json::Value;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let spec_path = manifest_dir.join("spec/cloud-hypervisor-openapi.yaml");
    let output_path = PathBuf::from(std::env::var("OUT_DIR")?).join("cloud_hypervisor_api.rs");

    println!("cargo:rerun-if-changed={}", spec_path.display());

    let raw_spec = fs::read_to_string(&spec_path)?;
    let spec_yaml: serde_yaml::Value = serde_yaml::from_str(&raw_spec)?;
    let mut spec_json: Value = serde_json::to_value(spec_yaml)?;
    sanitize_spec(&mut spec_json);

    let spec: OpenAPI = serde_json::from_value(spec_json)?;
    let mut settings = GenerationSettings::default();
    settings.with_interface(InterfaceStyle::Builder);
    let mut generator = progenitor::Generator::new(&settings);
    let tokens = generator.generate_tokens(&spec)?;
    let syntax = syn::parse2(tokens)?;
    let formatted = prettyplease::unparse(&syntax);

    fs::write(output_path, formatted)?;
    Ok(())
}

fn sanitize_spec(value: &mut Value) {
    assign_missing_operation_ids(value);
    normalize_nonstandard_integer_types(value);
    strip_unknown_integer_formats(value);
    remove_conflicting_empty_success_responses(value);
    fix_generic_vhost_user_config(value);
}

fn assign_missing_operation_ids(value: &mut Value) {
    let Some(paths) = value.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };

    for (path, path_item) in paths {
        let Some(operations) = path_item.as_object_mut() else {
            continue;
        };

        for (method, operation) in operations {
            let Some(operation) = operation.as_object_mut() else {
                continue;
            };

            if operation.contains_key("operationId") {
                continue;
            }

            operation.insert(
                "operationId".to_string(),
                Value::String(default_operation_id(method, path)),
            );
        }
    }
}

fn default_operation_id(method: &str, path: &str) -> String {
    let mut operation_id = String::new();
    operation_id.push_str(method);

    for segment in path.split('/') {
        if segment.is_empty() {
            continue;
        }

        let mut upper_next = true;
        for ch in segment.chars() {
            if ch.is_ascii_alphanumeric() {
                if upper_next {
                    operation_id.push(ch.to_ascii_uppercase());
                    upper_next = false;
                } else {
                    operation_id.push(ch);
                }
            } else {
                upper_next = true;
            }
        }
    }

    operation_id
}

fn normalize_nonstandard_integer_types(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(ty)) = map.get_mut("type") {
                if matches!(ty.as_str(), "uint8" | "uint16" | "uint32" | "int8") {
                    *ty = "integer".to_string();
                }
            }

            for child in map.values_mut() {
                normalize_nonstandard_integer_types(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_nonstandard_integer_types(item);
            }
        }
        _ => {}
    }
}

fn strip_unknown_integer_formats(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(ty)) = map.get("type") {
                if ty == "integer" {
                    let remove_format = matches!(
                        map.get("format").and_then(Value::as_str),
                        Some("uint8" | "uint16" | "uint32" | "int8")
                    );
                    if remove_format {
                        map.remove("format");
                    }
                }
            }

            for child in map.values_mut() {
                strip_unknown_integer_formats(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_unknown_integer_formats(item);
            }
        }
        _ => {}
    }
}

fn remove_conflicting_empty_success_responses(value: &mut Value) {
    let Some(paths) = value.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };

    for path_item in paths.values_mut() {
        let Some(operations) = path_item.as_object_mut() else {
            continue;
        };

        for operation in operations.values_mut() {
            let Some(responses) = operation
                .as_object_mut()
                .and_then(|operation| operation.get_mut("responses"))
                .and_then(Value::as_object_mut)
            else {
                continue;
            };

            if responses.contains_key("200") && responses.contains_key("204") {
                responses.remove("204");
            }
        }
    }
}

fn fix_generic_vhost_user_config(value: &mut Value) {
    let Some(schema) = value
        .get_mut("components")
        .and_then(|components| components.get_mut("schemas"))
        .and_then(|schemas| schemas.get_mut("GenericVhostUserConfig"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };

    let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) else {
        return;
    };

    required.retain(|entry| matches!(entry.as_str(), Some("socket" | "virtio_id")));
}
