use super::*;

#[test]
fn malformed_protocol_is_an_error_even_for_conflict_status() {
    // Given truncated, misframed, or invalid merge-tree records.
    let oid = "a".repeat(40);
    let malformed = [
        String::new(),
        format!("{oid}\0"),
        format!("{oid}\0file\0"),
        format!("{oid}\0\0one\0"),
        format!("{oid}\0\01\0"),
        format!("{oid}\0\01\0path\0kind\0"),
        format!("{oid}\0\01\0path\0kind\0text"),
        format!("{oid}\0\0{}\0", usize::MAX),
        format!("{oid}\0\00\0\0text\0"),
        "not-an-oid\0\0".to_owned(),
    ];
    // When parsing either recognized process status, then malformed data is never a conflict.
    for output in malformed {
        for conflict in [false, true] {
            assert!(
                parse_output(output.as_bytes(), conflict).is_err(),
                "{output:?}"
            );
        }
    }
    assert!(parse_output(b"\xff\0\0", true).is_err());
    assert!(parse_output(format!("{oid}\0\0").as_bytes(), true).is_err());
    assert!(parse_output(format!("{oid}\0file\0\0").as_bytes(), false).is_err());
}

#[test]
fn classification_uses_status_and_preserves_structured_messages() {
    // Given a valid informational record whose prose is not a classification signal.
    let oid = "a".repeat(40);
    let output = format!("{oid}\0\01\0odd\npath\0Auto-merging\0CONFLICT in harmless prose\n\0");
    // When parsing a success status, then its text does not turn it into a conflict.
    assert_eq!(
        parse_output(output.as_bytes(), false).unwrap(),
        MergePreflight::Clean {
            tree: oid,
            messages: vec![MergeMessage {
                paths: vec!["odd\npath".to_owned()],
                kind: "Auto-merging".to_owned(),
                message: "CONFLICT in harmless prose\n".to_owned(),
            }],
        }
    );
}
