use serde_json::json;

use super::Builder;

#[test]
fn built_in_data_facts_match_golden() {
    let inputs = [
        (
            "other/src/lib.rs",
            "Rust",
            "mod child { #[derive(Serialize)] struct Address; }",
        ),
        (
            "wire/src/lib.rs",
            "Rust",
            include_str!("fixtures/golden.rs"),
        ),
        (
            "wire/src/child.rs",
            "Rust",
            include_str!("fixtures/golden-child.rs"),
        ),
        (
            "wire/src/nested/src/lib.rs",
            "Rust",
            "mod child { #[derive(Serialize)] struct Address; } #[derive(Serialize)] struct Nested { wire: crate::child::Address }",
        ),
        // Similar-looking files do not enter the crate without a `mod` declaration.
        (
            "wire/src/orphan.rs",
            "Rust",
            include_str!("fixtures/golden-child.rs"),
        ),
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
        remaining: 200,
        ..Builder::default()
    };
    let source = r#"{"title":"User","type":"object"}"#;
    builder.read("a.schema.json", Some(source));
    builder.read("b.schema.json", Some(source));
    let data = builder.finish([("a.schema.json", "JSON"), ("b.schema.json", "JSON")]);
    assert_eq!(data.types.len(), 1);
    assert_eq!(data.types[0].path, "a.schema.json");
    assert!(data.unreadable_paths().contains("b.schema.json"));

    // Only the library mount fits. Its partial layout must not turn a path
    // that is ambiguous between the library and binary into a false edge.
    let mut builder = Builder {
        layout_remaining: 200,
        ..Builder::default()
    };
    let roots = "mod shared; mod target { #[derive(Serialize)] struct Wire; }";
    builder.read("src/lib.rs", Some(roots));
    builder.read("src/main.rs", Some(roots));
    builder.read(
        "src/shared.rs",
        Some("#[derive(Serialize)] struct Shared { field: crate::target::Wire }"),
    );
    let data = builder.finish([
        ("src/lib.rs", "Rust"),
        ("src/main.rs", "Rust"),
        ("src/shared.rs", "Rust"),
    ]);
    assert_eq!(data.types.len(), 3);
    assert!(data.unreadable_paths().is_empty());
    assert!(data.edges.is_empty(), "{:?}", data.edges);
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
