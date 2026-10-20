//! Setup validation reports every independent defect in one call, without persisting.
mod setup_support;

use serde_json::json;
use setup_support::{Fixture, snapshot};

#[test]
fn independent_defects_are_reported_together_as_path_message_list() {
    // Given a gathering setup and one proposal with several unrelated cross-reference defects.
    let fixture = Fixture::new();
    let mut input = fixture.input();
    fixture.ok(&["begin", "--expected-revision", "0"]);
    let baseline = snapshot(fixture.root());
    input["discovery"]["candidates"][0]["settings"][0]["effort"] = json!("low");
    input["proposal"]["access"][0]["facets"]["access_verified"] = json!(true);
    input["proposal"]["access"][0]["observed_at"] = json!("2026-10-18T11:00:00Z");
    input["proposal"]["access"][0]["evidence"] = json!(["local-model", "missing-evidence"]);
    input["proposal"]["basis"] = json!({"kind":"manual"});
    let file = fixture.write("defects.json", &input);

    // When validate and propose each run once with --json.
    let validated = fixture.reject(&["validate", "--file", file.to_str().unwrap(), "--json"]);
    let proposed = fixture.reject(&[
        "propose",
        "--file",
        file.to_str().unwrap(),
        "--expected-revision",
        "1",
        "--json",
    ]);

    // Then every defect appears in one {path, message} list and nothing is persisted.
    assert_eq!(validated["errors"], proposed["errors"]);
    let errors = validated["errors"].as_array().unwrap();
    assert!(errors.iter().all(|error| {
        error.as_object().unwrap().len() == 2
            && error["path"].is_string()
            && error["message"].is_string()
    }));
    let paths: Vec<_> = errors.iter().map(|e| e["path"].as_str().unwrap()).collect();
    let candidate = r#"$.proposal.profiles["custom.database"].metadata.candidates[0]"#;
    for expected in [
        "$.proposal.basis",
        candidate,
        "$.proposal.access[0].facets",
        "$.proposal.access[0].observed_at",
        "$.proposal.access[0].evidence[1]",
    ] {
        assert!(paths.contains(&expected), "{expected}: {errors:#?}");
    }
    assert_eq!(snapshot(fixture.root()), baseline);

    // And structural defects are likewise collected with their JSON paths.
    let mut broken = fixture.input();
    broken["proposal"]["version"] = json!("one");
    broken["discovery"]["observed_at"] = json!(7);
    let profile = &mut broken["proposal"]["profiles"]["custom.database"];
    let metadata = profile.as_object_mut().unwrap().remove("metadata").unwrap();
    profile["guidance"] = metadata["guidance"].clone();
    let file = fixture.write("broken.json", &broken);
    let rejected = fixture.reject(&["validate", "--file", file.to_str().unwrap(), "--json"]);
    let errors = rejected["errors"].as_array().unwrap();
    let paths: Vec<_> = errors.iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert_eq!(
        paths,
        [
            r#"$.proposal.profiles["custom.database"].guidance"#,
            "$.discovery.observed_at",
            "$.proposal.version",
            r#"$.proposal.profiles["custom.database"].metadata"#,
            "$.proposal.access[0].candidate",
        ]
    );
    assert_eq!(errors[0]["message"], "`guidance` belongs inside `metadata`");
}

fn reject_both(fixture: &Fixture, name: &str, text: &str) -> Vec<(String, String)> {
    let file = fixture.root().join(name);
    std::fs::write(&file, text).unwrap();
    let file = file.to_str().unwrap();
    let baseline = snapshot(fixture.root());
    let validated = fixture.reject(&["validate", "--file", file, "--json"]);
    let proposed = fixture.reject(&[
        "propose",
        "--file",
        file,
        "--expected-revision",
        "1",
        "--json",
    ]);
    assert_eq!(validated["errors"], proposed["errors"]);
    assert_eq!(snapshot(fixture.root()), baseline, "{name} was persisted");
    validated["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let path = e["path"].as_str().unwrap().to_owned();
            (path, e["message"].as_str().unwrap().to_owned())
        })
        .collect()
}

