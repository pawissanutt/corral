//! Shared, read-only PM documents and classic-board operating text.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{Ctx, PmAction, PmActionKind, PmWorkset};

/// Complete CLI output: pretty-print `json`, or print `text` verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PmReadout {
    pub json: serde_json::Value,
    pub text: String,
}

/// The attention projection and operating header from one board read-back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoardReadback {
    pub operating: String,
    pub attention: crate::attention::AttentionBoard,
}

static BUILD_IDENTITY: OnceLock<&'static str> = OnceLock::new();

/// Set at executable startup to the identity printed by `corral build-identity`.
pub fn set_build_identity(identity: &'static str) {
    let _ = BUILD_IDENTITY.set(identity);
}

/// Bind served commands and their action digests to this exact executable build.
pub fn client_identity() -> Result<(String, String)> {
    let exe = std::env::current_exe().context("finding client executable")?;
    let exe = exe
        .to_str()
        .context("client executable path is not UTF-8")?;
    let build = BUILD_IDENTITY
        .get()
        .context("client build identity is not initialized")?;
    Ok((exe.to_owned(), (*build).to_owned()))
}

/// Evaluate on one pinned snapshot with the same preparation as CLI status reads.
pub fn evaluate<T>(ctx: &Ctx, pass: impl FnOnce(&Ctx) -> Result<T>) -> Result<T> {
    let snapshot = Ctx {
        store: ctx.store.read_only_snapshot()?,
        ..ctx.clone()
    };
    crate::lesson_rows::prepare_coordination(&snapshot)?;
    let _hot = corral_core::work::HotScope::enter();
    let result = pass(&snapshot);
    if std::env::var_os("CORRAL_WORK_REPORT").is_some_and(|value| !value.is_empty()) {
        use corral_core::work::{Work, count};
        let counts: serde_json::Map<_, _> = Work::ALL
            .iter()
            .map(|work| (work.name().to_owned(), count(*work).into()))
            .collect();
        eprintln!("work: {}", serde_json::Value::Object(counts));
    }
    result
}

fn readback(state: &PmWorkset, automation: &str) -> Result<serde_json::Value> {
    let mut action = serde_json::to_value(&state.next)?;
    action["workset"] = serde_json::to_value(state)?;
    action["automation"] = automation.into();
    Ok(action)
}

pub fn format_workset(state: &PmWorkset) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "{}: {}/{} accepted; {} scoped running; independent work continues: {}",
        state.status.as_str(),
        state.progress.accepted,
        state.progress.total,
        state.progress.running,
        state.independent_work_continues,
    );
    for decision in &state.decisions {
        let newly_required = decision.presentation.is_none();
        let _ = writeln!(
            text,
            "Decision {} ({}): {} [{}]",
            decision.id,
            decision.task,
            decision.action.why,
            match &decision.presentation {
                Some(receipt) if receipt.confirmed => "presented; answer pending",
                Some(_) => "delivery uncertain; answer pending",
                None => "not yet presented",
            },
        );
        if newly_required {
            for evidence in &decision.action.evidence {
                let _ = writeln!(text, "  {evidence}");
            }
        }
    }
    for blocker in &state.blockers {
        let _ = writeln!(text, "Blocker: {blocker}");
    }
    for item in &state.inbox {
        let _ = writeln!(text, "Inbox: {} [{}]", item.why, item.evidence.join("; "));
    }
    text
}

pub fn format_action(action: &PmAction) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "{}: {}",
        action.task.as_deref().unwrap_or("workspace"),
        action.why
    );
    for evidence in &action.evidence {
        let _ = writeln!(text, "Evidence: {evidence}");
    }
    if let Some(digest) = &action.question_digest {
        for line in crate::question_digest_lines(digest) {
            let _ = writeln!(text, "{line}");
        }
        let _ = writeln!(text, "Verify: {}", action.verify);
        return text;
    }
    let _ = writeln!(
        text,
        "Recommended: {}{}",
        action.recommended,
        if action.requires_owner {
            " (owner required)"
        } else {
            ""
        }
    );
    let _ = writeln!(text, "Command: {}", action.command);
    let _ = writeln!(text, "Verify: {}", action.verify);
    for option in &action.options {
        let _ = writeln!(
            text,
            "Option {}: {} -> {}",
            option.id, option.label, option.command
        );
    }
    text
}

pub fn next(ctx: &Ctx) -> Result<PmReadout> {
    evaluate(ctx, |ctx| {
        let state = crate::pm_workset(ctx)?;
        let automation = crate::automation_summary(ctx)?;
        let _print = corral_core::timing::span("next.print");
        let json = readback(&state, &automation)?;
        let mut text = format!("{automation}\n");
        text.push_str(&format_workset(&state));
        text.push_str(&format_action(&state.next));
        Ok(PmReadout { json, text })
    })
}

