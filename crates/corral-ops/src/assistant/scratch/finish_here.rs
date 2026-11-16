//! Owner-instructed, task-independent scratch verification, landing and build recording.
use anyhow::{Context, Result, ensure};
use corral_core::build_record::{
    self as wire, BuildRecord, RecordFinding, RecordMessage, RecordReview, RecordSlice,
    ReviewVerdict,
};
use corral_core::config::BuildExit;
use serde::{Deserialize, Serialize};

use super::{find, git, holder, lock, repo_of};
use crate::Ctx;
use crate::assistant::owner_action::{self, BuildAuthority, OwnerAuthority};
use crate::build_session::{gate, land, record};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttachedReview {
    pub reviewer: String,
    pub model: String,
    pub verdict: ReviewVerdict,
    pub summary: String,
    pub findings: Vec<RecordFinding>,
    pub head: Option<String>,
    pub at: String,
}

/// Evidence is read only from the host's reserved operation payload, never command arguments.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostEvidence {
    #[serde(default)]
    pub review: Option<AttachedReview>,
    #[serde(default)]
    pub slices: Vec<RecordSlice>,
    #[serde(default)]
    pub messages: Vec<RecordMessage>,
}

pub struct FinishHere<'a> {
    pub title: &'a str,
    pub expected_head: &'a str,
    pub override_reason: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum FinishedHere {
    Landed {
        landed: String,
        base_branch: String,
        gate_attempt: String,
        record: record::Published,
    },
    GateFailed {
        landed: Option<String>,
        gate_attempt: String,
        gate: gate::GateReceipt,
    },
}

pub fn finish_here(
    ctx: &Ctx,
    session: &str,
    slug: &str,
    opts: &FinishHere<'_>,
) -> Result<FinishedHere> {
    let authority = owner_action::build_finish_authority(ctx, session)?;
    let (intent, evidence) = match owner_action::authority() {
        OwnerAuthority::ViaAssistant(via) => {
            let container = ctx.store.get_assistant_container(session)?;
            let receipt = crate::agent::assistant_receipt(
                &ctx.store,
                session,
                &container.state,
                &via.request,
            )?
            .context("finish_here owner-action reservation disappeared")?;
            let evidence = serde_json::from_value(receipt.result["finish_here_evidence"].clone())
                .context("reading host-attached finish_here evidence")?;
            (via.owner_message.clone(), evidence)
        }
        // The owner's own shell has no host review; review policy needs an explicit override.
        OwnerAuthority::Shell => (opts.title.to_owned(), HostEvidence::default()),
    };
    finish_authorized(ctx, session, slug, opts, &intent, evidence, &authority)
}

