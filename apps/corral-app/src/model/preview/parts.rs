//! The parts a sample pi host serves besides its transcript, read as corrald reads them through
//! the host: its cards and host events (every host), and a build's children with their details
//! and conversations, jobs and their paged output, changed files and patches, usage, context,
//! plan, model catalog, commands, board and build record. A stopped host serves none of them,
//! as corrald answers `no running Pi host`. What the owner's actions change is in
//! `parts_ops.rs`; the sample build's content in `parts_sample.rs`.

use corral_protocol::detail::*;
use corral_protocol::session_state::{
    BoardBrief, Counts, PlanBrief, PlanModeBrief, SessionState, UsageBrief,
};
use corral_protocol::subscription::{
    SessionAgentMessage, SessionChild, SessionChildVerdict, SessionFleet,
};

use super::pi::{Host, resolve, scopes};
use super::{put, stamp};
use crate::model::daemon::{DetailCache, Feed, detail_key};

/// Every model the sample offers holds this many tokens of context.
pub(super) const WINDOW: u64 = 200_000;

/// What a stopped host answers for any part but its transcript.
const COLD: &str = "not_found: no running Pi host; resume the session first";

/// One child agent: its detail and its own transcript.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Child {
    pub(super) detail: SessionChildDetail,
    pub(super) transcript: Vec<SessionMessage>,
}

impl Child {
    /// Whether it still runs or waits on the owner.
    pub(super) fn live(&self) -> bool {
        matches!(self.detail.state.as_str(), "running" | "needs" | "queued")
    }
}

/// One background job: its row, and its output as the older bytes and the newest page.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Job {
    pub(super) job: SessionJob,
    pub(super) older: String,
    pub(super) newest: String,
}

/// What a build's host serves besides its transcript and cards.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Parts {
    pub(super) children: Vec<Child>,
    pub(super) messages: Vec<SessionAgentMessage>,
    pub(super) jobs: Vec<Job>,
    /// The session's own comparison.
    pub(super) changes: SessionChanges,
    pub(super) patches: Vec<PatchPage>,
    /// The session's own row; its turns follow the host's.
    pub(super) pane: UsageRow,
    /// Helpers the host counts apart from its children.
    pub(super) helpers: Vec<UsageRow>,
    pub(super) context_tokens: u64,
    pub(super) context: SessionContext,
    pub(super) plan: SessionPlan,
    /// Plan mode, while the session is in it, and its one plan document.
    pub(super) plan_mode: Option<PlanModeBrief>,
    pub(super) plan_doc: String,
    /// The catalog; the current model and effort are the row's.
    pub(super) models: SessionModels,
    pub(super) commands: SessionCommands,
    pub(super) board: BuildBoard,
    pub(super) record: BuildRecord,
    pub(super) events: Vec<HostEvent>,
}

impl Parts {
    pub(super) fn child_mut(&mut self, id: &str) -> Option<&mut Child> {
        self.children.iter_mut().find(|child| child.detail.id == id)
    }

    /// Records a host event at `at`.
    pub(super) fn event(&mut self, at: &str, kind: &str, text: String) {
        let seq = self.events.last().map_or(1, |event| event.seq + 1);
        self.events.push(HostEvent {
            seq,
            at: at.into(),
            kind: kind.into(),
            text,
        });
    }

    /// The effort levels model `model` offers.
    pub(super) fn levels(&self, model: Option<&str>) -> Vec<String> {
        self.models
            .models
            .iter()
            .find(|choice| Some(choice.id.as_str()) == model)
            .map(|choice| choice.effort_levels.clone())
            .unwrap_or_default()
    }

    /// The fleet corrald's live page carries for this host.
    pub(super) fn fleet(&self) -> SessionFleet {
        SessionFleet {
            children: self.children.iter().map(row).collect(),
            messages: self.messages.clone(),
            messages_total: self.messages.len() as u64,
            max_parallel: 3,
        }
    }

