//! Missing-gate advice and instruction-conflict reports are read-only, including when doctor
//! runs with --fix.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::PathBuf;

use corral_core::{ProfileRole, Store, detect_gate_presets};
use corral_ops::{GateInstructionConflict, scan_gate_instructions};

use super::Check;

pub(super) fn checks(store: &Store) -> Vec<Check> {
    let mut checks = gate_advice(store);
    checks.extend(instruction_conflicts(store));
    checks
}

fn gate_advice(store: &Store) -> Vec<Check> {
    store
        .config
        .projects
        .iter()
        .filter(|(name, _)| {
            store.config.has_review_policy(Some(name), None)
                && store.config.review_policy(Some(name), None).gate.is_empty()
        })
        .map(|(name, _)| {
            let suggestions = store
                .project_path(name)
                .and_then(|repo| detect_gate_presets(&repo));
            let advice = match suggestions {
                Ok(presets) if !presets.is_empty() => format!(
                    "Suggested commands (choose and configure; never applied automatically): {}",
                    presets
                        .iter()
                        .map(|preset| format!("{}: {}", preset.name, preset.commands.join(" && ")))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
                Ok(_) => {
                    "No stack preset detected; choose the repository's validation commands.".into()
                }
                Err(error) => format!("Could not inspect stack presets: {error:#}"),
            };
            Check {
                id: "review_gate",
                ok: false,
                hard: false,
                message: format!("project {name} has no gate configured"),
                fix: Some(format!(
                    "Set projects.{name}.review.gate in corral.toml. {advice}"
                )),
            }
        })
        .collect()
}

/// Every template and profile instruction a tiered run could read: `(surface, path, text)`.
/// Templates are workspace files and profiles are workspace-wide, so each tiered project scans the
/// same sources against its own full gate commands.
fn instruction_sources(store: &Store) -> (Vec<(&'static str, String, String)>, Vec<Check>) {
    let config = &store.config;
    let mut sources = Vec::new();
    let mut failures = Vec::new();
    let mut files: Vec<(&'static str, PathBuf)> =
        vec![("run_template", store.prompts_dir().join("run.md"))];
    for harness in config.harnesses.values() {
        if let Some(prompt) = &harness.prompt {
            files.push(("harness_template", store.root().join(prompt)));
        }
    }
    files.push(("review_template", store.prompts_dir().join("review.md")));
    for (surface, file) in files {
        match std::fs::read_to_string(&file) {
            Ok(text) => sources.push((surface, file.display().to_string(), text)),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => failures.push(Check {
                id: "gate_instructions",
                ok: false,
                hard: false,
                message: format!("could not read {}: {e}", file.display()),
                fix: None,
            }),
        }
    }
    for (name, profile) in &config.profiles {
        let Some(metadata) = &profile.metadata else {
            continue;
        };
        let roles = &metadata.eligibility.roles;
        let path = format!("profiles.{name}.metadata.instructions");
        if roles.contains(&ProfileRole::Implementation) || roles.contains(&ProfileRole::Planning) {
            sources.push((
                "worker_instructions",
                path.clone(),
                metadata.instructions.clone(),
            ));
        }
        if roles.contains(&ProfileRole::ImplementationReview)
            || roles.contains(&ProfileRole::PlanReview)
        {
            sources.push(("reviewer_instructions", path, metadata.instructions.clone()));
        }
    }
    (sources, failures)
}

/// In a tiered project a full-gate directive in a template or a profile's working instructions
/// refuses dispatch (worker surfaces) or review launch (reviewer surfaces). Doctor reports each
/// file or profile and line before any dispatch is attempted; the refusal at the launch boundary
/// remains the control.
fn instruction_conflicts(store: &Store) -> Vec<Check> {
    let tiered: Vec<(&String, Vec<String>)> = store
        .config
        .projects
        .keys()
        .filter_map(|name| {
            let policy = store.config.review_policy(Some(name), None);
            (!policy.gate_tiers.is_empty()).then_some((name, policy.gate))
        })
        .collect();
    if tiered.is_empty() {
        return Vec::new();
    }
    let (sources, mut checks) = instruction_sources(store);
    let mut found: BTreeMap<(String, String, usize, String), Vec<&str>> = BTreeMap::new();
    for (project, gate) in &tiered {
        for (surface, path, text) in &sources {
            if let Some((line, phrase)) = scan_gate_instructions(text, gate) {
                found
                    .entry((surface.to_string(), path.clone(), line, phrase))
                    .or_default()
                    .push(project);
            }
        }
    }
    checks.extend(
        found
            .into_iter()
            .map(|((surface, path, line, phrase), projects)| {
                let refuses = match surface.as_str() {
                    "review_template" | "reviewer_instructions" => "review launch",
                    _ => "dispatch",
                };
                let conflict = GateInstructionConflict {
                    surface,
                    path: path.clone(),
                    line,
                    phrase,
                };
                Check {
                    id: "gate_instructions",
                    ok: false,
                    hard: false,
                    message: format!(
                        "{conflict}; this will refuse {refuses} in tiered project(s) {}",
                        projects.join(", ")
                    ),
                    fix: Some(format!(
                        "Remove the full-gate directive from {path}: corral appends the authoritative Gate contract section to every tiered run."
                    )),
                }
            }),
    );
    checks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(policy: &str) -> (tempfile::TempDir, Store) {
        let root = tempfile::tempdir().unwrap();
        let mut store = Store::init(root.path()).unwrap();
        store.config =
            toml::from_str(&format!("{policy}\n[projects.app]\npath = \".\"\n")).unwrap();
        (root, store)
    }

    #[test]
    fn unconfigured_manual_projects_do_not_warn() {
        let (_root, store) = fixture("");
        assert!(checks(&store).is_empty());
    }

    #[test]
    fn explicit_global_policy_without_gate_warns_with_suggestions() {
        let (root, store) = fixture("[review]\naccept = \"owner\"");
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"",
        )
        .unwrap();
        let checks = checks(&store);
        assert_eq!(checks.len(), 1);
        assert!(!checks[0].ok);
        assert!(!checks[0].hard);
        assert!(checks[0].fix.as_ref().unwrap().contains("cargo test"));
        assert!(
            store
                .config
                .review_policy(Some("app"), None)
                .gate
                .is_empty()
        );
    }

    #[test]
    fn project_policy_with_cleared_gate_warns_only_for_that_project() {
        let (_root, mut store) = fixture("[review]\ngate = [\"true\"]");
        store.config.projects.get_mut("app").unwrap().review.gate = Some(Vec::new());
        assert_eq!(checks(&store).len(), 1);
    }

    #[test]
    fn configured_gate_needs_no_advice() {
        let (_root, store) = fixture("[review]\ngate = [\"true\"]");
        assert!(checks(&store).is_empty());
    }

    #[test]
    fn explicit_project_manual_policy_without_gate_warns() {
        let (_root, mut store) = fixture("");
        store
            .config
            .projects
            .get_mut("app")
            .unwrap()
            .review
            .remove_worktree = Some(true);
        assert_eq!(checks(&store).len(), 1);
    }

    const TIERED: &str = "[review]\ngate = [\"scripts/gate.sh\"]\n[review.gate_tiers]\nquick = [\"true\"]\n[harnesses.h]\ncommand = \"true\"\nprompt = \"h.md\"\n";

    fn profile(roles: &[ProfileRole], instructions: &str) -> corral_core::Profile {
        corral_core::Profile {
            metadata: Some(corral_core::ProfileMetadata {
                display_name: "p".into(),
                purpose: "p".into(),
                guidance: Default::default(),
                instructions: instructions.into(),
                eligibility: corral_core::ProfileEligibility {
                    enabled: true,
                    automatic: true,
                    roles: roles.iter().copied().collect(),
                    projects: Default::default(),
                },
                revision: "1".into(),
                evidence: None,
                candidates: Vec::new(),
                fallback: None,
            }),
            harness: "h".into(),
            model: None,
            vendor: None,
            escalate_to: None,
            effort: None,
            args: Vec::new(),
        }
    }

    fn write(store: &Store, rel: &str, text: &str) {
        let path = store.root().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn conflicts(store: &Store) -> Vec<Check> {
        checks(store)
            .into_iter()
            .filter(|check| check.id == "gate_instructions")
            .collect()
    }

    #[test]
    fn a_tiered_project_reports_each_conflicting_template_and_profile_with_its_line() {
        let (_root, mut store) = fixture(TIERED);
        write(
            &store,
            ".corral/prompts/run.md",
            "# {{id}}\n\nRun the full gate.\n",
        );
        write(&store, "h.md", "{{id}}\nthen run scripts/gate.sh\n");
        write(
            &store,
            ".corral/prompts/review.md",
            "ok\nok\nRe-run the gate yourself.\n",
        );
        store.config.profiles.insert(
            "worker".into(),
            profile(
                &[ProfileRole::Implementation],
                "Be brief.\nRun full gate before done.",
            ),
        );
        store.config.profiles.insert(
            "judge".into(),
            profile(
                &[ProfileRole::ImplementationReview],
                "Run the full gate first.",
            ),
        );

        let messages: Vec<String> = conflicts(&store)
            .into_iter()
            .map(|check| {
                assert!(!check.ok && !check.hard);
                check.message
            })
            .collect();

        assert_eq!(messages.len(), 5, "{messages:#?}");
        for (surface, line, refuses) in [
            ("run_template", 3, "dispatch"),
            ("harness_template", 2, "dispatch"),
            ("review_template", 3, "review launch"),
            ("worker_instructions", 2, "dispatch"),
            ("reviewer_instructions", 1, "review launch"),
        ] {
            let needle = format!("{surface} ");
            let message = messages
                .iter()
                .find(|m| m.contains(&needle))
                .unwrap_or_else(|| panic!("{surface} missing from {messages:#?}"));
            assert!(message.contains(&format!("line {line} ")), "{message}");
            assert!(
                message.contains(&format!("will refuse {refuses} in tiered project(s) app")),
                "{message}"
            );
        }
    }

    #[test]
    fn clean_untiered_and_reduced_tier_instructions_report_nothing() {
        let (_root, store) = fixture(TIERED);
        write(&store, ".corral/prompts/run.md", "Gate:\n{{gate}}\n");
        write(
            &store,
            "h.md",
            "Use scripts/gate.sh --tier quick when told.\n",
        );
        assert!(conflicts(&store).is_empty());

        let (_root, store) = fixture("[review]\ngate = [\"scripts/gate.sh\"]\n");
        write(&store, ".corral/prompts/run.md", "Run the full gate.\n");
        assert!(conflicts(&store).is_empty());
    }
}
