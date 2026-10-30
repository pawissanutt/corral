use super::*;
use corral_core::ModelEvidenceSource;
use serde_json::json;

fn timestamp() -> Timestamp {
    "2026-10-18T12:00:00Z".parse().unwrap()
}

fn discovery() -> SetupDiscovery {
    SetupDiscovery {
        observed_at: timestamp(),
        harnesses: vec![DiscoveredHarness {
            key: "omo".into(),
            installed: true,
            models: vec!["provider/model".into()],
            efforts: vec!["high".into()],
            vendors: vec![],
        }],
        candidates: vec![CandidateObservation {
            harness: "omo".into(),
            connection: "personal".into(),
            provider: "provider".into(),
            model: "provider/model".into(),
            observed_at: timestamp(),
            facets: AccessFacets {
                installed: true,
                discovered: true,
                ..AccessFacets::default()
            },
            settings: vec![CandidateSettings {
                effort: Some("high".into()),
                args: vec!["--max-tokens=1000".into()],
            }],
            evidence: vec![],
        }],
    }
}

fn local_evidence() -> ModelEvidence {
    ModelEvidence {
        version: 1,
        id: "probe-1".into(),
        source: ModelEvidenceSource::LocalDiscovery {
            method: "explicit exact-candidate observation".into(),
        },
        retrieved_at: timestamp(),
        published_on: None,
        measured_on: Some("2026-10-18".parse().unwrap()),
        claim: "personal connection accepts provider/model".into(),
        finding: "observed response for recorded settings".into(),
        limitations: vec![],
    }
}

#[test]
fn sanitized_inventory_when_raw_probe_contains_private_fields() {
    // Given: private probe details and unsafe listing entries.
    let raw = crate::HarnessInfo {
        key: "omo".into(),
        program: Some("private-command".into()),
        path: Some("/private/bin/omo".into()),
        version: Some("private-version".into()),
        models: vec![
            "provider/model".into(),
            "sk-private".into(),
            "/private/model".into(),
        ],
        model_source: crate::ModelSource::Listed,
        efforts: vec!["high".into(), "Bearer private".into()],
        takes_effort: true,
        notes: vec!["API_KEY=private".into()],
    };
    // When: the raw report is projected into discovery.
    let report = sanitized_discovery(timestamp(), vec![raw]);
    // Then: only the allowlisted inventory fields survive, with no access candidates.
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        json!({
            "observed_at": "2026-10-18T12:00:00Z",
            "harnesses": [{
                "key": "omo", "installed": true,
                "models": ["provider/model"], "efforts": ["high"]
            }],
            "candidates": []
        })
    );
    report.validate().unwrap();
}

#[test]
fn empty_inventory_when_config_has_no_harnesses() {
    // Given: no configured commands, so discovery cannot launch any subprocess.
    let mut config = Config::default_for(std::path::Path::new("/nonexistent"));
    config.harnesses.clear();
    config.agents.clear();
    // When: local discovery runs.
    let report = discover_with(
        timestamp(),
        crate::detect_harnesses(&config),
        &config,
        false,
        vec!["openai".into()],
    );
    // Then: it supplies no candidate or inferred access.
    assert!(report.harnesses.is_empty());
    assert!(report.candidates.is_empty());
    report.validate().unwrap();
}

#[test]
fn pi_native_is_reported_ready_with_its_vendors_and_efforts() {
    // Given a config that does not name pi-native, and a host the doctor reports ready.
    let mut config = Config::default_for(std::path::Path::new("/nonexistent"));
    config.harnesses.clear();
    config.agents.clear();
    // When discovery runs.
    let report = discover_with(
        timestamp(),
        Vec::new(),
        &config,
        true,
        vec!["openai".into()],
    );
    // Then the built-in host is listed installed, free-text model, openai only.
    let entry = report
        .harnesses
        .iter()
        .find(|h| h.key == "pi-native")
        .unwrap();
    assert!(entry.installed && entry.models.is_empty());
    assert_eq!(entry.vendors, vec!["openai".to_owned()]);
    assert!(entry.efforts.contains(&"medium".to_owned()));
    report.validate().unwrap();
}

#[test]
fn pi_native_carries_the_models_its_host_listed() {
    // Given a ready host whose detection listed provider-qualified models.
    let mut config = Config::default_for(std::path::Path::new("/nonexistent"));
    config.harnesses.clear();
    config.agents.clear();
    let probed = crate::HarnessInfo {
        key: "pi-native".into(),
        program: None,
        path: Some("/bin/bun".into()),
        version: Some("1.3.0".into()),
        models: vec!["openai-codex/gpt-6.1-sol".into()],
        model_source: crate::ModelSource::Listed,
        efforts: Vec::new(),
        takes_effort: true,
        notes: Vec::new(),
    };
    // When discovery runs.
    let report = discover_with(
        timestamp(),
        vec![probed],
        &config,
        true,
        vec!["openai".into()],
    );
    // Then the settings picker and routing see the listed models.
    let entry = report
        .harnesses
        .iter()
        .find(|h| h.key == "pi-native")
        .unwrap();
    assert_eq!(entry.models, vec!["openai-codex/gpt-6.1-sol".to_owned()]);
    report.validate().unwrap();
}

