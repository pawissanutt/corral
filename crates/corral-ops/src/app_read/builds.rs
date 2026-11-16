//! Published builds are not tasks. Read canonical records on one pinned Store view;
//! omit evidence explicitly with source counts rather than scanning logs/transcripts.
use anyhow::{Context, Result, ensure};
use corral_core::build_record::{BuildRecord, BuildRecordSummary, RecordMessage, RecordUsage};
use corral_protocol::{app::APP_CONTROL_MAX, detail::*, rows::BuildRow};
use serde::Serialize;

use crate::Ctx;

/// Compact records, ordered by the Store's newest-updated instant and ID tie-break.
pub fn builds_rows(ctx: &Ctx) -> Result<Vec<BuildRow>> {
    ctx.read_step(|ctx| {
        Ok(ctx
            .store
            .list_build_records()?
            .into_iter()
            .map(row)
            .collect())
    })
}

fn row(record: BuildRecordSummary) -> BuildRow {
    BuildRow {
        id: record.id,
        kind: record.kind.as_str().into(),
        project: record.project,
        title: record.title,
        state: record.state.as_str().into(),
        created: record.created,
        updated: record.updated,
        landed: record.landed,
        override_reason: record.override_reason,
        slices: record.slices as u64,
        review_rounds: record.review_rounds as u64,
        gate_attempts: record.gate_attempts as u64,
        gate_failures: record.gate_failures as u64,
        messages: record.messages as u64,
        cost_usd: record.usage_total.cost,
        tokens: tokens(&record.usage_total),
        rev: record.revision,
    }
}

/// One byte-bounded record, with original collection totals and no local locators.
pub fn build_detail(ctx: &Ctx, id: &str) -> Result<BuildDetail> {
    ctx.read_step(|ctx| {
        let record = ctx
            .store
            .get_build_record(id)
            .with_context(|| format!("reading app build {id}"))?;
        // The canonical get API returns the document, not its storage revision. Keep
        // the summary lookup on this same snapshot so rows and detail agree exactly.
        let rev = ctx
            .store
            .list_build_records()?
            .into_iter()
            .find(|record| record.id == id)
            .with_context(|| format!("build record {id} has no summary"))?
            .revision;
        let mut detail = project(record, rev)?;
        bound(&mut detail)?;
        Ok(detail)
    })
}

fn project(record: BuildRecord, rev: u64) -> Result<BuildDetail> {
    let intent_total = record.intent.len() as u64;
    let commits_total = record.commits.len() as u64;
    let slices_total = record.slices.len() as u64;
    let verification_total = record.verification.len() as u64;
    let reviews_total = record.reviews.len() as u64;
    let messages_total = record.messages.len() as u64;
    let decisions_total = record.decisions.len() as u64;
    let timeline_total = record.timeline.len() as u64;
    let commits_skip = record.commits.len().saturating_sub(50);
    let messages = messages(record.messages)?;
    Ok(BuildDetail {
        id: record.id,
        kind: record.kind.as_str().into(),
        project: record.project,
        title: record.title,
        rev,
        state: record.outcome.state.as_str().into(),
        outcome: BuildOutcome {
            state: record.outcome.state.as_str().into(),
            at: record.outcome.at,
            head: record.outcome.head,
            landed: record.outcome.landed,
            base_branch: record.outcome.base_branch,
            override_reason: record.outcome.override_reason,
        },
        branch: record.branch,
        base: record.base,
        intent: record
            .intent
            .into_iter()
            .map(|intent| BuildIntent {
                at: intent.at,
                text: intent.text,
            })
            .collect(),
        intent_total,
        diff: record.diff.map(|diff| BuildDiff {
            commits: diff.commits,
            files: diff.files,
            added: diff.added,
            removed: diff.removed,
        }),
        commits: record
            .commits
            .into_iter()
            .skip(commits_skip)
            .map(|commit| BuildCommit {
                sha: commit.sha,
                subject: commit.subject,
                slice: commit.slice,
            })
            .collect(),
        commits_total,
        slices: record
            .slices
            .into_iter()
            .map(|slice| BuildSlice {
                id: slice.id,
                title: slice.title,
                kind: slice.kind.as_str().into(),
                model: slice.model,
                effort: slice.effort,
                state: slice.state.as_str().into(),
                started: slice.started,
                ended: slice.ended,
                tokens: tokens(&slice.usage.usage),
                cost_usd: slice.usage.usage.cost,
                turns: slice.usage.turns,
                outside: slice.outside,
                report: slice.report,
            })
            .collect(),
        slices_total,
        verification: record
            .verification
            .into_iter()
            .map(|gate| BuildVerification {
                attempt: gate.attempt,
                tier: gate.tier,
                head: gate.head,
                result: gate.result.as_str().into(),
                failed_command: gate.failed_command,
                started: gate.started,
                ended: gate.ended,
            })
            .collect(),
        verification_total,
        reviews: record
            .reviews
            .into_iter()
            .map(|review| BuildReview {
                round: review.round,
                reviewer: review.reviewer,
                model: review.model,
                head: review.head,
                verdict: review.verdict.as_str().into(),
                at: review.at,
                summary: review.summary,
                findings: review
                    .findings
                    .into_iter()
                    .map(|finding| BuildFinding {
                        id: finding.id,
                        severity: finding.severity.as_str().into(),
                        path: finding.path,
                        line: finding.line,
                        text: finding.text,
                        resolution: finding.resolution.map(|resolution| BuildResolution {
                            state: resolution.state.as_str().into(),
                            commit: resolution.commit,
                            reason: resolution.reason,
                        }),
                    })
                    .collect(),
            })
            .collect(),
        reviews_total,
        messages,
        messages_total,
        decisions: record
            .decisions
            .into_iter()
            .map(|decision| BuildDecision {
                at: decision.at,
                kind: decision.kind.as_str().into(),
                text: decision.text,
                answer: decision.answer,
            })
            .collect(),
        decisions_total,
        usage: BuildUsageTotals {
            parent: usage(record.usage.parent),
            children: usage(record.usage.children),
            reviewers: usage(record.usage.reviewers),
            total: usage(record.usage.total),
        },
        timeline: record
            .timeline
            .into_iter()
            .map(|entry| BuildTimeline {
                at: entry.at,
                phase: entry.phase.as_str().into(),
                detail: entry.detail,
            })
            .collect(),
        timeline_total,
    })
}

