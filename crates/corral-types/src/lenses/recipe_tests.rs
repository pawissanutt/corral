use super::{MAX_REPO_RECIPE_BYTES, RepoRecipe};
use serde_json::{Value, json};

fn recipe() -> Value {
    json!({
        "schema":"corral.repo-diagram", "version":1, "id":"people", "title":"People", "layout":"graph",
        "groups":[{"id":"staff","label":"Staff"}],
        "nodes":[{"id":"person","label":"Person","group":"staff"}],
        "edges":[{"id":"manager","from":"person","to":"person","label":"Managed by","from_label":"*","to_label":"0..1"}],
        "evidence":[{"element":"manager","anchor":{"kind":"text","path":"src/model.rs","text":"struct Manager"}}],
        "coverage":{"path":"src/model.rs","name":"State","nodes":["person"]}
    })
}

#[test]
fn references_are_typed_and_ids_are_global() {
    assert!(RepoRecipe::from_json(&recipe().to_string()).is_ok());
    // Each invalid reference points to an existing ID, but of the wrong primitive type.
    for (pointer, value) in [
        ("/nodes/0/group", "manager"),
        ("/edges/0/to", "staff"),
        ("/coverage/nodes/0", "manager"),
        // Neither the lens ID nor the label is a selectable evidence target.
        ("/evidence/0/element", "people"),
        ("/evidence/0/element", "Person"),
        ("/edges/0/id", "staff"),
    ] {
        let mut bad = recipe();
        *bad.pointer_mut(pointer).unwrap() = json!(value);
        assert!(
            RepoRecipe::from_json(&bad.to_string()).is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn only_literal_committed_source_requests_are_accepted() {
    for anchor in [
        json!({"kind":"text", "path":"a/../secret", "text":"x"}),
        json!({"kind":"symbol", "path":"C:\\src", "name":"x"}),
        json!({"kind":"path", "path":".git/config"}),
        json!({"kind":"path", "path":"file:///etc/hosts"}),
        json!({"kind":"text", "path":"src/main.rs", "text":" "}),
        json!({"kind":"entry", "entry_kind":"route", "name":"/", "command":"run"}),
        json!({"kind":"line", "path":"src/main.rs", "line":1}),
    ] {
        let mut bad = recipe();
        bad["evidence"][0]["anchor"] = anchor;
        assert!(RepoRecipe::from_json(&bad.to_string()).is_err());
    }
}

#[test]
fn document_and_primitives_have_independent_budgets() {
    let mut doc = recipe().to_string();
    doc.extend(std::iter::repeat_n(
        ' ',
        MAX_REPO_RECIPE_BYTES + 1 - doc.len(),
    ));
    assert!(RepoRecipe::from_json(&doc).is_err());

    let mut many = recipe();
    let edges = many["edges"].as_array_mut().unwrap();
    for i in 0..160 {
        let mut edge = edges[0].clone();
        edge["id"] = json!(format!("e{i}"));
        edges.push(edge);
    }
    assert!(many.to_string().len() <= MAX_REPO_RECIPE_BYTES);
    assert!(RepoRecipe::from_json(&many.to_string()).is_err());
}

#[test]
fn strict_versioned_envelope_has_no_revision_or_viewer_override() {
    let doc = recipe().to_string();
    for bad in [
        doc.replace("\"version\":1", "\"version\":2"),
        doc.replace("\"version\":1", "\"version\":2,\"version\":1"),
        doc.replace("\"graph\"", "\"custom\""),
        doc.replace("\"schema\":", "\"read_at\":\"main\",\"schema\":"),
        doc.replace("\"schema\":", "\"script\":\"run\",\"schema\":"),
    ] {
        assert!(RepoRecipe::from_json(&bad).is_err());
    }
}
