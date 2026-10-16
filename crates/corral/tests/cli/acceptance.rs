use super::*;
use serde_json::Value;

fn identity(root: &Path) -> Value {
    let audit: Value =
        serde_json::from_str(&ok(root, &["done", "T-001", "--check", "--json"])).unwrap();
    audit["identity"].clone()
}

fn stale_fields(root: &Path) -> Value {
    let result = corral(root, &["verdict", "T-001", "approve", "--notes", "fixture"]);
    assert!(!result.status.success());
    let stderr = String::from_utf8(result.stderr).unwrap();
    serde_json::from_str(stderr.trim().strip_prefix("error: stale review: ").unwrap()).unwrap()
}

#[test]
fn identity_is_deterministic_across_processes_and_config_layout() {
    // Given an isolated candidate and its identity from a separate CLI process.
    let ws = init_ws();
    let (store, _, _) = review_task(&ws, None);
    let first = identity(&ws.root);
    let config = std::fs::read_to_string(store.config_path()).unwrap();
    std::fs::write(
        store.config_path(),
        format!("# same policy, different layout\n{config}"),
    )
    .unwrap();
    // When another process resolves the same effective values.
    let second = identity(&ws.root);
    // Then the digests are stable, independent of process hash seeds or TOML layout.
    assert_eq!(first, second);
}

#[test]
fn same_head_input_change_reports_actual_field_and_both_digests() {
    let ws = init_ws();
    let (store, _, _) = review_task(&ws, Some(started_review("opus-rev")));
    let before = identity(&ws.root);
    std::fs::write(store.prompts_dir().join("review.md"), "fixture override").unwrap();
    let after = identity(&ws.root);
    // When a prompt input changes without moving HEAD.
    let fields = stale_fields(&ws.root);
    // Then only the changed input digest is diagnosed, never a fictitious HEAD move.
    assert_eq!(before["head"], after["head"]);
    assert_eq!(
        fields,
        serde_json::json!([{
            "field":"inputs_digest", "expected":before["inputs_digest"], "actual":after["inputs_digest"]
        }])
    );
    println!("STALE_REVIEW_FIELDS {fields}");
}

#[test]
fn edited_spec_reports_the_specification_digest_change() {
    let ws = init_ws();
    let (store, _, _) = review_task(&ws, Some(started_review("opus-rev")));
    let before = identity(&ws.root);
    store
        .owner_decision("T-001", "narrow", "Update candidate specification", "flag")
        .unwrap();
    store
        .update("T-001", |task| {
            task.body.push_str("\nAdditional required behavior.");
            Ok(())
        })
        .unwrap();
    let after = identity(&ws.root);
    let fields = stale_fields(&ws.root);
    assert_eq!(
        fields,
        serde_json::json!([{
            "field":"spec_digest", "expected":before["spec_digest"], "actual":after["spec_digest"]
        }])
    );
    println!("STALE_REVIEW_FIELDS {fields}");
}