#[test]
fn structural_and_reference_defects_and_duplicates_are_reported_together() {
    // Given a gathering setup.
    let fixture = Fixture::new();
    fixture.ok(&["begin", "--expected-revision", "0"]);
    let candidate = r#"$.proposal.profiles["custom.database"].metadata.candidates[0]"#;

    // When a mistyped field accompanies missing access and discovery references.
    let mut mixed = fixture.input();
    mixed["proposal"]["version"] = json!("one");
    mixed["proposal"]["access"] = json!([]);
    mixed["discovery"]["candidates"] = json!([]);
    let found = reject_both(&fixture, "mixed.json", &mixed.to_string());

    // Then the field and both independent reference failures arrive in one call,
    // without verdicts computed from the placeholder version.
    let paths: Vec<_> = found.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        paths,
        ["$.proposal.version", candidate, candidate],
        "{found:#?}"
    );
    assert!(
        found[1].1.contains("one independent access record"),
        "{found:#?}"
    );
    assert!(
        found[2].1.contains("no independent discovery"),
        "{found:#?}"
    );

    // When metadata, recommendation and access references are all unresolved.
    let mut unresolved = fixture.input();
    let profile = &mut unresolved["proposal"]["profiles"]["custom.database"];
    profile["metadata"]["evidence"] = json!("missing-metadata-evidence");
    unresolved["proposal"]["recommendations"]["custom.database"]["evidence"] =
        json!(["missing-recommendation-evidence"]);
    let mut stray = unresolved["proposal"]["access"][0].clone();
    stray["profile"] = json!("missing-profile");
    stray["candidate"] = json!("missing-candidate");
    stray["evidence"] = json!(["missing-access-evidence"]);
    unresolved["proposal"]["access"]
        .as_array_mut()
        .unwrap()
        .push(stray);
    let found = reject_both(&fixture, "unresolved.json", &unresolved.to_string());

    // Then each failure is reported at its owning path.
    for (path, message) in [
        (
            r#"$.proposal.profiles["custom.database"].metadata.evidence"#,
            "unknown evidence 'missing-metadata-evidence'",
        ),
        (
            r#"$.proposal.recommendations["custom.database"].evidence[0]"#,
            "unknown evidence 'missing-recommendation-evidence'",
        ),
        (
            "$.proposal.access[1].profile",
            "access names unknown profile",
        ),
        (
            "$.proposal.access[1].evidence[0]",
            "unknown evidence 'missing-access-evidence'",
        ),
    ] {
        assert!(
            found.contains(&(path.to_owned(), message.to_owned())),
            "{path}: {found:#?}"
        );
    }

    // When a typed field is duplicated, it stays rejected rather than last-value-wins.
    let text = fixture.input().to_string();
    let duplicated = text.replacen(
        r#""procedure_version":"fixture-v1""#,
        r#""procedure_version":"other","procedure_version":"fixture-v1""#,
        1,
    );
    assert_ne!(duplicated, text);
    let found = reject_both(&fixture, "duplicate.json", &duplicated);
    assert_eq!(
        found,
        [(
            "$.proposal.procedure_version".to_owned(),
            "duplicate field `procedure_version`".to_owned()
        )]
    );
}

const CANDIDATE: &str = r#"$.proposal.profiles["custom.database"].metadata.candidates[0]"#;

fn assert_reported(found: &[(String, String)], expected: &[(&str, &str)]) {
    for (path, fragment) in expected {
        assert!(
            found
                .iter()
                .any(|(at, message)| at == path && message.contains(fragment)),
            "{path} / {fragment}: {found:#?}"
        );
    }
    assert!(
        found.iter().all(
            |(_, message)| !message.contains("cannot be checked further")
                && !message.starts_with("missing field `basis`")
        ),
        "{found:#?}"
    );
}

