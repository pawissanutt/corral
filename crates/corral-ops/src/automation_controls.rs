//! Owner-facing finite grant controls. Attribution flags are not authority.
use std::path::Path;

use anyhow::{Context, Result, ensure};
use corral_core::{OperatingPreview, OperatingSelection, Store};

use crate::command::Invoker;

/// Whether the caller runs in an agent or assistant shell (`AI_AGENT`, `PI_CODING_AGENT` or
/// `CORRAL_ASSISTANT`). Owner guards refuse every such entry, whatever attribution flags say,
/// unless the caller claimed a receipted owner action (`OwnerAuthority::ViaAssistant`).
pub fn agent_entry(invoker: &Invoker) -> bool {
    invoker.agent_entry()
}

/// Reject agent, assistant and PM entry points independently of caller-supplied attribution.
/// Like other local owner controls, this trusts the owner's OS session, not a CLI flag.
pub fn require_automation_owner(store: &Store, invoker: &Invoker) -> Result<()> {
    let agent = agent_entry(invoker);
    let pm = invoker.owner_action.is_none() && invoker.pm_marker();
    let cwd = invoker
        .cwd()
        .context("reading automation control directory")?;
    require_owner_entry(store, &cwd, agent, pm)
}

pub(crate) fn require_owner_entry(store: &Store, cwd: &Path, agent: bool, pm: bool) -> Result<()> {
    ensure!(
        !agent && !pm,
        "automation confirmation requires the owner, not a PM or agent"
    );
    let cwd = cwd
        .canonicalize()
        .context("resolving automation control directory")?;
    // Confirmation is an owner history command: past worktrees remain agent entry points.
    let view = store.view()?;
    let mut page = corral_core::backend::PageReq::first(500);
    loop {
        let tasks = view.tasks(
            &corral_core::backend::TaskQuery {
                statuses: vec![],
                project: None,
                order: corral_core::backend::TaskOrder::IdAsc,
            },
            page.clone(),
        )?;
        for header in tasks.items {
            let rows = view.rows(
                header.num,
                &corral_core::backend::RowQuery::part(corral_core::backend::TaskPart::Run),
                corral_core::backend::PageReq::first(u32::MAX),
            )?;
            for row in rows.items {
                corral_core::work::decoded(
                    corral_core::work::Counter::HistoryRowDecodes,
                    u64::try_from(serde_json::to_vec(&row.doc)?.len())?,
                    corral_core::work::Subject::Plain,
                );
                let path: std::path::PathBuf = serde_json::from_value(row.doc["worktree"].clone())?;
                if let Ok(worktree) = path.canonicalize() {
                    ensure!(
                        !cwd.starts_with(worktree),
                        "automation confirmation is unavailable from an agent run worktree"
                    );
                }
            }
        }
        match tasks.next {
            Some(next) => page.after = Some(next),
            None => break,
        }
    }
    Ok(())
}

/// Resolve exactly what the owner will review; this never publishes authority.
pub fn automation_preview(
    store: &Store,
    selection: &OperatingSelection,
) -> Result<OperatingPreview> {
    store.preview_operating_policy(selection)
}

/// The caller must obtain the digest through explicit owner input, never synthesize it.
pub fn automation_confirm(
    store: &Store,
    preview: &OperatingPreview,
    owner_digest: &str,
    invoker: &Invoker,
) -> Result<corral_core::Config> {
    require_automation_owner(store, invoker)?;
    ensure!(
        owner_digest == preview.id,
        "confirmation digest does not match the exact preview"
    );
    let config = store.confirm_operating_policy(preview, "owner", corral_core::clock::now())?;
    let _owner = store.attribute(corral_core::Actor::Owner);
    store
        .log_event(
            None,
            "operating_policy_confirmed",
            serde_json::json!({
                "preview_id": preview.id,
                "grant_digest": preview.grant_digest,
                "revision": config.automation.revision,
                "tasks": preview.policy.scope.tasks.keys().collect::<Vec<_>>(),
                "authority": preview.policy.authority,
            }),
        )
        .context("recording owner operating-policy confirmation")?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pm_and_agent_entries_cannot_claim_owner() {
        // Given a local workspace and a claimed owner entry.
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        // When PM or agent provenance is present, no attribution can elevate it.
        for (agent, pm) in [(true, false), (false, true), (true, true)] {
            // Then owner controls fail closed.
            assert!(require_owner_entry(&store, root.path(), agent, pm).is_err());
        }
    }

    #[test]
    fn assistant_shell_is_refused_by_owner_guards_without_agent_markers() {
        // Given a child process whose only entry marker is CORRAL_ASSISTANT.
        let root = tempfile::tempdir().unwrap();
        Store::init(root.path()).unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "automation_controls::tests::assistant_guard_subprocess",
                "--nocapture",
            ])
            .env("ASSISTANT_GUARD_ROOT", root.path())
            .env("CORRAL_ASSISTANT", "A-1")
            .current_dir(root.path());
        for key in ["AI_AGENT", "PI_CODING_AGENT", "CORRAL_PM"] {
            command.env_remove(key);
        }
        // When the automation and plan member-gate owner guards run there.
        let output = command.stdin(std::process::Stdio::null()).output().unwrap();
        // Then the child ran exactly that test and every guard refused.
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn assistant_guard_subprocess() {
        let Ok(root) = std::env::var("ASSISTANT_GUARD_ROOT") else {
            return;
        };
        let store = Store::open(Path::new(&root)).unwrap();
        let invoker = Invoker::process();
        assert!(agent_entry(&invoker));
        assert!(require_automation_owner(&store, &invoker).is_err());
        let entry = crate::plan::member_gate::OwnerEntry::from_env().unwrap();
        assert!(entry.agent && !entry.pm);
        assert!(require_owner_entry(&store, &entry.cwd, entry.agent, entry.pm).is_err());
    }

    #[test]
    fn persisted_agent_worktree_rejects_owner_entry_without_environment_markers() {
        // Given a persisted agent run and a nested working directory.
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        let task = store
            .create("Agent work", None, corral_core::Status::Backlog, "Work")
            .unwrap();
        let worktree = root.path().join("app/.worktrees").join(&task.id);
        let cwd = worktree.join("nested");
        std::fs::create_dir_all(&cwd).unwrap();
        let run: corral_core::Run = serde_json::from_value(serde_json::json!({
            "id": "R1", "agent": "generic", "worktree": worktree,
            "branch": "corral/fixture", "base": "main", "base_sha": "fixture",
            "started": "2026-10-18T00:00:00Z"
        }))
        .unwrap();
        store
            .update(&task.id, |task| {
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
        // When a caller without PM/agent environment markers claims an owner entry.
        let result = require_owner_entry(&store, &cwd, false, false);
        // Then persisted run provenance still prevents elevation.
        assert!(result.is_err());
    }
}