#[test]
fn pi_native_is_listed_unready_only_when_configured() {
    // Given an unready host and a config that opts in through a pi-native harness.
    let mut config = Config::default_for(std::path::Path::new("/nonexistent"));
    config.harnesses.clear();
    config.agents.clear();
    assert!(
        discover_with(
            timestamp(),
            Vec::new(),
            &config,
            false,
            vec!["openai".into()]
        )
        .harnesses
        .is_empty()
    );
    config.harnesses.insert(
        "pi-native".into(),
        corral_core::Harness {
            command: "{corral} agent launch {delivery_args}".into(),
            flags: None,
            model_flag: None,
            effort_flag: None,
            efforts: Vec::new(),
            prompt: None,
        },
    );
    // When discovery also saw a probed (and not installed) entry of the same key.
    let probed = crate::detect_harnesses(&config);
    let report = discover_with(timestamp(), probed, &config, false, vec!["openai".into()]);
    // Then one entry remains, not installed.
    let entries: Vec<_> = report
        .harnesses
        .iter()
        .filter(|h| h.key == "pi-native")
        .collect();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].installed);
    report.validate().unwrap();
}

#[test]
fn rejects_unsafe_harness_keys_when_projecting() {
    // Given: a raw entry with a credential-like identity.
    let raw = crate::HarnessInfo {
        key: "sk-private".into(),
        program: None,
        path: None,
        version: None,
        models: vec![],
        model_source: crate::ModelSource::None,
        efforts: vec![],
        takes_effort: false,
        notes: vec![],
    };
    // When: the raw entry is sanitized.
    let report = sanitized_discovery(timestamp(), vec![raw]);
    // Then: the unsafe identity is absent rather than redacted into a new identity.
    assert!(report.harnesses.is_empty());
}

#[test]
fn facets_remain_independent_when_round_tripped() {
    // Given: each observation facet by itself, backed by evidence.
    for facets in [
        AccessFacets {
            installed: true,
            ..AccessFacets::default()
        },
        AccessFacets {
            discovered: true,
            ..AccessFacets::default()
        },
        AccessFacets {
            user_reported: true,
            ..AccessFacets::default()
        },
        AccessFacets {
            access_verified: true,
            ..AccessFacets::default()
        },
        AccessFacets {
            execution_tested: true,
            ..AccessFacets::default()
        },
    ] {
        let mut report = discovery();
        report.candidates[0].facets = facets.clone();
        report.candidates[0].evidence.push(local_evidence());
        // When: a valid report is serialized and decoded.
        report.validate().unwrap();
        let decoded: SetupDiscovery =
            serde_json::from_value(serde_json::to_value(&report).unwrap()).unwrap();
        // Then: no facet implies another facet.
        assert_eq!(decoded.candidates[0].facets, facets);
        assert_eq!(decoded, report);
    }
}

#[test]
fn rejects_verification_when_evidence_is_missing_or_web_only() {
    // Given: verification or execution assertions with no local evidence.
    for execution in [false, true] {
        for web in [false, true] {
            let mut report = discovery();
            report.candidates[0].facets.access_verified = !execution;
            report.candidates[0].facets.execution_tested = execution;
            if web {
                let mut evidence = local_evidence();
                evidence.source = ModelEvidenceSource::Web {
                    url: "https://example.com/models".into(),
                };
                report.candidates[0].evidence.push(evidence);
            }
            // When: validating an assertion that listing or web evidence cannot prove.
            let result = report.validate();
            // Then: it is rejected.
            assert!(result.is_err());
        }
    }
}

#[test]
fn rejects_missing_settings_when_execution_was_tested() {
    // Given: execution evidence without the exact tested settings.
    let mut report = discovery();
    report.candidates[0].facets.execution_tested = true;
    report.candidates[0].evidence.push(local_evidence());
    report.candidates[0].settings.clear();
    // When: validating.
    let result = report.validate();
    // Then: unspecified settings cannot be treated as tested settings.
    assert!(result.is_err());
}

