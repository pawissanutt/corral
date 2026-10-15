//! Missing-gate advice is read-only, including when doctor runs with --fix.

use corral_core::{Store, detect_gate_presets};

use super::Check;

pub(super) fn checks(store: &Store) -> Vec<Check> {
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
}
