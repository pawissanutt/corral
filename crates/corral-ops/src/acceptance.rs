//! The "Acceptance record" artifact the autopilot writes for every task it lands: gate log, verdict and notes,
//! rounds, landed sha and the policy used.

use anyhow::{Context, Result};
use corral_core::{Accept, Artifact, ArtifactKind, MergeMode, ReviewPolicy, Reviewer, Run, Task};
use corral_git::DiffStat;
use jiff::Timestamp;

use crate::Ctx;
use crate::artifacts::add_artifact;

pub(crate) struct Landing<'a> {
    pub policy: &'a ReviewPolicy,
    /// The base's sha after landing.
    pub sha: &'a str,
    pub line: &'a str,
    /// The landed change relative to the base it landed on.
    pub stat: &'a DiffStat,
    pub at: Timestamp,
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("{s:?}"))
}

fn latest_artifact(task: &Task, run: &Run, kind: ArtifactKind, title_prefix: &str) -> String {
    task.artifacts
        .iter()
        .rev()
        .find(|a| a.run == run.id && a.kind == kind && a.title.starts_with(title_prefix))
        .map(|a| format!("; artifact `{}`", a.path))
        .unwrap_or_default()
}

fn policy_toml(p: &ReviewPolicy) -> String {
    let list = |v: &[String]| {
        let items: Vec<String> = v.iter().map(|s| quoted(s)).collect();
        format!("[{}]", items.join(", "))
    };
    let reviewer = match &p.reviewer {
        Reviewer::None => "none".to_string(),
        Reviewer::Pm => "pm".to_string(),
        Reviewer::CrossVendor => "cross-vendor".to_string(),
        Reviewer::Profile(name) => name.clone(),
    };
    let accept = match p.accept {
        Accept::Owner => "owner",
        Accept::Auto => "auto",
    };
    let merge = match p.merge {
        MergeMode::Rebase => "rebase",
        MergeMode::Ff => "ff",
    };
    [
        format!("gate = {}", list(&p.gate)),
        format!("reviewer = {}", quoted(&reviewer)),
        format!("accept = {}", quoted(accept)),
        format!("merge = {}", quoted(merge)),
        format!("max_rounds = {}", p.max_rounds),
        format!("max_diff_lines = {}", p.max_diff_lines),
        format!("owner_paths = {}", list(&p.owner_paths)),
        format!("remove_worktree = {}", p.remove_worktree),
    ]
    .iter()
    .map(|l| format!("    {l}\n"))
    .collect()
}

fn render(task: &Task, run: &Run, landing: &Landing) -> String {
    let p = landing.policy;
    let gate = match (&run.gate, p.gate.is_empty()) {
        (_, true) => "none configured".to_string(),
        (Some(g), false) => format!(
            "{} at `{}`{}",
            g.result.as_str(),
            short(&g.sha),
            latest_artifact(task, run, ArtifactKind::Log, "Gate:")
        ),
        (None, false) => "configured, but no gate result was recorded".to_string(),
    };
    let review = match (&run.review, &p.reviewer) {
        (_, Reviewer::None) => "none configured".to_string(),
        (Some(r), _) => format!(
            "{} by {} at `{}`{}",
            r.verdict.map_or("no verdict", |v| v.as_str()),
            r.reviewer,
            short(&r.sha),
            latest_artifact(task, run, ArtifactKind::Report, "Review (")
        ),
        (None, _) => "configured, but no review was recorded".to_string(),
    };
    let stat = landing.stat;
    let mut out = format!(
        "# Acceptance record: {} {}\n\nLanded by corral's autopilot at {}, with no owner action.\n\n",
        task.id, task.title, landing.at
    );
    for (label, value) in [
        (
            "Run",
            format!(
                "{} on branch `{}` (profile {})",
                run.id, run.branch, run.agent
            ),
        ),
        (
            "Landed",
            format!("`{}` at `{}`: {}", run.base, landing.sha, landing.line),
        ),
        (
            "Rounds",
            format!(
                "{} change round(s) of max_rounds {}",
                task.rounds, p.max_rounds
            ),
        ),
        ("Gate", gate),
        ("Review", review),
        (
            "Diff",
            format!(
                "+{} \u{2212}{} in {} file(s)",
                stat.added,
                stat.removed,
                stat.files.len()
            ),
        ),
    ] {
        out.push_str(&format!("- {label}: {value}\n"));
    }
    out.push_str("\n## Policy used\n\n");
    out.push_str(&policy_toml(p));
    if let Some(notes) = run.review.as_ref().and_then(|r| r.notes.as_deref()) {
        out.push_str(&format!("\n## Reviewer notes\n\n{}\n", notes.trim_end()));
    }
    out
}

/// Writes the acceptance record of run `run_id` and snapshots it as an Evidence artifact "Acceptance record".
pub(crate) fn record_acceptance(
    ctx: &Ctx,
    id: &str,
    run_id: &str,
    landing: &Landing,
) -> Result<Artifact> {
    let task = ctx.store.get(id)?;
    let run = task
        .runs
        .iter()
        .find(|r| r.id == run_id)
        .with_context(|| format!("{id}: run {run_id} not found"))?;
    let tmp = ctx
        .store
        .state_dir()
        .join("tmp")
        .join(format!("{id}-{run_id}-acceptance"));
    std::fs::create_dir_all(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    let file = tmp.join("acceptance-record.md");
    let added = std::fs::write(&file, render(&task, run, landing))
        .with_context(|| format!("writing {}", file.display()))
        .and_then(|()| {
            add_artifact(
                ctx,
                id,
                &file,
                Some(ArtifactKind::Evidence),
                Some("Acceptance record"),
            )
        });
    if let Err(e) = std::fs::remove_dir_all(&tmp) {
        crate::messages::report(
            Some(id),
            "cleanup",
            format!("could not remove {} (ignored): {e}", tmp.display()),
        );
    }
    added
}
