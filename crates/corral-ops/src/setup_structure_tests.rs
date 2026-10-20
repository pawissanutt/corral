use serde_json::{Value, json};

use super::parse_setup_input;
use crate::setup_validation::SetupValidationErrors;

const NOW: &str = "2026-10-18T12:00:00Z";

fn input() -> Value {
    let evidence = json!({
        "version":1, "id":"local-model", "retrieved_at":NOW,
        "published_on":null, "measured_on":null,
        "source":{"kind":"local_discovery","method":"fixture models"},
        "claim":"Exact local model listing", "finding":"Owner connection reported",
        "limitations":[]
    });
    let facets = json!({"installed":true,"discovered":true,"user_reported":true});
    json!({
        "proposal":{
            "version":1,"id":"proposal-1","prepared_at":NOW,
            "procedure_version":"fixture-v1","based_on_config_revision":"r",
            "preferences":{}, "basis":{"kind":"sourced"},
            "profiles":{"custom.database":{
                "harness":"fixture","model":"openai/exact-fixture-v1",
                "metadata":{
                    "display_name":"Database","purpose":"Persistence correctness",
                    "guidance":{"use_when":[],"avoid_when":[],"positive_examples":[],
                        "negative_examples":[],"distinctions":[]},
                    "instructions":"Check transaction boundaries",
                    "eligibility":{"enabled":true,"automatic":true,"roles":[],"projects":[]},
                    "revision":"profile-r1","evidence":"local-model",
                    "candidates":[{
                        "id":"preferred","harness":"fixture","connection":"owner",
                        "provider":"openai","model":"openai/exact-fixture-v1",
                        "effort":null,"evidence":"local-model","cost":{"kind":"subscription"}
                    }],"fallback":null
                }
            }},
            "roles":{}, "evidence":{"local-model":evidence}, "recommendations":{},
            "access":[{"profile":"custom.database","candidate":"preferred",
                "observed_at":NOW,"facets":facets,"evidence":["local-model"]}],
            "repair_policy":null
        },
        "discovery":{
            "observed_at":NOW,
            "harnesses":[{"key":"fixture","installed":true,"models":[],"efforts":[]}],
            "candidates":[{"harness":"fixture","connection":"owner","provider":"openai",
                "model":"openai/exact-fixture-v1","observed_at":NOW,"facets":facets,
                "settings":[],"evidence":[evidence]}]
        }
    })
}

fn problems_in(bytes: &[u8]) -> Vec<(String, String)> {
    parse_setup_input(bytes)
        .unwrap()
        .notes
        .problems
        .into_iter()
        .map(|(path, message)| (path.to_string(), message))
        .collect()
}

fn problems(value: &Value) -> Vec<(String, String)> {
    problems_in(&serde_json::to_vec(value).unwrap())
}

fn paths(problems: &[(String, String)]) -> Vec<&str> {
    problems.iter().map(|(path, _)| path.as_str()).collect()
}

#[test]
fn well_formed_input_decodes_without_problems() {
    assert!(problems(&input()).is_empty());
}

#[test]
fn independent_structural_defects_are_all_reported_with_paths() {
    // Given one input with six unrelated missing, unknown and mistyped fields.
    let mut value = input();
    value["proposal"]["version"] = json!("one");
    value["proposal"]["unexpected"] = json!(true);
    let metadata = &mut value["proposal"]["profiles"]["custom.database"]["metadata"];
    metadata.as_object_mut().unwrap().remove("purpose");
    metadata["guidance"]["use_when"] = json!("persistence");
    value["proposal"]["access"][0]["observed_at"] = json!("yesterday");
    value["discovery"]["candidates"][0]["settings"] = json!([{"effort":"high"}]);

    // When it is decoded once.
    let found = problems(&value);

    // Then every defect is reported at its own path.
    let paths = paths(&found);
    for expected in [
        "$.proposal.version",
        "$.proposal.unexpected",
        r#"$.proposal.profiles["custom.database"].metadata"#,
        r#"$.proposal.profiles["custom.database"].metadata.guidance.use_when"#,
        "$.proposal.access[0].observed_at",
        "$.discovery.candidates[0].settings[0]",
    ] {
        assert!(
            paths.contains(&expected),
            "{expected} missing from {found:#?}"
        );
    }
    assert_eq!(found.len(), 6, "{found:#?}");
}

