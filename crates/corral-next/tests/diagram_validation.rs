use std::{fs, process::Command};

use serde_json::{Value, json};

#[test]
fn validates_a_local_diagram_without_a_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = include_bytes!("../../../docs/examples/submit-to-land.diagram.json");
    let recipe = dir.path().join("recipe.json");
    fs::write(&recipe, fixture).unwrap();
    let validate = || {
        Command::new(env!("CARGO_BIN_EXE_corral-next"))
            .current_dir(dir.path())
            // Discovery would fail on this root hint; offline validation must ignore it.
            .env("CORRAL_NEXT_ROOT", dir.path().join("missing-workspace"))
            .args(["artifact", "validate-diagram", "recipe.json", "--json"])
            .output()
            .unwrap()
    };

    let output = validate();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result, json!({"renderable": true, "diagnostics": []}));

    let mut invalid: Value = serde_json::from_slice(fixture).unwrap();
    invalid["version"] = json!(999);
    fs::write(&recipe, serde_json::to_vec(&invalid).unwrap()).unwrap();
    let output = validate();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result.as_object().unwrap().len(), 2);
    assert_eq!(result["renderable"], false);
    let diagnostics = result["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "unsupported_version");
    assert_eq!(diagnostics[0]["at"], "/version");
    assert!(!diagnostics[0]["message"].as_str().unwrap().is_empty());
}
