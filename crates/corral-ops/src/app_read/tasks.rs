//! Task details read the current head and keyed run evidence, never task history.
use anyhow::Result;
use corral_core::{StepState, TaskRef};
use corral_protocol::detail::{
    ArtifactRef, DetailBody, TaskDependency, TaskDetail, TaskHold, TaskProgress, TaskRun,
};

use super::short;
use crate::Ctx;

pub(super) fn task(ctx: &Ctx, id: &str) -> Result<DetailBody> {
    let head = ctx.store.head(id)?;
    let shell = head.shell_task()?;
    let dependencies = head
        .depends_on
        .iter()
        .map(|id| {
            let dependency = ctx.store.head(id)?;
            Ok(TaskDependency {
                id: dependency.id.clone(),
                title: short(&dependency.title, 160),
                status: dependency.status.as_str().into(),
                satisfied: dependency.status == corral_core::Status::Done,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let current = head.current_run.as_deref();
    let progress = current
        .and_then(|run| run.progress.as_ref())
        .map(|progress| TaskProgress {
            completed: count(
                progress
                    .steps()
                    .filter(|step| step.status == StepState::Done)
                    .count(),
            ),
            total: count(progress.steps().count()),
            text: Some(excerpt(&progress.summary(), 256)),
        });
    let artifacts = ctx
        .store
        .artifact_rows(&head.id, None)?
        .iter()
        .map(|artifact| artifact_ref(&head.id, artifact))
        .collect();
    let native_session =
        match current.and_then(|run| run.agent_host.as_ref().map(|host| (run, host))) {
            Some((run, host)) => head
                .agent_generation(
                    &corral_core::agent::ContainerRef::Worker {
                        task: head.id.clone(),
                        run: run.id.clone(),
                    },
                    host.binding.generation,
                )?
                .and_then(|generation| generation.bound.map(|bound| bound.session_id)),
            None => None,
        };
    let run = current.map(|run| TaskRun {
        id: run.id.clone(),
        session: native_session
            .or_else(|| run.session.as_ref().and_then(|session| session.id.clone())),
        profile: run
            .profile
            .clone()
            .or_else(|| (!run.agent.is_empty()).then(|| run.agent.clone())),
        model: run.model.clone(),
        effort: run.effort.clone(),
        base: (!run.base_sha.is_empty()).then(|| run.base_sha.clone()),
        head: candidate(run).map(str::to_owned),
        started_at: Some(run.started.to_string()),
        ended_at: run.ended.map(|at| at.to_string()),
    });
    let owner_next = match current {
        Some(run) => head
            .attention_obligations(&run.id)?
            .decisions
            .into_iter()
            .find(|decision| decision.pending())
            .map(|decision| excerpt(&decision.request.requested_action, 256)),
        None => None,
    };
    let context = super::plans::plan_context(ctx, &head)?;
    Ok(DetailBody::Task(TaskDetail {
        id: head.id.clone(),
        owner_revision: corral_core::owner_revision(&shell)?,
        title: excerpt(&head.title, 256),
        status: head.status.as_str().into(),
        body_excerpt: excerpt(&head.body, 6 * 1024),
        stages: crate::boardview::rail::steps(&head),
        dependencies,
        progress,
        owner_next,
        hold: head.hold.as_ref().map(|hold| TaskHold {
            reason: Some(excerpt(&hold.reason, 512)),
            since: Some(hold.at.to_string()),
            by: None,
        }),
        park: head.park.as_ref().map(|park| TaskHold {
            reason: Some(excerpt(&park.reason, 512)),
            since: Some(park.at.to_string()),
            by: Some(park.by.label().to_ascii_lowercase()),
        }),
        park_revision: head.park.as_ref().map(|park| park.revision),
        artifacts,
        run,
        plan_context: context.map(|(position, _)| position),
        log_tail: current
            .and_then(|run| run.blocked_excerpt.as_deref())
            .map(tail)
            .unwrap_or_default(),
    }))
}

pub(super) fn artifact_ref(task: &str, artifact: &corral_core::Artifact) -> ArtifactRef {
    ArtifactRef {
        source_ref: format!(
            "{task}:{}:{}",
            artifact.run,
            corral_core::snapshot_name(artifact)
        ),
        title: excerpt(&artifact.title, 256),
        media_type: None,
    }
}

pub(super) fn candidate(run: &corral_core::RunHead) -> Option<&str> {
    let checkpoint = run.facts.latest_checkpoint.as_ref();
    let stage = run
        .autopilot
        .as_ref()
        .filter(|stage| stage.stage != corral_core::AutoStage::Done && stage.sha.is_some());
    match (checkpoint, stage) {
        (Some(checkpoint), Some(stage)) if stage.at > checkpoint.captured => stage.sha.as_deref(),
        (Some(checkpoint), _) => Some(&checkpoint.head),
        (_, Some(stage)) => stage.sha.as_deref(),
        _ => None,
    }
}

/// Bound escaped JSON text as well as UTF-8; excerpts visibly identify omitted bytes.
pub(super) fn excerpt(value: &str, maximum: usize) -> String {
    let encoded_width = |character: char| match character {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
        character if character <= '\u{1f}' => 6,
        character => character.len_utf8(),
    };
    if value.chars().map(encoded_width).sum::<usize>() <= maximum {
        return value.to_owned();
    }
    let mut remaining = maximum.saturating_sub(18);
    let end = value
        .char_indices()
        .find_map(|(index, character)| {
            let width = encoded_width(character);
            if width > remaining {
                Some(index)
            } else {
                remaining -= width;
                None
            }
        })
        .unwrap_or(value.len());
    format!("{}\n… (truncated)", &value[..end])
}

fn tail(text: &str) -> Vec<String> {
    let mut lines: Vec<_> = text
        .lines()
        .rev()
        .take(40)
        .map(|line| excerpt(line, 256))
        .collect();
    lines.reverse();
    if text.lines().count() > lines.len() {
        lines.insert(0, "… (earlier recorded lines truncated)".into());
    }
    lines
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_core::{Status, Store};

    #[test]
    fn task_detail_uses_canonical_park_revision() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let store = Store::init(tmp.path())?;
        let mut owner = store.create("Parked", None, Status::Ready, "")?;
        owner.park = Some(serde_json::from_value(serde_json::json!({
            "version": 1, "revision": 47, "by": "owner", "reason": "Wait",
            "at": "2026-11-14T00:00:00Z", "resume_when": [],
            "resume_action": "Resume", "review_by": "2026-11-15T00:00:00Z"
        }))?);
        store.fixture_put_task(&owner)?;
        let ctx = crate::resources::FixedResourceProbe::ctx(store);

        let DetailBody::Task(detail) = task(&ctx, &owner.id)? else {
            anyhow::bail!("wrong detail body")
        };
        assert_eq!(
            detail.park_revision,
            owner.park.as_ref().map(|park| park.revision)
        );
        assert_eq!(detail.park_revision, Some(47));

        owner.park = None;
        ctx.store.fixture_put_task(&owner)?;
        let DetailBody::Task(detail) = task(&ctx, &owner.id)? else {
            anyhow::bail!("wrong detail body")
        };
        assert_eq!(detail.park_revision, None);
        Ok(())
    }
}