#[test]
fn more_than_sixty_four_defects_are_all_reported() {
    // Given seventy mistyped provider preferences in a small input.
    let mut value = input();
    value["proposal"]["preferences"]["allowed_providers"] = json!(vec![7; 70]);

    // When it is decoded once.
    let found = problems(&value);

    // Then every element is reported and nothing claims the check stopped early.
    let expected: Vec<_> = (0..70)
        .map(|index| format!("$.proposal.preferences.allowed_providers[{index}]"))
        .collect();
    assert_eq!(paths(&found), expected);
}

#[test]
fn duplicate_typed_fields_stay_rejected_but_duplicate_map_keys_do_not() {
    // Given a proposal whose typed `procedure_version` field appears twice.
    let text = serde_json::to_string(&input()).unwrap();
    let duplicated = text.replacen(
        r#""procedure_version":"fixture-v1""#,
        r#""procedure_version":"other","procedure_version":"fixture-v1""#,
        1,
    );
    assert_ne!(duplicated, text);

    // When it is decoded, the typed decoder's duplicate rejection is preserved.
    let found = problems_in(duplicated.as_bytes());
    assert_eq!(
        found,
        [(
            "$.proposal.procedure_version".to_owned(),
            "duplicate field `procedure_version`".to_owned()
        )]
    );

    // Then a repeated map key, which typed decoding accepts, stays accepted.
    let map = text.replacen("\"roles\":{}", "\"roles\":{\"pm\":\"a\",\"pm\":\"b\"}", 1);
    assert_ne!(map, text);
    assert!(problems_in(map.as_bytes()).is_empty());
}

#[test]
fn metadata_fields_beside_metadata_say_where_they_belong() {
    // Given a profile whose metadata fields were written at the profile level.
    let mut value = input();
    let profile = &mut value["proposal"]["profiles"]["custom.database"];
    let metadata = profile.as_object_mut().unwrap().remove("metadata").unwrap();
    profile["guidance"] = metadata["guidance"].clone();
    profile["purpose"] = metadata["purpose"].clone();

    // When it is decoded.
    let found = problems(&value);

    // Then each misplaced field points into metadata.
    assert_eq!(
        found,
        ["guidance", "purpose"].map(|field| (
            format!(r#"$.proposal.profiles["custom.database"].{field}"#),
            format!("`{field}` belongs inside `metadata`"),
        ))
    );
}

#[test]
fn indices_after_an_unrepairable_element_keep_submitted_positions() {
    // Given two discovery candidates whose tagged sources cannot be repaired.
    let mut value = input();
    let candidate = value["discovery"]["candidates"][0].clone();
    value["discovery"]["candidates"] = json!([candidate.clone(), candidate]);
    for index in 0..2 {
        value["discovery"]["candidates"][index]["evidence"][0]["source"] = json!({"kind":"x"});
    }

    // When both elements are discarded so decoding can continue.
    let found = problems(&value);

    // Then each problem names the element's original index.
    let paths = paths(&found);
    assert!(
        paths[0].starts_with("$.discovery.candidates[0]"),
        "{found:#?}"
    );
    assert!(
        paths[1].starts_with("$.discovery.candidates[1]"),
        "{found:#?}"
    );
}

#[test]
fn malformed_json_is_one_root_problem() {
    let err = parse_setup_input(b"{").unwrap_err();
    let found = err.downcast::<SetupValidationErrors>().unwrap().0;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "$");
}
