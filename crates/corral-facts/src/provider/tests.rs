use super::*;
use crate::{Coverage, FileChange, FileFact, ModuleId, TreeFacts};

const DONE: &str = r#"{"type":"done","provided":[],"skipped":{},"omitted":{}}"#;
const DEFINITION: &str = r#"{"type":"definition","path":"src.rs","name":"f","line":1,"end_line":1,"kind":"fn","extra":true}"#;

#[test]
fn rejects_entire_reply_without_exactly_one_final_done_or_with_a_malformed_row() {
    let valid = format!("{DEFINITION}\r\n{DONE}\r\n");
    assert!(Reply::parse(valid.as_bytes(), &[]).is_ok());

    for output in [
        DEFINITION.to_owned(),
        format!("{DONE}\n{DONE}"),
        format!("{DONE}\n{DEFINITION}"),
        format!("{DEFINITION}\n\n{DONE}"),
        format!("{DEFINITION}\n{DONE}\n\n"),
        format!("{DEFINITION}\n{{\"type\":\"done\",\"provided\":[],\"skipped\":{{}}}}"),
        format!("{DEFINITION}\n[\"done\",[],{{}},{{}}]"),
    ] {
        assert!(Reply::parse(output.as_bytes(), &[]).is_err(), "{output}");
    }
    assert!(Reply::parse(b"\xff", &[]).is_err());

    // The malformed rows are unrequested. None can be ignored by coverage filtering.
    for malformed in [
        r#"{"type":"file","path":"src.rs","read":"partial"}"#,
        r#"{"type":"file","path":"src.rs","read":{"complete":null}}"#,
        r#"{"type":0,"path":"src.rs","read":"complete"}"#,
        r#"{"type":"definition","path":"src.rs","name":"f","line":2,"end_line":1,"kind":"fn"}"#,
        r#"{"type":"reference","from":"src.rs","to":"other.rs","kind":"call","count":0}"#,
        r#"{"type":"entry","path":"src.rs","line":0,"kind":"command","name":"serve"}"#,
        r#"{"type":"entry","path":"src.rs","line":1,"kind":"handler","name":"serve"}"#,
        r#"{"type":"entry","path":"src.rs","line":1,"kind":{"command":null},"name":"serve"}"#,
        r#"{"type":"entry","path":"src.rs","line":1,"kind":"command"}"#,
        r#"{"type":"type","path":"src.rs","name":"Wire","line":1,"kind":"record","fields":[{"name":"a","type":"bool","line":0}]}"#,
        r#"{"type":"change","change":"removed","fact":"reference","path":"src.rs","name":"f","line":1,"to_path":"other.rs"}"#,
        r#"{"type":"change","change":"removed","fact":0,"path":"src.rs","name":"f","line":1}"#,
        r#"{"type":"dependent","path":"src.rs","name":"g","line":1}"#,
        r#"{"type":"dependent","path":"src.rs","name":"g","line":1,"of":"f","depth":0}"#,
        r#"{"type":"test","path":"src.rs","name":1}"#,
        r#"{"type":"package","path":"src.rs"}"#,
        r#"{} {}"#,
    ] {
        let output = format!("{DEFINITION}\n{malformed}\n{DONE}");
        let error = Reply::parse(output.as_bytes(), &[]).unwrap_err();
        assert!(error.to_string().contains("line 2"), "{error:#}");
    }
}