    /// One usage row per agent: the session's own, each child's, each helper's.
    fn rows(&self, turns: u64) -> Vec<UsageRow> {
        let mut rows = vec![UsageRow {
            turns,
            ..self.pane.clone()
        }];
        rows.extend(self.children.iter().map(|child| {
            let usage = &child.detail.usage;
            UsageRow {
                key: format!("child:{}", child.detail.id),
                name: child.detail.title.clone(),
                kind: child.detail.kind.clone(),
                tokens: usage.input + usage.output,
                input: usage.input,
                output: usage.output,
                cost_usd: usage.cost_usd,
                context_percent: usage.context_percent,
                turns: usage.turns,
            }
        }));
        rows.extend(self.helpers.iter().cloned());
        rows
    }

    /// The total over every row; a row without a price leaves it partial.
    fn total(rows: &[UsageRow]) -> UsageTotalRow {
        let priced: Vec<f64> = rows.iter().filter_map(|row| row.cost_usd).collect();
        UsageTotalRow {
            tokens: rows.iter().map(|row| row.tokens).sum(),
            cost_usd: (!priced.is_empty()).then(|| priced.iter().sum()),
            partial: priced.len() < rows.len(),
        }
    }

    fn percent(&self) -> f32 {
        (self.context_tokens as f32 * 100.0 / WINDOW as f32).round()
    }

    /// The children at work, those waiting on the owner, and all of them.
    pub(super) fn children_counts(&self) -> Counts {
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Counts {
            live: count(self.children.iter().filter(|child| child.live()).count()),
            waiting: count(
                self.children
                    .iter()
                    .filter(|child| child.detail.question.is_some())
                    .count(),
            ),
            total: count(self.children.len()),
        }
    }

    /// The live state's counts and briefs: what the host runs, has changed, plans and spent.
    pub(super) fn brief(&self, state: &mut SessionState) {
        let total = Self::total(&self.rows(state.turn));
        state.usage = UsageBrief {
            tokens: self.pane.tokens,
            cost_usd: self.pane.cost_usd,
            cost_partial: total.partial,
            context_tokens: self.context_tokens,
            context_window: WINDOW,
            context_percent: Some(self.percent()),
        };
        state.effort_levels = self.levels(state.model.as_deref());
        state.plan_mode = self.plan_mode.clone();
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        state.children = self.children_counts();
        state.jobs = Counts {
            live: count(
                self.jobs
                    .iter()
                    .filter(|job| job.job.state == "running")
                    .count(),
            ),
            waiting: 0,
            total: count(self.jobs.len()),
        };
        state.changes = Some(self.changes.total);
        let phase = |phase: &str| {
            count(
                self.board
                    .tasks
                    .iter()
                    .filter(|task| task.phase == phase)
                    .count(),
            )
        };
        state.board = Some(BoardBrief {
            open: count(
                self.board
                    .tasks
                    .iter()
                    .filter(|task| !matches!(task.status.as_str(), "done" | "dropped"))
                    .count(),
            ),
            blocked: phase("blocked"),
            failed: phase("failed"),
            proposed: self.board.proposed.is_some(),
        });
        state.plan = self.plan.plan.as_ref().map(|plan| PlanBrief {
            done: count(
                plan.steps
                    .iter()
                    .filter(|step| step.state == "done")
                    .count(),
            ),
            total: count(plan.steps.len()),
            current: plan
                .steps
                .iter()
                .find(|step| step.state == "in_progress")
                .map(|step| step.text.clone()),
            stale: self.plan.stale,
        });
    }
}

/// A child's fleet row, from its detail.
fn row(child: &Child) -> SessionChild {
    let detail = &child.detail;
    let mut edited: Vec<&str> = detail.edits.iter().map(|edit| edit.path.as_str()).collect();
    edited.sort_unstable();
    edited.dedup();
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    SessionChild {
        id: detail.id.clone(),
        kind: detail.kind.clone(),
        title: detail.title.clone(),
        state: detail.state.clone(),
        model: detail.model.clone(),
        started_at: detail.started_at,
        ended_at: detail.ended_at,
        activity: detail.activity.clone(),
        tokens: detail.usage.input + detail.usage.output,
        cost_usd: detail.usage.cost_usd,
        verdict: detail
            .verdict
            .as_ref()
            .map(|verdict| match verdict.verdict.as_str() {
                "approve" => SessionChildVerdict::Approve,
                _ => SessionChildVerdict::Changes,
            }),
        question: detail.question.as_ref().map(|_| true),
        profile: Some(detail.profile.clone()),
        effort: detail.effort.clone(),
        pending_model: detail.pending_model.clone(),
        turns: detail.usage.turns,
        context_percent: detail.usage.context_percent,
        files: count(edited.len()),
        outside: count(detail.edits.iter().filter(|edit| edit.outside).count()),
        owns: count(detail.owns.len()),
        after: detail.after.clone(),
        steerable: child.live(),
        error: detail.error.clone(),
    }
}

