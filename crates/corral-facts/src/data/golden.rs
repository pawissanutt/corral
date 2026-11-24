use serde_json::json;

use super::Builder;

#[test]
fn built_in_data_facts_match_golden() {
    let inputs = [
        ("wire/model.rs", "Rust", include_str!("fixtures/golden.rs")),
        ("sql/model.sql", "SQL", include_str!("fixtures/golden.sql")),
        (
            "schema/model.schema.json",
            "JSON",
            include_str!("fixtures/golden.schema.json"),
        ),
    ];
    let mut builder = Builder::default();
    for (path, _, text) in inputs {
        builder.read(path, Some(text));
    }
    let data = builder.finish(inputs.iter().map(|(path, lang, _)| (*path, *lang)));
    let actual = json!({"types": data.types, "edges": data.edges, "coverage": data.coverage});
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/golden.expected.json")).unwrap();
    assert_eq!(
        actual,
        expected,
        "{}",
        serde_json::to_string_pretty(&actual).unwrap()
    );
}

#[test]
fn json_links_use_pointers_instead_of_colliding_display_names() {
    let mut builder = Builder::default();
    builder.read(
        "user.schema.json",
        Some(
            r##"{
       "title":"User",
       "$defs":{"User":{"type":"string"}},
       "definitions":{"User":{"type":"integer"}},
       "properties":{
           "nested":{"$ref":"#/$defs/User"},
           "legacy":{"$ref":"#/definitions/User"},
           "elsewhere":{"$ref":"other.schema.json"}
       }
    }"##,
        ),
    );
    builder.read(
        "other.schema.json",
        Some(r#"{"title":"Other","type":"object"}"#),
    );
    let data = builder.finish([("user.schema.json", "JSON"), ("other.schema.json", "JSON")]);
    assert_eq!(data.types.len(), 4);
    assert_eq!(data.edges.len(), 3);
    for (field, name, path) in [
        ("legacy", "#/definitions/User", "user.schema.json"),
        ("nested", "#/$defs/User", "user.schema.json"),
        ("elsewhere", "Other", "other.schema.json"),
    ] {
        assert!(data.edges.iter().any(|edge| edge.from.name == "User"
            && edge.field == field
            && edge.to.name == name
            && edge.to.path == path));
    }
}

#[test]
fn oversize_names_make_the_source_unreadable_before_edge_expansion() {
    let mut builder = Builder::default();
    let schema = format!(
        r#"{{"title":"{}","properties":{{"name":{{"type":"string"}}}}}}"#,
        "n".repeat(super::MAX_NAME_BYTES + 1)
    );
    builder.read("long.schema.json", Some(&schema));
    let data = builder.finish([("long.schema.json", "JSON")]);
    assert!(data.types.is_empty());
    assert!(data.unreadable_paths().contains("long.schema.json"));
    assert!(data.coverage["JSON"].is_empty());
}

#[test]
fn json_delta_distinguishes_a_root_from_a_definition_with_the_same_name() {
    let path = "user.schema.json";
    let read = |text| {
        let mut builder = Builder::default();
        builder.read(path, Some(text));
        builder.finish([(path, "JSON")])
    };
    let before = read(r#"{"$defs":{"User":{"properties":{"id":{"type":"integer"}}}}}"#);
    let after = read(r#"{"title":"User","properties":{"id":{"type":"integer"}}}"#);
    let delta = crate::compare_data(&before, &after);
    assert_eq!(delta.len(), 2);
    use crate::DataChangeKind::{TypeAdded, TypeRemoved};
    assert!(delta.iter().all(|row| row.name == "User" && row.contract));
    assert!(delta.iter().any(|row| row.change == TypeAdded));
    assert!(delta.iter().any(|row| row.change == TypeRemoved));
}

#[test]
fn tree_expansion_budget_records_remaining_sources_as_unreadable() {
    let mut builder = Builder {
        facts: Default::default(),
        remaining: 200,
    };
    let source = r#"{"title":"User","type":"object"}"#;
    builder.read("a.schema.json", Some(source));
    builder.read("b.schema.json", Some(source));
    let data = builder.finish([("a.schema.json", "JSON"), ("b.schema.json", "JSON")]);
    assert_eq!(data.types.len(), 1);
    assert_eq!(data.types[0].path, "a.schema.json");
    assert!(data.unreadable_paths().contains("b.schema.json"));
}

#[test]
fn rejects_both_group_and_ungrouped_rust_recursion_before_syn() {
    for ty in [
        format!("{}u8{}", "(".repeat(25), ")".repeat(25)),
        format!("{}u8", "&".repeat(97)),
    ] {
        let source = format!("#[derive(Serialize)] struct Wire({ty});");
        assert!(super::rust::read("wire.rs", &source).is_none());
    }
}
