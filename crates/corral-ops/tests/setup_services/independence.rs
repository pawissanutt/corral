//! Independent defects are additive: any combination reports exactly the union of what each
//! defect reports alone, so a structural repair never hides an unrelated verdict.
use super::*;
use serde_json::{Value, json};

struct Defect {
    name: &'static str,
    /// A path the defect must be reported at when it is the only one.
    path: &'static str,
    apply: fn(&mut Value),
}

/// Pairwise independent: no defect's verdict reads a node another defect changes.
fn catalog() -> Vec<Defect> {
    vec![
        Defect {
            name: "mistyped candidate argument",
            path: "$.proposal.profiles.database.metadata.candidates[0].args[0]",
            apply: |v| {
                v["proposal"]["profiles"]["database"]["metadata"]["candidates"][0]["args"] =
                    json!([7])
            },
        },
        Defect {
            name: "mistyped provider preference",
            path: "$.proposal.preferences.provider_preference[0]",
            apply: |v| v["proposal"]["preferences"]["provider_preference"] = json!([7]),
        },
        Defect {
            name: "mistyped evidence limitation",
            path: r#"$.proposal.evidence["local-support"].limitations[0]"#,
            apply: |v| v["proposal"]["evidence"]["local-support"]["limitations"] = json!([7]),
        },
        Defect {
            name: "mistyped recommendation uncertainty",
            path: "$.proposal.recommendations.database.uncertainty[0]",
            apply: |v| v["proposal"]["recommendations"]["database"]["uncertainty"] = json!([7]),
        },
        Defect {
            name: "mistyped observation facet",
            path: "$.discovery.candidates[0].facets.installed",
            apply: |v| v["discovery"]["candidates"][0]["facets"]["installed"] = json!("yes"),
        },
        Defect {
            name: "mistyped harness installation",
            path: "$.discovery.harnesses[0].installed",
            apply: |v| v["discovery"]["harnesses"][0]["installed"] = json!("yes"),
        },
        Defect {
            name: "unknown escalation target",
            path: "$.proposal.profiles.database.escalate_to",
            apply: |v| {
                v["proposal"]["profiles"]["database"]["escalate_to"] = json!("missing-profile")
            },
        },
        Defect {
            name: "unknown discovery harness",
            path: "$.discovery.candidates[0].harness",
            apply: |v| v["discovery"]["harnesses"][0]["key"] = json!("unrelated"),
        },
        Defect {
            name: "provider outside preferences",
            path: "$.proposal.profiles.database.metadata.candidates[0].provider",
            apply: |v| v["proposal"]["preferences"]["allowed_providers"] = json!(["other"]),
        },
        Defect {
            name: "recommendation for an unchanged profile",
            path: "$.proposal.recommendations.ghost",
            apply: |v| {
                v["proposal"]["recommendations"]["ghost"] = json!({
                    "reason": "Unrelated", "evidence": ["local-support"],
                    "alternatives_considered": [], "uncertainty": []
                });
            },
        },
        Defect {
            name: "unknown access evidence",
            path: "$.proposal.access[0].evidence[1]",
            apply: |v| v["proposal"]["access"][0]["evidence"] = json!(["local-support", "missing"]),
        },
        Defect {
            name: "access timestamp differs from discovery",
            path: "$.proposal.access[0].observed_at",
            apply: |v| v["proposal"]["access"][0]["observed_at"] = json!("2026-10-18T11:00:00Z"),
        },
        Defect {
            name: "manual basis",
            path: "$.proposal.basis",
            apply: |v| v["proposal"]["basis"] = json!({"kind": "manual"}),
        },
        Defect {
            name: "duplicate discovered harness",
            path: "$.discovery.harnesses[2].key",
            apply: |v| {
                let spare = json!({"key": "spare", "installed": true, "models": [], "efforts": []});
                let harnesses = v["discovery"]["harnesses"].as_array_mut().unwrap();
                harnesses.extend([spare.clone(), spare]);
            },
        },
        Defect {
            name: "unsafe observed connection",
            path: "$.discovery.candidates[0]",
            apply: |v| {
                let profile = &mut v["proposal"]["profiles"]["database"];
                profile["metadata"]["candidates"][0]["connection"] = json!("-unsafe");
                v["discovery"]["candidates"][0]["connection"] = json!("-unsafe");
            },
        },
        Defect {
            name: "unsafe harness effort",
            path: "$.discovery.harnesses[0]",
            apply: |v| v["discovery"]["harnesses"][0]["efforts"] = json!(["-unsafe"]),
        },
        Defect {
            name: "unknown recommendation evidence",
            path: "$.proposal.recommendations.database.evidence[1]",
            apply: |v| {
                v["proposal"]["recommendations"]["database"]["evidence"] =
                    json!(["local-support", "missing-recommendation"]);
            },
        },
        Defect {
            name: "unknown role profile",
            path: "$.proposal.roles.planning",
            apply: |v| v["proposal"]["roles"]["planning"] = json!("ghost-profile"),
        },
    ]
}

fn reported(store: &Store, base: &Value, chosen: &[&Defect]) -> BTreeSet<(String, String)> {
    let mut input = base.clone();
    for defect in chosen {
        (defect.apply)(&mut input);
    }
    let bytes = serde_json::to_vec(&input).unwrap();
    let error = parse_setup_input(&bytes)
        .and_then(|draft| setup_validate_draft(store, &draft))
        .unwrap_err();
    let errors = error.downcast::<SetupValidationErrors>().unwrap();
    errors.0.into_iter().map(|e| (e.path, e.message)).collect()
}

#[test]
fn independent_defects_report_exactly_the_union_of_their_own_errors() {
    // Given a valid input and pairwise independent structural and cross-reference defects.
    let (_dir, store, input) = fixture();
    let base = serde_json::to_value(&input).unwrap();
    let draft = parse_setup_input(&serde_json::to_vec(&base).unwrap()).unwrap();
    setup_validate_draft(&store, &draft).unwrap();
    let catalog = catalog();
    let alone: Vec<_> = catalog
        .iter()
        .map(|defect| reported(&store, &base, &[defect]))
        .collect();
    let mut failures: Vec<_> = catalog
        .iter()
        .zip(&alone)
        .filter(|(defect, errors)| !errors.iter().any(|(path, _)| path == defect.path))
        .map(|(defect, errors)| format!("{} not at {}: {errors:?}", defect.name, defect.path))
        .collect();

    // When every pair, and the whole catalog at once, is validated in one call.
    let n = catalog.len();
    let mut combinations: Vec<Vec<usize>> = (0..n)
        .flat_map(|i| (i + 1..n).map(move |j| vec![i, j]))
        .collect();
    combinations.push((0..n).collect());
    for combination in combinations {
        let chosen: Vec<_> = combination.iter().map(|&i| &catalog[i]).collect();
        let expected: BTreeSet<_> = combination
            .iter()
            .flat_map(|&i| alone[i].iter().cloned())
            .collect();
        let found = reported(&store, &base, &chosen);

        // Then the call reports exactly the union of each defect's own errors.
        if found != expected {
            let names: Vec<_> = chosen.iter().map(|defect| defect.name).collect();
            failures.push(format!(
                "{names:?}: missing {:?}; unexpected {:?}",
                expected.difference(&found).collect::<Vec<_>>(),
                found.difference(&expected).collect::<Vec<_>>()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