/// The session a part detail belongs to; none for a detail that is not a host's part.
fn scope_of(target: &DetailRef) -> Option<&str> {
    match target {
        DetailRef::Session { id } => id.split_once("/child/").map(|(session, _)| session),
        DetailRef::SessionCards { id }
        | DetailRef::SessionModels { id }
        | DetailRef::SessionChild { id, .. }
        | DetailRef::SessionJobs { id }
        | DetailRef::SessionJobOutput { id, .. }
        | DetailRef::SessionChanges { id, .. }
        | DetailRef::SessionChangeFile { id, .. }
        | DetailRef::SessionUsage { id }
        | DetailRef::SessionContext { id }
        | DetailRef::SessionPlan { id }
        | DetailRef::SessionPlanModeDoc { id }
        | DetailRef::SessionEvents { id }
        | DetailRef::SessionCommands { id }
        | DetailRef::BuildBoard { id }
        | DetailRef::BuildPlanTask { id, .. }
        | DetailRef::BuildRecord { id } => Some(id),
        _ => None,
    }
}

/// Serves `target` when it is a part of a sample host; none for any other detail. A walked
/// job output reaches the older page `page` names.
pub(super) fn serve(
    feed: &mut Feed,
    target: &DetailRef,
    page: Option<&str>,
) -> Option<Result<(), String>> {
    let scope = scope_of(target)?;
    Some(serve_part(feed, target, scope, page))
}

fn serve_part(
    feed: &mut Feed,
    target: &DetailRef,
    scope: &str,
    page: Option<&str>,
) -> Result<(), String> {
    let id = resolve(feed, scope).ok_or_else(|| format!("not_found: {scope} has no Pi host"))?;
    let host = feed
        .hosts
        .get(id.as_str())
        .ok_or_else(|| format!("not_found: {scope} has no Pi host"))?;
    if host.gone {
        return Err(COLD.into());
    }
    let body = match target {
        DetailRef::SessionCards { .. } => DetailBody::SessionCards(SessionCards {
            cards: host.pending.clone(),
        }),
        DetailRef::SessionEvents { .. } => DetailBody::SessionEvents(events(host)),
        DetailRef::SessionPlanModeDoc { .. } => {
            return super::plan_mode::serve(feed, target, id.as_str());
        }
        DetailRef::SessionJobOutput { job, .. } => {
            let pages = output(host, job, page)?;
            put_pages(feed, target, pages);
            return Ok(());
        }
        _ => {
            let parts = host
                .parts
                .as_deref()
                .ok_or("not_found: this session's host does not serve that part")?;
            let row = feed.assistants.get(id.as_str());
            let model = row.and_then(|row| row.model.clone());
            let effort = row.and_then(|row| row.effort.clone());
            part(parts, target, host.turn, model, effort)?
        }
    };
    put(feed, target.clone(), body);
    Ok(())
}

