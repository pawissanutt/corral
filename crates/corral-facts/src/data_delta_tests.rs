use super::*;

#[test]
fn data_delta_compares_field_shapes_at_committed_revisions() {
    let repo = Repo::new();
    let workspace = tempfile::tempdir().unwrap();
    let cache = FactsCache::new(workspace.path());
    let path = "src/model.rs";
    repo.put(
        "src/lib.rs",
        "pub mod model;\n#[derive(Serialize)] struct Holder { wire: crate::model::Wire }",
    );
    repo.put(
        path,
        "#[derive(Serialize)]\nstruct Wire {\n    stable: Vec<bool>,\n    gone: u32,\n    value: i32,\n}\n#[derive(Deserialize)] struct Deleted;\n",
    );
    repo.put(
        "custom/Cargo.toml",
        "[package]\nname='custom'\nversion='0.0.0'\n[lib]\npath='src/entry.rs'",
    );
    repo.put("custom/src/entry.rs", "mod lib; mod child;");
    repo.put(
        "custom/src/child.rs",
        "#[derive(Serialize)] pub struct Child;",
    );
    repo.put("custom/src/lib.rs", "mod child { #[derive(Serialize)] struct Child; } #[derive(Serialize)] struct Decoy { field: crate::child::Child }");
    let schema = "schema/events.schema.json";
    repo.put(schema, r##"{"title":"Event","$defs":{"A":{"type":"string"},"B":{"type":"string"},"C":{"type":"string"}},"properties":{"payload":{"oneOf":[{"$ref":"#/$defs/A"},{"$ref":"#/$defs/B"}]}}}"##);
    let before = repo.commit("2025-01-01T00:00:00Z", "before");
    repo.put(
        path,
        "\n#[derive(Serialize)]\nstruct Wire {\n    value: String,\n    added: bool,\n    stable: Vec <bool>,\n}\n#[derive(Deserialize)] struct New;\n",
    );
    repo.put(schema, r##"{"title":"Event","$defs":{"A":{"type":"string"},"B":{"type":"string"},"C":{"type":"string"}},"properties":{"payload":{"oneOf":[{"$ref":"#/$defs/A"},{"$ref":"#/$defs/C"}]}}}"##);
    let after = repo.commit("2025-01-02T00:00:00Z", "after");
    repo.put(path, "uncommitted change");
    for (revision, second) in [(&before, "B"), (&after, "C")] {
        let tree = cache.tree_facts(repo.path(), &revision.tree).unwrap();
        assert!(
            tree.data
                .edges
                .iter()
                .any(|edge| edge.from.path == "src/lib.rs"
                    && edge.from.name == "Holder"
                    && edge.field == "wire"
                    && edge.to.path == path
                    && edge.to.name == "Wire")
        );
        assert!(
            !tree.data.edges.iter().any(|edge| edge.from.name == "Decoy"),
            "an overridden conventional root must not link to itself"
        );
        let edges: Vec<_> = tree
            .data
            .edges
            .iter()
            .filter(|edge| edge.from.name == "Event" && edge.field == "payload")
            .collect();
        assert_eq!(edges.len(), 2, "{edges:#?}");
        for target in ["A", second] {
            assert!(
                edges
                    .iter()
                    .any(|edge| edge.to.name == target && edge.kind == DataEdgeKind::Has),
                "{edges:#?}"
            );
        }
    }
    let all_changes = cache.delta(repo.path(), &before, &after).unwrap().data;
    assert!(all_changes.iter().any(|row| row.name == "Event" && matches!(&row.change,
        DataChangeKind::FieldRetyped { before, after }
        if before.name == "payload" && before.ty == "#/$defs/A | #/$defs/B" && after.ty == "#/$defs/A | #/$defs/C")), "{all_changes:#?}");
    let changes: Vec<_> = all_changes
        .into_iter()
        .filter(|row| row.source == DataSource::RustSerde)
        .collect();
    use DataChangeKind::*;
    assert!(
        changes
            .iter()
            .all(|row| row.contract && row.source == DataSource::RustSerde)
    );
    assert!(
        matches!(changes.as_slice(), [removed_type, added_type, added, gone, retyped]
            if removed_type.name == "Deleted" && removed_type.change == TypeRemoved
            && added_type.name == "New" && added_type.change == TypeAdded
            && added.name == "Wire" && matches!(&added.change, FieldAdded {field} if field.name == "added" && field.line == 5)
            && gone.line == 2 && matches!(&gone.change, FieldRemoved {field} if field.name == "gone" && field.line == 4)
            && retyped.line == 3 && matches!(&retyped.change, FieldRetyped {before, after} if before.name == "value" && before.ty == "i32" && after.ty == "String")
        ),
        "{changes:#?}"
    );
}

#[test]
fn json_union_rendering_and_expansion_limits() {
    let path = "union.schema.json";
    let read = |text: &str| {
        let mut builder = crate::data::Builder::default();
        builder.read(path, Some(text));
        builder.finish([(path, "JSON")])
    };
    let facts = read(
        r##"{"title":"U","$defs":{"A":{"type":"string"},"B":{"type":"string"}},"properties":{"payload":{"type":"array","items":{"anyOf":[{"$ref":"#/$defs/A"},{"$ref":"#/$defs/B"}]}},"nullable":{"anyOf":[{"type":"null"},{"$ref":"#/$defs/A"}]},"constrained":{"oneOf":[{"type":"null","$ref":"#/$defs/A"},{"$ref":"#/$defs/B"}]}}}"##,
    );
    let shape = facts.types.iter().find(|ty| ty.name == "U").unwrap();
    assert!(
        shape
            .fields
            .iter()
            .any(|field| field.name == "payload" && field.ty == "array<(#/$defs/A | #/$defs/B)>")
    );
    assert!(
        shape
            .fields
            .iter()
            .any(|field| field.name == "nullable" && field.ty == "#/$defs/A | null")
    );
    assert!(
        shape
            .fields
            .iter()
            .any(|field| field.name == "constrained" && field.ty == "#/$defs/A | #/$defs/B")
    );
    for field in ["payload", "constrained"] {
        assert_eq!(
            facts
                .edges
                .iter()
                .filter(|edge| edge.field == field)
                .count(),
            2
        );
    }
    let long = format!(
        r#"{{"title":"U","properties":{{"p":{{"oneOf":[{}]}}}}}}"#,
        vec![r#"{"type":"string"}"#; 4000].join(",")
    );
    assert!(read(&long).unreadable_paths().contains(path));
    let many_links = format!(
        r##"{{"title":"U","properties":{{"p":{{"anyOf":[{}]}}}}}}"##,
        vec![r##"{"$ref":"#"}"##; 3500].join(",")
    );
    assert!(read(&many_links).unreadable_paths().contains(path));
}