#[test]
fn strict_decoding_when_unknown_fields_or_wrong_settings_types_arrive() {
    // Given: otherwise valid serialized discovery with an unknown field at each layer.
    let original = serde_json::to_value(discovery()).unwrap();
    for pointer in [
        "",
        "/harnesses/0",
        "/candidates/0",
        "/candidates/0/settings/0",
        "/candidates/0/facets",
    ] {
        let mut value = original.clone();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("secret".into(), json!("private"));
        // When: decoding.
        let result = serde_json::from_value::<SetupDiscovery>(value);
        // Then: each struct rejects arbitrary fields.
        assert!(result.is_err(), "{pointer}");
    }
    for settings in [
        json!({"effort": null, "args": []}),
        json!([{"effort": 1, "args": []}]),
        json!([{"effort": null, "args": "not argv"}]),
        json!([{"effort": null}]),
    ] {
        let mut value = original.clone();
        value["candidates"][0]["settings"] = settings;
        assert!(serde_json::from_value::<SetupDiscovery>(value).is_err());
    }
}

#[test]
fn rejects_secrets_and_identity_overrides_when_args_are_supplied() {
    // Given: both split and equals credential flags, secret values, and identity overrides.
    for arg in [
        "--api-key",
        "--auth-token=private",
        "--refresh-token",
        "--clientSecret=private",
        "--key",
        "--key-file=/private/key",
        "-Hprivate",
        "-uuser:private",
        "--api_key=private",
        "--token=private",
        "--password",
        "Bearer private",
        "sk-private",
        "ghp_private",
        "OPENAI_API_KEY=private",
        "--header",
        "-H",
        "--model=other",
        "--provider",
        "--connection=other",
        "--effort=low",
        "",
        " ",
        "line\nbreak",
    ] {
        let mut report = discovery();
        report.candidates[0].settings[0].args = vec![arg.into()];
        // When: validating supplied settings.
        let result = report.validate();
        // Then: unsafe argv cannot enter a setup observation.
        assert!(result.is_err(), "{arg:?}");
    }
}

#[test]
fn rejects_invalid_identities_when_candidates_are_supplied() {
    // Given: blank, credential-like, multiline, and credential-URL identities.
    for identity in [
        "",
        " ",
        "Bearer private",
        "sk-private",
        "https://user:pass@host",
        "bad\nid",
    ] {
        for field in ["harness", "connection", "provider", "model"] {
            let mut value = serde_json::to_value(discovery()).unwrap();
            value["candidates"][0][field] = json!(identity);
            let report: SetupDiscovery = serde_json::from_value(value).unwrap();
            // When: validating the typed report.
            let result = report.validate();
            // Then: all exact identity components are checked.
            assert!(result.is_err(), "{field}: {identity:?}");
        }
    }
}

#[test]
fn exact_duplicates_rejected_but_connections_remain_distinct() {
    // Given: two observations for the same harness/provider/model.
    for same_connection in [false, true] {
        let mut report = discovery();
        let mut second = report.candidates[0].clone();
        if !same_connection {
            second.connection = "team".into();
        }
        report.candidates.push(second);
        // When: checking exact candidate identity.
        let result = report.validate();
        // Then: only identical connection tuples collide.
        assert_eq!(result.is_err(), same_connection);
    }
}

#[test]
fn rejects_broken_evidence_when_validating_observations() {
    // Given: evidence with a broken identity, source, date, or version.
    let mut original = discovery();
    original.candidates[0].evidence.push(local_evidence());
    for (field, value) in [
        ("version", json!(2)),
        ("id", json!("")),
        ("claim", json!(" ")),
        ("finding", json!("")),
        ("retrieved_at", json!("2026-10-19T12:00:00Z")),
        ("published_on", json!("2026-10-19")),
        ("measured_on", json!("2026-10-19")),
        ("source", json!({"kind": "local_discovery", "method": ""})),
        ("source", json!({"kind": "web", "url": "file:///private"})),
    ] {
        let mut value_report = serde_json::to_value(&original).unwrap();
        value_report["candidates"][0]["evidence"][0][field] = value;
        let report: SetupDiscovery = serde_json::from_value(value_report).unwrap();
        // When: validating evidence integrity.
        let result = report.validate();
        // Then: the invalid reference is rejected.
        assert!(result.is_err(), "{field}");
    }
}

#[test]
fn rejects_duplicate_evidence_and_settings_or_empty_effort() {
    // Given: each otherwise-valid report has one ambiguous or empty record.
    for defect in 0..4 {
        let mut report = discovery();
        report.candidates[0].evidence.push(local_evidence());
        match defect {
            0 => report.candidates[0].evidence.push(local_evidence()),
            1 => {
                let duplicate = report.candidates[0].settings[0].clone();
                report.candidates[0].settings.push(duplicate);
            }
            2 => report.candidates[0].settings[0].effort = Some(String::new()),
            3 => report.candidates[0].observed_at = "2026-10-19T12:00:00Z".parse().unwrap(),
            _ => unreachable!(),
        }
        // When: validating.
        let result = report.validate();
        // Then: duplicate evidence/settings, empty effort, and inconsistent dates fail.
        assert!(result.is_err(), "{defect}");
    }
}