pub fn brief(ctx: &Ctx) -> Result<PmReadout> {
    evaluate(ctx, |ctx| {
        let health = crate::pm_health(ctx)?;
        let setup = {
            let _timing = corral_core::timing::span("brief.setup");
            crate::setup_status(&ctx.store)?
        };
        // Aggregate rows include every project and status without decoding task histories.
        let status_counts = {
            let _timing = corral_core::timing::span("brief.counts");
            ctx.store
                .view()?
                .aggregates(&[corral_core::backend::AggregateName::StatusCounts])?
                .status_counts
        };
        let (used, limit) = crate::slots(ctx)?;
        let state = crate::pm_workset(ctx)?;
        let automation = crate::automation_summary(ctx)?;
        let now = jiff::Timestamp::now();
        let due = {
            let _timing = corral_core::timing::span("brief.lessons");
            crate::lessons_due(ctx, now)?
        };
        let score = |lesson: &corral_core::Lesson| {
            let age = now.duration_since(lesson.last_seen).as_secs_f64().max(0.0);
            lesson.confidence
                * if lesson.pinned {
                    1.0
                } else {
                    2.0_f64.powf(-age / (90.0 * 86400.0))
                }
        };
        let mut lessons = std::collections::BTreeMap::new();
        {
            let _timing = corral_core::timing::span("brief.lessons");
            for project in ctx.store.config.projects.keys() {
                let mut selected = ctx.store.lessons(project)?;
                selected.retain(|l| !l.contradicted);
                selected.sort_by(|a, b| {
                    b.pinned
                        .cmp(&a.pinned)
                        .then_with(|| score(b).total_cmp(&score(a)))
                        .then_with(|| a.id.cmp(&b.id))
                });
                selected.truncate(ctx.store.config.lessons.top);
                if !selected.is_empty() {
                    lessons.insert(project.clone(), selected);
                }
            }
        }
        let mut counts = std::collections::BTreeMap::new();
        {
            let _timing = corral_core::timing::span("brief.counts");
            for count in status_counts {
                if count.n > 0 {
                    *counts.entry(count.status).or_insert(0u64) += count.n;
                }
            }
        }
        let _print = corral_core::timing::span("brief.print");
        let next = readback(&state, &automation)?;
        let json = serde_json::json!({
            "slots": {"used":used,"limit":limit}, "counts":counts, "next":next,
            "operating": next["workset"],
            "lessons":lessons, "lessons_due":due,
            "health":health,
            "setup":setup,
            "automation":next["automation"],
        });
        let mut text = format!("{automation}\n");
        text.push_str(&format_workset(&state));
        let _ = writeln!(text, "Health: {}", serde_json::to_string(&health)?);
        let _ = writeln!(text, "Setup: {}", serde_json::to_string(&setup)?);
        let _ = writeln!(
            text,
            "Slots: {used}/{limit}; states: {}",
            serde_json::to_string(&counts)?
        );
        text.push_str(&format_action(&state.next));
        for (project, rules) in lessons {
            for rule in rules {
                let _ = writeln!(text, "Lesson {project}/{}: {}", rule.id, rule.rule);
            }
        }
        for schedule in due {
            let _ = writeln!(
                text,
                "Lessons due {}: {}",
                schedule.project,
                schedule.reasons.join(", ")
            );
        }
        Ok(PmReadout { json, text })
    })
}

/// The workspace PM agent's bounded next card, independent of its session.
pub fn agent_next(ctx: &Ctx) -> Result<Value> {
    let projects = crate::agent::hooks::pm::projects(&ctx.store)?;
    agent_next_for_projects(ctx, &projects)
}

/// The workspace PM agent's bounded brief, including its observational digest.
pub fn agent_brief(ctx: &Ctx) -> Result<Value> {
    let projects = crate::agent::hooks::pm::projects(&ctx.store)?;
    agent_brief_for_projects(ctx, crate::agent::hooks::pm::WORKSPACE, &projects)
}

pub(crate) fn agent_next_for_projects(ctx: &Ctx, projects: &BTreeSet<String>) -> Result<Value> {
    let next = crate::pm_workset::next_for_projects(ctx, projects)?;
    crate::agent::ops::pm_bound::bounded_next(&next, corral_core::agent::RESULT_MAX_BYTES)
}