#[test]
fn delta_filters_paths_against_the_right_tree_before_considering_coverage() {
    let before = tree(&["gone.rs", "common.rs"]);
    let after = tree(&["common.rs", "new.rs"]);
    let files = vec![FileChange {
        path: "new.rs".into(),
        added: 1,
        removed: 0,
        binary: false,
    }];
    let mut delta = crate::delta::compare(&before, &after, files.clone(), 0);
    let output = br#"{"type":"change","change":"removed","fact":"reference","path":"gone.rs","name":"gone::f","line":1,"to_path":"common.rs","to_name":"common::f"}
{"type":"change","change":"removed","fact":"reference","path":"common.rs","name":"common::g","line":1,"to_path":"new.rs","to_name":"f"}
{"type":"change","change":"added","fact":"definition","path":"new.rs","name":"f","line":1}
{"type":"change","change":"changed","fact":"definition","path":"gone.rs","name":"gone::g","line":1}
{"type":"dependent","path":"gone.rs","name":"gone::h","line":1,"of":"gone::f"}
{"type":"test","path":"common.rs","name":"passes"}
{"type":"test","path":"gone.rs","name":"gone::test"}
{"type":"reference","from":"gone.rs","to":"missing.rs","kind":"call","count":1}
{"type":"done","provided":["dependents","references","changes"],"skipped":{"unsupported":1},"omitted":{"limit":2}}"#;
    let reply = Reply::parse(output, &["changes", "dependents", "tests"]).unwrap();
    let cached = serde_json::to_vec(&reply).unwrap();
    let reply: Reply = serde_json::from_slice(&cached).unwrap();
    let coverage = reply.merge_delta(&mut delta, &before, &after);

    assert_eq!(delta.files, files);
    assert_eq!(coverage.provided, ["changes", "dependents"]);
    assert_eq!(coverage.not_provided, ["tests"]);
    assert_eq!(coverage.dropped_unknown_files, 5);
    assert_eq!(coverage.skipped["unsupported"], 1);
    assert_eq!(coverage.omitted["limit"], 2);
    let changes = delta.changes.unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].path, "gone.rs");
    assert_eq!(changes[0].change, ChangeKind::Removed);
    assert_eq!(
        changes[0].fact,
        ChangedFact::Reference {
            to_path: "common.rs".into(),
            to_name: "common::f".into(),
        }
    );
    assert_eq!(changes[1].path, "new.rs");
    assert_eq!(changes[1].fact, ChangedFact::Definition);
    assert_eq!(delta.dependents, Some(vec![]));
    assert_eq!(delta.tests, None);
}

#[test]
fn data_rows_replace_only_their_files_and_relink_against_the_merged_types() {
    use crate::DataSource;
    let mut tree = tree(&["src.rs", "keep.rs", "storage.sql"]);
    tree.files[2].language = "SQL".into();
    let mut builder = crate::data::Builder::default();
    builder.read("src.rs", Some("#[derive(Serialize)] struct Old;"));
    builder.read(
        "keep.rs",
        Some("#[derive(Serialize)] struct Kept { child: Wire }"),
    );
    builder.read(
        "storage.sql",
        Some(r#"CREATE TABLE stored (wire INT REFERENCES "Wire"(id));"#),
    );
    tree.data = builder.finish([
        ("src.rs", "rust"),
        ("keep.rs", "rust"),
        ("storage.sql", "SQL"),
    ]);
    assert!(tree.data.edges.is_empty());
    let answer = br#"{"type":"type","path":"src.rs","name":"Wire","line":1,"kind":"message","fields":[{"name":"v","type":"String","line":2}]}
{"type":"type","path":"not-in-tree.rs","name":"Stra","line":1,"kind":"record","fields":[]}
{"type":"done","provided":["types"],"skipped":{},"omitted":{}}"#;
    let ignored = Reply::parse(answer, &[]).unwrap().snapshot_data(&tree).0;
    assert_eq!(ignored, tree.data);
    let reply = Reply::parse(answer, &["types"]).unwrap();
    let cached = serde_json::to_vec(&reply).unwrap();
    let reply: Reply = serde_json::from_slice(&cached).unwrap();
    let coverage = reply.merge_snapshot(&mut tree);
    assert_eq!(coverage.dropped_unknown_files, 1);
    assert_eq!(
        tree.data
            .types
            .iter()
            .map(|ty| ty.name.as_str())
            .collect::<Vec<_>>(),
        ["Kept", "Wire", "stored"]
    );
    assert_eq!(
        tree.data.coverage["rust"],
        BTreeSet::from([DataSource::RustSerde, DataSource::Extension])
    );
    assert_eq!(tree.data.edges.len(), 1);
    assert_eq!(tree.data.edges[0].to.name, "Wire");
    assert!(!tree.data.types[1].fields[0].optional);
}

fn tree(paths: &[&str]) -> TreeFacts {
    TreeFacts {
        files: paths
            .iter()
            .map(|&path| FileFact {
                path: path.into(),
                language: "rust".into(),
                lines: 1,
                test: false,
                read_status: ReadStatus::Complete,
                container: String::new(),
                module: ModuleId {
                    container: String::new(),
                    path: path.into(),
                },
                definitions: vec![],
            })
            .collect(),
        containers: vec![],
        modules: vec![],
        dependencies: vec![],
        file_references: vec![],
        module_references: vec![],
        package_edges: vec![],
        spawns: vec![],
        runtime: Default::default(),
        entry_points: Default::default(),
        data: Default::default(),
        coverage: Coverage::default(),
    }
}