#[test]
fn malformed_unrelated_fields_do_not_hide_independent_reference_failures() {
    // Given a gathering setup.
    let fixture = Fixture::new();
    fixture.ok(&["begin", "--expected-revision", "0"]);

    // When a candidate argument is mistyped and both reference collections are empty.
    let mut input = fixture.input();
    let candidate = &mut input["proposal"]["profiles"]["custom.database"]["metadata"];
    candidate["candidates"][0]["args"] = json!([7]);
    input["proposal"]["access"] = json!([]);
    input["discovery"]["candidates"] = json!([]);
    let found = reject_both(&fixture, "args.json", &input.to_string());

    // Then the argument and both identity-based reference failures are reported together.
    let args = format!("{CANDIDATE}.args[0]");
    assert_reported(
        &found,
        &[
            (&args, "invalid type"),
            (CANDIDATE, "one independent access record; found 0"),
            (CANDIDATE, "no independent discovery"),
        ],
    );

    // When a nonmatching observation has a malformed facet that is not part of its identity.
    let mut input = fixture.input();
    let candidate = &mut input["proposal"]["profiles"]["custom.database"]["metadata"];
    candidate["candidates"][0]["model"] = json!("openai/other-model");
    input["discovery"]["candidates"][0]["facets"]["installed"] = json!("yes");
    let found = reject_both(&fixture, "facet.json", &input.to_string());

    // Then the missing matching observation is still reported.
    assert_reported(
        &found,
        &[
            ("$.discovery.candidates[0].facets.installed", "invalid type"),
            (CANDIDATE, "no independent discovery"),
        ],
    );
}

#[test]
fn malformed_candidate_fields_do_not_hide_profile_reference_failures() {
    // Given a gathering setup.
    let fixture = Fixture::new();
    fixture.ok(&["begin", "--expected-revision", "0"]);
    let args = format!("{CANDIDATE}.args[0]");

    // When a candidate argument is mistyped and access names an unknown candidate.
    let mut input = fixture.input();
    let metadata = &mut input["proposal"]["profiles"]["custom.database"]["metadata"];
    metadata["candidates"][0]["args"] = json!([7]);
    input["proposal"]["access"][0]["candidate"] = json!("missing-candidate");
    let found = reject_both(&fixture, "reverse.json", &input.to_string());

    // Then the argument and the access-to-candidate failure arrive in one call.
    assert_reported(
        &found,
        &[
            (&args, "invalid type"),
            (
                "$.proposal.access[0].candidate",
                "access names unknown candidate",
            ),
        ],
    );

    // When a candidate argument is mistyped and every recommendation is missing.
    let mut input = fixture.input();
    let metadata = &mut input["proposal"]["profiles"]["custom.database"]["metadata"];
    metadata["candidates"][0]["args"] = json!([7]);
    input["proposal"]["recommendations"] = json!({});
    let found = reject_both(&fixture, "recommendation.json", &input.to_string());

    // Then the argument and the missing recommendation arrive in one call.
    assert_reported(
        &found,
        &[
            (&args, "invalid type"),
            (
                r#"$.proposal.recommendations["custom.database"]"#,
                "missing profile recommendation",
            ),
        ],
    );
}

#[test]
fn tagged_enum_members_and_tags_are_reported_without_hiding_other_failures() {
    // Given a gathering setup.
    let fixture = Fixture::new();
    fixture.ok(&["begin", "--expected-revision", "0"]);

    // When both members of a metered cost are mistyped.
    let mut input = fixture.input();
    let candidate = &mut input["proposal"]["profiles"]["custom.database"]["metadata"];
    candidate["candidates"][0]["cost"] =
        json!({"kind":"metered","currency":7,"maximum_micros":"many"});
    let found = reject_both(&fixture, "metered.json", &input.to_string());

    // Then each member is reported at its own path.
    let currency = format!("{CANDIDATE}.cost.currency");
    let maximum = format!("{CANDIDATE}.cost.maximum_micros");
    assert_reported(
        &found,
        &[(&currency, "expected a string"), (&maximum, "expected u64")],
    );

    // When an enum tag is unknown and both reference collections are empty.
    let mut input = fixture.input();
    input["proposal"]["basis"] = json!({"kind":"typo"});
    input["proposal"]["access"] = json!([]);
    input["discovery"]["candidates"] = json!([]);
    let found = reject_both(&fixture, "tag.json", &input.to_string());

    // Then the tag and the independent reference failures arrive in one call.
    assert_reported(
        &found,
        &[
            ("$.proposal.basis.kind", "unknown variant `typo`"),
            (CANDIDATE, "one independent access record; found 0"),
            (CANDIDATE, "no independent discovery"),
        ],
    );
}