fn messages(source: Vec<RecordMessage>) -> Result<Vec<BuildMessage>> {
    let mut dated = source
        .into_iter()
        .map(|message| {
            let at = message
                .at
                .parse::<jiff::Timestamp>()
                .context("parsing build message timestamp")?;
            Ok((at, message))
        })
        .collect::<Result<Vec<_>>>()?;
    // Stable ties retain canonical source order; offsets compare as instants.
    dated.sort_by_key(|(at, _)| *at);
    let skip = dated.len().saturating_sub(100);
    Ok(dated
        .into_iter()
        .skip(skip)
        .map(|(_, message)| BuildMessage {
            at: message.at,
            from: message.from,
            to: message.to,
            text: message.text,
            delivery: message.delivery.as_str().into(),
        })
        .collect())
}

fn tokens(usage: &RecordUsage) -> u64 {
    usage.input.saturating_add(usage.output)
}

fn usage(usage: RecordUsage) -> BuildUsage {
    BuildUsage {
        input: usage.input,
        output: usage.output,
        cost_usd: usage.cost,
    }
}

/// Use the same envelope reserve as plan detail paging, including the Build body
/// tag. Lists are atomic here: no partial finding, report or message is presented.
/// Budget pressure keeps a contiguous recent suffix, with totals exposing omissions.
fn bound(detail: &mut BuildDetail) -> Result<()> {
    let intent = std::mem::take(&mut detail.intent);
    let commits = std::mem::take(&mut detail.commits);
    let slices = std::mem::take(&mut detail.slices);
    let verification = std::mem::take(&mut detail.verification);
    let reviews = std::mem::take(&mut detail.reviews);
    let messages = std::mem::take(&mut detail.messages);
    let decisions = std::mem::take(&mut detail.decisions);
    let timeline = std::mem::take(&mut detail.timeline);
    // Count the actual tagged wire body without cloning the record.
    #[derive(Serialize)]
    struct Body<'a> {
        kind: &'static str,
        body: &'a BuildDetail,
    }
    let bytes = super::paging::json_len(&Body {
        kind: "build",
        body: detail,
    })?;
    let maximum = APP_CONTROL_MAX - 4096;
    ensure!(
        bytes <= maximum,
        "build record {} metadata exceeds detail byte budget",
        detail.id
    );
    let mut remaining = maximum - bytes;
    // Interactive evidence gets priority over potentially large reports/graphs.
    detail.messages = recent(messages, &mut remaining)?;
    detail.intent = recent(intent, &mut remaining)?;
    detail.decisions = recent(decisions, &mut remaining)?;
    detail.verification = recent(verification, &mut remaining)?;
    detail.reviews = recent(reviews, &mut remaining)?;
    detail.commits = recent(commits, &mut remaining)?;
    detail.slices = recent(slices, &mut remaining)?;
    detail.timeline = recent(timeline, &mut remaining)?;
    Ok(())
}

fn recent<T: Serialize>(mut items: Vec<T>, remaining: &mut usize) -> Result<Vec<T>> {
    items.reverse();
    let mut accepted = 0;
    for item in &items {
        // A comma per element is conservative for the first item of each list.
        let bytes = super::paging::json_len(item)?.saturating_add(1);
        if bytes > *remaining {
            break;
        }
        *remaining -= bytes;
        accepted += 1;
    }
    items.truncate(accepted);
    items.reverse();
    Ok(items)
}

#[cfg(test)]
#[path = "builds_tests.rs"]
mod tests;