pub(crate) fn agent_brief_for_projects(
    ctx: &Ctx,
    project: &str,
    projects: &BTreeSet<String>,
) -> Result<Value> {
    let next = crate::pm_workset::next_for_projects(ctx, projects)?;
    let mut counts = BTreeMap::new();
    let status_counts = ctx
        .store
        .view()?
        .aggregates(&[corral_core::backend::AggregateName::StatusCounts])?
        .status_counts;
    for count in status_counts {
        if count.n > 0 && projects.contains(&count.project) {
            *counts.entry(count.status).or_insert(0u64) += count.n;
        }
    }
    // Native owners only, with semantic closure and no task-history hydration.
    let mut heads = ctx.store.unfinished_native_heads()?;
    heads.retain(|head| head.project.as_ref().is_some_and(|p| projects.contains(p)));
    heads.sort_by(|a, b| (a.num, &a.id).cmp(&(b.num, &b.id)));
    let mut pending = Vec::new();
    for head in heads {
        pending.extend(crate::questions::recorded_pending_views_ref(&head)?);
    }
    let questions: Vec<_> = pending
        .into_iter()
        .filter(|q| q.pending)
        .map(|q| {
            json!({"task":q.task,"key":q.question.key,
                "revision":q.question.revision,"kind":q.question.kind,
                "state":q.effective_state,"blocking":q.question.blocking})
        })
        .collect();
    let brief = crate::agent::ops::pm_bound::bounded_brief(
        &next,
        json!({"project":project,"counts":counts,"questions":questions}),
        corral_core::agent::RESULT_MAX_BYTES,
    )?;
    crate::agent::ops::pm_shadow::with_brief_digest(brief, &next)
}

pub fn board_readback(
    ctx: &Ctx,
    attention: Option<crate::attention::AttentionBoard>,
) -> Result<BoardReadback> {
    let attention = match attention {
        Some(board) => board,
        None => {
            let _timing = corral_core::timing::span("board.attention");
            // A partial listing must never become a falsely reassuring attention count.
            let (sender, messages) = std::sync::mpsc::channel();
            let result = {
                let _capture = corral_core::capture_messages(sender);
                crate::attention::board_hot(ctx, jiff::Timestamp::now())
            };
            let mut unavailable = None;
            for message in messages.try_iter() {
                if message.file.is_some() {
                    unavailable.get_or_insert(message.text);
                } else {
                    corral_core::report_message(
                        message.task.as_deref(),
                        message.source,
                        message.text,
                    );
                }
            }
            if let Some(error) = unavailable {
                bail!(error);
            }
            result?
        }
    };
    let workset = {
        let _timing = corral_core::timing::span("board.workset");
        crate::pm_workset_with_attention(ctx, &attention)
    };
    Ok(BoardReadback {
        operating: readback_result(workset),
        attention,
    })
}

pub fn action_reason(action: &PmAction) -> String {
    match action.kind {
        PmActionKind::Owner | PmActionKind::Wait => action.evidence.first().cloned(),
        PmActionKind::Question => action.evidence.last().map(|text| readable_question(text)),
        _ => None,
    }
    .unwrap_or_else(|| action.why.clone())
}

/// Harness questions carry raw JSON; display the question text it asks.
pub fn readable_question(text: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
        return text.to_owned();
    };
    let items = match &value {
        serde_json::Value::Array(items) => items.iter().collect::<Vec<_>>(),
        serde_json::Value::Object(_) => vec![&value],
        _ => return text.to_owned(),
    };
    let questions = items
        .iter()
        .filter_map(|item| {
            item.get("question")
                .or_else(|| item.get("header"))
                .and_then(serde_json::Value::as_str)
        })
        .collect::<Vec<_>>();
    if questions.is_empty() {
        text.to_owned()
    } else {
        questions.join("; ")
    }
}

fn concise(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.width() <= 120 {
        return text;
    }
    let mut out = String::new();
    let mut width = 0;
    for c in text.chars() {
        let cw = c.width().unwrap_or(0);
        if width + cw + 1 > 120 {
            break;
        }
        out.push(c);
        width += cw;
    }
    out.push('…');
    out
}

pub fn operating_summary(workset: &PmWorkset) -> String {
    let mode = match workset.mode {
        corral_core::OperatingMode::Assist => "Assist",
        corral_core::OperatingMode::Autopilot => "Autopilot",
    };
    let progress = &workset.progress;
    let mut text = format!(
        "{mode}{} | {} | scope {}/{} accepted; {} running | O mode",
        if workset.paused { " (paused)" } else { "" },
        workset.status.as_str(),
        progress.accepted,
        progress.total,
        progress.running,
    );
    if workset.paused {
        text.push_str(
            "\nNo new automatic actions; Resume required; running receipts still collected.",
        );
    } else if let Some(decision) = workset.decisions.first() {
        let _ = write!(
            text,
            "\n{}: {}; {} decision(s); independent work {}",
            decision.task,
            concise(&action_reason(&decision.action)),
            workset.decisions.len(),
            if workset.independent_work_continues {
                "continues"
            } else {
                "waiting"
            },
        );
    } else if let Some(wait) = workset.waits.first() {
        let _ = write!(
            text,
            "\n{}: {}",
            wait.task.as_deref().unwrap_or("workspace"),
            concise(&action_reason(wait))
        );
    } else if let Some(blocker) = workset.blockers.first() {
        let _ = write!(text, "\n{}", concise(blocker));
    }
    text
}

pub fn readback_result(result: Result<PmWorkset>) -> String {
    match result {
        Ok(workset) => operating_summary(&workset),
        Err(error) => format!("Assist | policy unavailable; no automatic authority: {error:#}"),
    }
}