fn finish_authorized(
    ctx: &Ctx,
    session: &str,
    slug: &str,
    opts: &FinishHere<'_>,
    intent: &str,
    evidence: HostEvidence,
    authority: &BuildAuthority,
) -> Result<FinishedHere> {
    let _guard = lock(ctx, &format!("scratch-{session}-{slug}"))?;
    let scratch = find(ctx, session, slug)?;
    ensure!(
        holder(&ctx.store.list()?, &scratch).is_none(),
        "scratch_attached: scratch already belongs to a task; nothing finished"
    );
    let repo = repo_of(&scratch)?;
    let head =
        corral_git::rev_sha(&scratch.path, "HEAD").context("reading scratch candidate HEAD")?;
    let expected = opts.expected_head;
    ensure!(
        (7..=64).contains(&expected.len()) && expected.bytes().all(|b| b.is_ascii_hexdigit()),
        "--expected-head takes a commit SHA of 7 to 64 hex digits"
    );
    ensure!(
        head.starts_with(&expected.to_ascii_lowercase()),
        "stale_candidate: {session}/{slug} is at {head}, not the candidate {expected} the owner was shown"
    );
    ensure!(
        corral_git::is_clean(&scratch.path).context("checking scratch candidate")?,
        "scratch_dirty: commit or remove uncommitted changes before finishing"
    );
    let project = ctx
        .store
        .config
        .projects
        .get(&scratch.project)
        .context("scratch project disappeared")?;
    let policy = project.build_exit.unwrap_or(BuildExit::Review);
    ensure!(
        policy != BuildExit::None,
        "in_session_finish_disabled: project build_exit is none"
    );
    let override_reason = opts
        .override_reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty());
    let approved = evidence.review.as_ref().is_some_and(|review| {
        review.verdict == ReviewVerdict::Approve
            && review.head.as_deref() == Some(head.as_str())
            && review.findings.is_empty()
    });
    ensure!(
        policy != BuildExit::Review || approved || override_reason.is_some(),
        "review_required: finish_here needs a host-attached approving review of {head} without open findings, or the owner's --override-reason"
    );
    let id = format!("{session}-{slug}");
    crate::build_session::validate_session_id(&id)?;
    let work = git::work(&repo, &scratch)?;
    let stat =
        corral_git::diff_stat(&scratch.path, &scratch.base).context("reading scratch diff")?;
    let commits: Vec<_> = work
        .commits
        .into_iter()
        .rev()
        .map(|(sha, subject)| wire::RecordCommit {
            sha,
            subject,
            slice: None,
        })
        .collect();
    let reviews: Vec<RecordReview> = evidence
        .review
        .into_iter()
        .map(|review| RecordReview {
            round: 1,
            reviewer: review.reviewer,
            model: review.model,
            head: review.head.unwrap_or_default(),
            verdict: review.verdict,
            at: review.at,
            summary: Some(review.summary),
            findings: review.findings,
        })
        .collect();
    let now = corral_core::clock::now().to_string();
    let zero = wire::RecordUsage {
        input: 0,
        output: 0,
        cost: None,
    };
    let base = match &project.base {
        Some(base) => base.clone(),
        None => corral_git::current_branch(&repo).context("reading assistant build base branch")?,
    };
    let mut document = BuildRecord {
        version: 1,
        id,
        kind: wire::BuildKind::Assistant,
        project: Some(scratch.project.clone()),
        title: opts.title.to_owned(),
        intent: vec![wire::RecordIntent {
            at: now.clone(),
            text: intent.to_owned(),
        }],
        created: scratch.created.to_string(),
        updated: now,
        repo_root: repo.to_string_lossy().into_owned(),
        worktree: scratch.path.to_string_lossy().into_owned(),
        branch: Some(scratch.branch.clone()),
        base: Some(base),
        outcome: wire::RecordOutcome {
            state: wire::OutcomeState::Open,
            at: None,
            head: None,
            landed: None,
            base_branch: None,
            override_reason: None,
        },
        diff: Some(wire::RecordDiff {
            commits: commits.len() as u64,
            files: stat.files.len() as u64,
            added: stat.added,
            removed: stat.removed,
        }),
        commits,
        slices: evidence.slices,
        verification: Vec::new(),
        reviews,
        messages: evidence.messages,
        decisions: Vec::new(),
        usage: wire::RecordUsageTotals {
            parent: zero.clone(),
            children: zero.clone(),
            reviewers: zero.clone(),
            total: zero,
        },
        timeline: Vec::new(),
        transcripts: wire::RecordTranscripts {
            parent: None,
            children: Default::default(),
        },
    };
    // Validate all host evidence and owner text before any landing effect.
    document.validate().map_err(anyhow::Error::msg)?;
    let gate = gate::run_authorized(
        ctx,
        &gate::GateRequest {
            session: &document.id,
            worktree: &scratch.path,
            project: &scratch.project,
            tier: "full",
            base: Some(&scratch.base),
        },
        authority,
    )?;
    if gate.result == "fail" {
        return Ok(FinishedHere::GateFailed {
            landed: None,
            gate_attempt: gate.attempt.clone(),
            gate,
        });
    }
    let landed = land::run_authorized(
        ctx,
        &land::LandRequest {
            session: &document.id,
            worktree: &scratch.path,
            project: &scratch.project,
            expected_head: &head,
            gate_attempt: &gate.attempt,
        },
        authority,
    )?;
    let at = corral_core::clock::now().to_string();
    document.updated = at.clone();
    document.outcome = wire::RecordOutcome {
        state: wire::OutcomeState::Landed,
        at: Some(at.clone()),
        head: Some(head),
        landed: Some(landed.landed.clone()),
        base_branch: Some(landed.base_branch.clone()),
        override_reason: override_reason.map(str::to_owned),
    };
    document.timeline = vec![
        wire::RecordTimeline {
            at: gate.started.clone(),
            phase: wire::TimelinePhase::Verify,
            detail: Some(gate.attempt.clone()),
        },
        wire::RecordTimeline {
            at,
            phase: wire::TimelinePhase::Land,
            detail: Some(landed.landed.clone()),
        },
    ];
    document.verification.push(wire::RecordVerification {
        attempt: gate.attempt.clone(),
        tier: gate.tier,
        head: gate.head,
        result: if gate.result == "skipped" {
            wire::GateResult::Skipped
        } else {
            wire::GateResult::Pass
        },
        failed_command: gate.failed_command,
        started: gate.started,
        ended: gate.ended,
        log: gate.log,
    });
    let published = record::publish_authorized(ctx, &serde_json::to_string(&document)?, authority)?;
    // Match landed scratch retirement: keep a failed-cleanup scratch discoverable, never
    // forget a directory/branch before cleanup has actually succeeded.
    let cleanup = crate::cleanup_worktree::remove_scratch(
        &repo,
        &scratch.path,
        &scratch.branch,
        crate::cleanup_worktree::Callers::Exempt,
    );
    let cleaned = cleanup.is_ok();
    ctx.store
        .update_assistant(session, |record| {
            if record.linked_scratch.as_deref() == Some(slug) {
                record.linked_scratch = None;
            }
            if cleaned {
                record.scratch.retain(|entry| entry.slug != slug);
            }
            Ok(())
        })
        .context("retiring landed assistant scratch")?;
    if let Err(error) = cleanup {
        crate::messages::report(
            None,
            "assistant",
            format!("landed; scratch cleanup failed: {error:#}"),
        );
    }
    Ok(FinishedHere::Landed {
        landed: landed.landed,
        base_branch: landed.base_branch,
        gate_attempt: gate.attempt,
        record: published,
    })
}

#[cfg(all(test, unix))]
#[path = "finish_here_tests.rs"]
mod tests;