/// One part of a build's host, read now.
fn part(
    parts: &Parts,
    target: &DetailRef,
    turn: u64,
    model: Option<String>,
    effort: Option<String>,
) -> Result<DetailBody, String> {
    let missing = |what: &str| format!("not_found: no such {what} in this session");
    Ok(match target {
        DetailRef::Session { id } => {
            let (_, name) = id.split_once("/child/").ok_or_else(|| missing("child"))?;
            let child = parts
                .children
                .iter()
                .find(|child| child.detail.id == name)
                .ok_or_else(|| missing("child"))?;
            DetailBody::Session(conversation(id, child))
        }
        DetailRef::SessionChild { child, .. } => DetailBody::SessionChild(
            parts
                .children
                .iter()
                .find(|found| found.detail.id == *child)
                .map(|found| found.detail.clone())
                .ok_or_else(|| missing("child"))?,
        ),
        DetailRef::SessionModels { .. } => DetailBody::SessionModels(SessionModels {
            current: model,
            effort,
            ..parts.models.clone()
        }),
        DetailRef::SessionJobs { .. } => DetailBody::SessionJobs(SessionJobs {
            jobs: parts.jobs.iter().map(|job| job.job.clone()).collect(),
            unavailable: None,
        }),
        DetailRef::SessionChanges { comparison, .. } => {
            DetailBody::SessionChanges(changes(parts, *comparison))
        }
        DetailRef::SessionChangeFile {
            comparison, path, ..
        } => {
            if *comparison != parts.changes.comparison {
                return Err(format!("not_found: {path} is not in that comparison"));
            }
            DetailBody::SessionChangeFile(
                parts
                    .patches
                    .iter()
                    .find(|patch| patch.path == *path)
                    .cloned()
                    .ok_or_else(|| format!("not_found: {path} is not among the changed files"))?,
            )
        }
        DetailRef::SessionUsage { .. } => {
            let rows = parts.rows(turn);
            DetailBody::SessionUsage(SessionUsage {
                model,
                effort,
                context: Some(ContextUse {
                    tokens: Some(parts.context_tokens),
                    context_window: WINDOW,
                    percent: Some(parts.percent()),
                }),
                total: Some(Parts::total(&rows)),
                rows,
            })
        }
        DetailRef::SessionContext { .. } => DetailBody::SessionContext(parts.context.clone()),
        DetailRef::SessionPlan { .. } => DetailBody::SessionPlan(parts.plan.clone()),
        DetailRef::SessionCommands { .. } => DetailBody::SessionCommands(parts.commands.clone()),
        DetailRef::BuildBoard { .. } => DetailBody::BuildBoard(parts.board.clone()),
        DetailRef::BuildRecord { .. } => DetailBody::BuildRecord(parts.record.clone()),
        // The sample keeps each proposed brief whole: one page holds all of it.
        DetailRef::BuildPlanTask { key, .. } => {
            let task = parts
                .board
                .proposed
                .as_ref()
                .and_then(|plan| plan.tasks.iter().find(|task| task.key == *key))
                .ok_or_else(|| missing("proposed task"))?;
            DetailBody::BuildPlanTask(BuildPlanTaskPage {
                key: key.clone(),
                body: task.body.clone(),
                bytes: task.body.len() as u64,
                offset: 0,
            })
        }
        _ => return Err(format!("Preview has no sample of {target:?}")),
    })
}

/// The changed files of `comparison`: the session's own, or why another cannot be compared.
fn changes(parts: &Parts, comparison: ChangesComparison) -> SessionChanges {
    if comparison == parts.changes.comparison {
        return parts.changes.clone();
    }
    let why = parts
        .changes
        .comparisons
        .iter()
        .find(|info| info.id == comparison)
        .and_then(|info| info.unavailable.clone())
        .unwrap_or_else(|| "this host does not offer that comparison".into());
    SessionChanges {
        comparison,
        files: Vec::new(),
        total: 0,
        added: 0,
        removed: 0,
        truncated: false,
        error: Some(why),
        ..parts.changes.clone()
    }
}

/// A child's own transcript, read-only: its brief, its replies and the owner's steers.
fn conversation(scope: &str, child: &Child) -> SessionPage {
    SessionPage {
        id: scope.into(),
        role: "child".into(),
        task: None,
        run: None,
        machine: Some("preview-workstation".into()),
        messages: child
            .transcript
            .iter()
            .enumerate()
            .map(|(n, message)| {
                let mut message = message.clone();
                if message.id.is_empty() {
                    message.id = format!("{scope}/entry/{n}");
                }
                message
            })
            .collect(),
        older: None,
        replyable: false,
        reply_refusal: Some("a child agent is steered from its parent session".into()),
        host: HostPresence::Running,
    }
}

