use std::fs;
use std::path::PathBuf;

use openapiv3::OpenAPI;
use progenitor::{GenerationSettings, InterfaceStyle};
use serde_json::Value;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let spec_path = manifest_dir.join("spec/firecracker-openapi.yaml");
    let output_path = PathBuf::from(std::env::var("OUT_DIR")?).join("firecracker_api.rs");

    println!("cargo:rerun-if-changed={}", spec_path.display());

    let raw_spec = fs::read_to_string(&spec_path)?;
    let spec_yaml: serde_yaml::Value = serde_yaml::from_str(&raw_spec)?;
    let mut spec_json: Value = serde_json::to_value(spec_yaml)?;
    remove_default_responses(&mut spec_json);

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

fn remove_default_responses(value: &mut Value) {
    let Some(paths) = value.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };

    for path_item in paths.values_mut() {
        let Some(operations) = path_item.as_object_mut() else {
            continue;
        };

        for operation in operations.values_mut() {
            let Some(responses) = operation
                .get_mut("responses")
                .and_then(Value::as_object_mut)
            else {
                continue;
            };

            responses.remove("default");
        }
    }
}
