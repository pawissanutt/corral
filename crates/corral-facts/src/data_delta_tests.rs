use super::*;

#[test]
fn data_delta_compares_field_shapes_at_committed_revisions() {
    let repo = Repo::new();
    let workspace = tempfile::tempdir().unwrap();
    let cache = FactsCache::new(workspace.path());
    let path = "src/model.rs";
    repo.put(
        path,
        "#[derive(Serialize)]\nstruct Wire {\n    stable: Vec<bool>,\n    gone: u32,\n    value: i32,\n}\n#[derive(Deserialize)] struct Deleted;\n",
    );
    let before = repo.commit("2025-01-01T00:00:00Z", "before");
    repo.put(
        path,
        "\n#[derive(Serialize)]\nstruct Wire {\n    value: String,\n    added: bool,\n    stable: Vec <bool>,\n}\n#[derive(Deserialize)] struct New;\n",
    );
    let after = repo.commit("2025-01-02T00:00:00Z", "after");
    repo.put(path, "uncommitted change");
    let changes = cache.delta(repo.path(), &before, &after).unwrap().data;
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