/// The host's events: a build's log, else each turn's start from its checkpoints.
fn events(host: &Host) -> SessionEventsPage {
    let events = match host.parts.as_deref() {
        Some(parts) => parts.events.clone(),
        None => host
            .checkpoints
            .iter()
            .enumerate()
            .map(|(n, point)| HostEvent {
                seq: n as u64 + 1,
                at: point.at.clone().unwrap_or_default(),
                kind: "turn_start".into(),
                text: format!("{{\"turn\":{}}}", point.turn),
            })
            .collect(),
    };
    let last = events.last().map_or(0, |event| event.seq);
    SessionEventsPage { events, last }
}

/// The newest page of a job's output, and the older one when `page` asks for it.
fn output(
    host: &Host,
    job: &str,
    page: Option<&str>,
) -> Result<Vec<(JobOutputPage, Option<String>)>, String> {
    let found = host
        .parts
        .as_deref()
        .and_then(|parts| parts.jobs.iter().find(|found| found.job.id == job))
        .ok_or_else(|| format!("not_found: no such job {job} in this session"))?;
    let older = found.older.len() as u64;
    let bytes = older + found.newest.len() as u64;
    let live = found.job.state == "running";
    let token = (older > 0).then(|| format!("{job}:{older}"));
    let newest = JobOutputPage {
        text: found.newest.clone(),
        offset: older,
        bytes,
        live,
    };
    match page {
        None => Ok(vec![(newest, token)]),
        Some(asked) if token.as_deref() == Some(asked) => Ok(vec![
            (newest, token.clone()),
            (
                JobOutputPage {
                    text: found.older.clone(),
                    offset: 0,
                    bytes,
                    live,
                },
                None,
            ),
        ]),
        Some(asked) => Err(format!("invalid: no output page {asked}")),
    }
}

/// A job's output as the client holds a walked read: its pages, newest first, and their words
/// joined oldest first.
fn put_pages(feed: &mut Feed, target: &DetailRef, pages: Vec<(JobOutputPage, Option<String>)>) {
    let Some(mut merged) = pages.first().map(|(newest, _)| newest.clone()) else {
        return;
    };
    merged.text = pages
        .iter()
        .rev()
        .map(|(page, _)| page.text.as_str())
        .collect();
    if let Some((oldest, _)) = pages.last() {
        merged.offset = oldest.offset;
    }
    let replies = pages
        .into_iter()
        .map(|(page, next)| DetailReply {
            etag: format!("preview-{}", feed.changes_head),
            as_of: stamp(feed.now),
            seq: feed.changes_head,
            next,
            body: Some(DetailBody::SessionJobOutput(page)),
        })
        .collect();
    feed.details.insert(
        detail_key(target),
        DetailCache {
            transcript_seams: Default::default(),
            target: target.clone(),
            pages: replies,
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::SessionJobOutput(merged)),
        },
    );
}

/// Every part of host `id` this device has read is read again, as the app's re-read on the
/// host's `live_part` would, and every scope that watches it gets its fleet.
pub(super) fn reread(feed: &mut Feed, id: &str) {
    let Some(host) = feed.hosts.get(id).filter(|host| !host.gone) else {
        return;
    };
    let fleet = host.parts.as_deref().map(Parts::fleet);
    let watching = scopes(feed, id);
    if let Some(fleet) = fleet {
        for scope in &watching {
            feed.live
                .entry(scope.clone())
                .or_default()
                .replace_fleet(Some(fleet.clone()));
        }
    }
    let read: Vec<(DetailRef, Option<String>)> = feed
        .details
        .values()
        .filter(|cache| {
            scope_of(&cache.target).is_some_and(|scope| {
                scope == id || watching.iter().any(|watched| watched.as_str() == scope)
            })
        })
        .map(|cache| {
            // A walked output is read again as far as it was walked.
            let older = (cache.pages.len() > 1)
                .then(|| cache.pages.first().and_then(|newest| newest.next.clone()))
                .flatten();
            (cache.target.clone(), older)
        })
        .collect();
    for (target, older) in read {
        // A part gone since it was read (a child no longer listed) reads as corrald answers it.
        if let Some(Err(why)) = serve(feed, &target, older.as_deref())
            && let Some(cache) = feed.details.get_mut(&detail_key(&target))
        {
            cache.error = Some(super::failure(why));
            cache.stale = true;
        }
    }
}

#[cfg(test)]
#[path = "parts_tests.rs"]
mod tests;
