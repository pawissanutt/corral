use super::{DiagnosticCode::*, *};
use serde_json::json;

const FIXTURE: &[u8] = include_bytes!("../../../docs/examples/submit-to-land.diagram.json");

fn fixture() -> Value {
    serde_json::from_slice(FIXTURE).unwrap()
}

#[test]
fn recognizes_the_root_schema_and_preserves_ids_in_a_cyclic_flow() {
    assert!(declares_schema(FIXTURE));
    assert!(!declares_schema(
        br#"{"example":{"schema":"corral.workflow-diagram"}}"#
    ));
    assert!(!declares_schema(b"# corral.workflow-diagram"));
    let recipe = validate(FIXTURE).unwrap();
    assert_eq!(recipe.start, "submit");
    assert_eq!(recipe.edges[2].id, "gate-fails");
    assert_eq!(recipe.edges[2].to, "submit");
    let bindings = recipe.nodes[1].bindings.as_ref().unwrap();
    assert_eq!(
        bindings
            .source
            .as_ref()
            .unwrap()
            .span
            .as_ref()
            .unwrap()
            .start,
        101
    );
    assert!(matches!(bindings.entity, Some(EntityRequest::File { .. })));
    assert_eq!(
        validate(&serde_json::to_vec(&recipe).unwrap()).unwrap(),
        recipe
    );
}

#[test]
fn rejects_incompatible_and_unsafe_recipe_shapes() {
    let cases = [
        ("/schema", json!("other"), UnsupportedSchema, "/schema"),
        ("/version", json!(9), UnsupportedVersion, "/version"),
        (
            "/subject/commit",
            json!("main"),
            InvalidSubject,
            "/subject/commit",
        ),
        (
            "/subject/project",
            json!("https://test.invalid"),
            InvalidSubject,
            "/subject/project",
        ),
        ("/nodes/1/id", json!("submit"), DuplicateId, "/nodes/1/id"),
        ("/edges/1/id", json!("submit"), DuplicateId, "/edges/1/id"),
        (
            "/edges/1/id",
            json!("submit-to-gate"),
            DuplicateId,
            "/edges/1/id",
        ),
        ("/edges/2/to", json!("missing"), DanglingEdge, "/edges/2/to"),
        ("/start", json!("land"), InvalidStart, "/start"),
        (
            "/nodes/1/label",
            json!("x".repeat(161)),
            InvalidLabel,
            "/nodes/1/label",
        ),
        (
            "/edges/1",
            json!({"id":"gate-passes","kind":"branch","from":"gate","to":"review"}),
            InvalidBranch,
            "/edges/1",
        ),
        (
            "/edges/4/label",
            json!("Approve"),
            InvalidBranch,
            "/edges/4/label",
        ),
        (
            "/nodes/1/bindings/source/path",
            json!("src/../private"),
            InvalidBinding,
            "/nodes/1/bindings/source/path",
        ),
        (
            "/nodes/1/bindings/source/span/end",
            json!(301),
            InvalidBinding,
            "/nodes/1/bindings/source/span",
        ),
        (
            "/nodes/1/bindings/entity/path",
            json!("file:///etc/passwd"),
            InvalidBinding,
            "/nodes/1/bindings/entity/path",
        ),
        (
            "/nodes/1/bindings/entity",
            json!({"kind":"open_url","url":"https://test.invalid"}),
            InvalidShape,
            "",
        ),
    ];
    for (pointer, input, code, at) in cases {
        let mut altered = fixture();
        *altered.pointer_mut(pointer).unwrap() = input;
        let bytes = serde_json::to_vec(&altered).unwrap();
        assert_eq!(declares_schema(&bytes), code != UnsupportedSchema);
        let diagnostic = validate(&bytes).unwrap_err();
        assert_eq!(
            (diagnostic.code, diagnostic.at.as_str()),
            (code, at),
            "{pointer}"
        );
    }
    let mut altered = fixture();
    altered["nodes"][0]["on_click"] = json!("anything");
    assert_eq!(
        validate(&serde_json::to_vec(&altered).unwrap())
            .unwrap_err()
            .code,
        InvalidShape
    );
    let duplicated = std::str::from_utf8(FIXTURE).unwrap().replacen(
        "\"version\": 1,",
        "\"version\": 1, \"version\": 1,",
        1,
    );
    assert_eq!(
        validate(duplicated.as_bytes()).unwrap_err().code,
        InvalidShape
    );
    assert_eq!(validate(b"{").unwrap_err().code, InvalidJson);
}

#[test]
fn bounds_input_and_rejects_dead_flow() {
    let mut bytes = FIXTURE.to_vec();
    bytes.resize(MAX_RECIPE_BYTES, b' ');
    assert!(validate(&bytes).is_ok());
    bytes.push(b' ');
    assert!(!declares_schema(&bytes));
    assert_eq!(validate(&bytes).unwrap_err().code, TooLarge);

    for (field, limit) in [("nodes", MAX_NODES), ("edges", MAX_EDGES)] {
        let mut altered = fixture();
        altered[field] = Value::Array(vec![altered[field][0].clone(); limit + 1]);
        let diagnostic = validate(&serde_json::to_vec(&altered).unwrap()).unwrap_err();
        assert_eq!(diagnostic.code, LimitExceeded);
        assert_eq!(diagnostic.at, format!("/{field}"));
    }
    let mut altered = fixture();
    // A second edge to an existing destination is valid if distinctly labeled and identified.
    altered["edges"][4]["to"] = json!("land");
    assert!(validate(&serde_json::to_vec(&altered).unwrap()).is_ok());
    altered["edges"][4]["to"] = json!("submit");
    altered["edges"][3]["to"] = json!("submit");
    assert_eq!(
        validate(&serde_json::to_vec(&altered).unwrap())
            .unwrap_err()
            .code,
        UnreachableNode
    );
    altered["nodes"].as_array_mut().unwrap().pop();
    assert_eq!(
        validate(&serde_json::to_vec(&altered).unwrap())
            .unwrap_err()
            .code,
        InvalidFlow
    );
}
