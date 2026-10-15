//! The detail view's Acceptance tab: the review policy in effect and where each value came from, the autopilot's
//! current stage and rounds, gate and review results per run with the reviewer's notes, the landing result, the
//! acceptance records (gate logs, review reports, change requests; ⏎ previews one) and the autopilot history.

use corral_core::{
    Accept, Artifact, ArtifactKind, AutoStage, Config, Event, GateResult, MergeMode, PolicySource,
    Reviewer, Task, Verdict,
};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::artifacts::ui;
use crate::theme::{self, BLUE, FG, GREEN, RED, YELLOW};

const KEY_W: usize = 14;
const SOURCE_W: usize = 26;
const HISTORY_KINDS: [&str; 5] = [
    "autopilot",
    "gate_started",
    "gate",
    "review_started",
    "verdict",
];

/// Indices into `task.artifacts` of the records the review pipeline produced, oldest first.
pub(super) fn records(task: &Task) -> Vec<usize> {
    task.artifacts
        .iter()
        .enumerate()
        .filter(|(_, a)| is_record(a))
        .map(|(i, _)| i)
        .collect()
}

fn is_record(a: &Artifact) -> bool {
    match a.kind {
        ArtifactKind::Log => a.title.starts_with("Gate:"),
        ArtifactKind::Report => {
            a.title.starts_with("Review (") || a.title.starts_with("Autopilot changes")
        }
        ArtifactKind::Evidence => a.title == "Acceptance record",
        _ => false,
    }
}

pub(super) fn history(events: Vec<Event>, task_id: &str) -> Vec<Event> {
    events
        .into_iter()
        .filter(|e| e.task.as_deref() == Some(task_id) && HISTORY_KINDS.contains(&e.kind.as_str()))
        .collect()
}

fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn when(ts: jiff::Timestamp) -> String {
    ts.strftime("%Y-%m-%d %H:%M").to_string()
}

fn heading(text: &str) -> Line<'static> {
    Line::styled(text.to_string(), theme::bold())
}

fn kv(key: &str, value: String, style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {key:<KEY_W$}"), theme::dim()),
        Span::styled(value, style),
    ])
}

fn policy_lines(task: &Task, config: &Config) -> Vec<Line<'static>> {
    let project = task.project.as_deref();
    let p = config.review_policy(project, task.review);
    let s = config.review_policy_sources(project, task.review);
    let list = |v: &[String]| {
        if v.is_empty() {
            "—".to_string()
        } else {
            v.join(" · ")
        }
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
    let autopilot = if config.autopilot_enabled() {
        "on"
    } else {
        "off"
    };
    let rows: [(&str, String, PolicySource); 9] = [
        ("gate", list(&p.gate), s.gate),
        ("reviewer", reviewer, s.reviewer),
        ("accept", accept.into(), s.accept),
        ("max rounds", p.max_rounds.to_string(), s.max_rounds),
        ("merge", merge.into(), s.merge),
        (
            "diff limit",
            format!("{} lines", p.max_diff_lines),
            s.max_diff_lines,
        ),
        ("owner paths", list(&p.owner_paths), s.owner_paths),
        (
            "remove wt",
            if p.remove_worktree { "yes" } else { "no" }.into(),
            s.remove_worktree,
        ),
        ("autopilot", autopilot.into(), s.autopilot),
    ];
    let mut out = vec![
        heading("Policy in effect"),
        Line::styled(
            format!("  {:<KEY_W$}{:<SOURCE_W$}value", "key", "set by"),
            theme::dim(),
        ),
    ];
    for (key, value, source) in rows {
        let from_color = match source {
            PolicySource::Default => theme::DIM,
            _ => BLUE,
        };
        out.push(Line::from(vec![
            Span::styled(format!("  {key:<KEY_W$}"), theme::dim()),
            Span::styled(
                ui::fit(&source.label(project), SOURCE_W),
                Style::new().fg(from_color),
            ),
            Span::styled(value, Style::new().fg(FG)),
        ]));
    }
    out
}

fn stage_color(stage: AutoStage) -> ratatui::style::Color {
    match stage {
        AutoStage::Gating | AutoStage::Reviewing | AutoStage::Landing => BLUE,
        AutoStage::Done | AutoStage::Reviewed => GREEN,
        AutoStage::Changes | AutoStage::Escalated => YELLOW,
        AutoStage::Blocked => RED,
    }
}

fn pipeline_lines(task: &Task, config: &Config) -> Vec<Line<'static>> {
    let max = config
        .review_policy(task.project.as_deref(), task.review)
        .max_rounds;
    let mut out = vec![heading("Pipeline")];
    let state = task.current_run().and_then(|r| r.autopilot.as_ref());
    match state {
        None => out.push(kv(
            "stage",
            "not picked up by the autopilot".into(),
            theme::dim(),
        )),
        Some(a) => {
            out.push(kv(
                "stage",
                format!("{} since {}", a.stage.as_str(), when(a.at)),
                Style::new().fg(stage_color(a.stage)),
            ));
            if let Some(reason) = &a.reason {
                for (i, l) in reason.lines().filter(|l| !l.trim().is_empty()).enumerate() {
                    let key = if i == 0 { "reason" } else { "" };
                    out.push(kv(key, l.to_string(), Style::new().fg(YELLOW)));
                }
            }
        }
    }
    out.push(kv(
        "rounds",
        format!("{} of {max} change rounds used", task.rounds),
        Style::new().fg(FG),
    ));
    let landed = task.runs.iter().rev().find_map(|r| {
        r.autopilot
            .as_ref()
            .filter(|a| a.stage == AutoStage::Done)
            .map(|a| (r, a))
    });
    let landing = match landed {
        Some((r, a)) => (
            format!(
                "{} landed on {} at {} ({})",
                r.id,
                r.base,
                short(a.sha.as_deref().unwrap_or("?")),
                when(a.at)
            ),
            Style::new().fg(GREEN),
        ),
        None => ("not landed".into(), theme::dim()),
    };
    out.push(kv("landing", landing.0, landing.1));
    out
}

fn run_lines(task: &Task) -> Vec<Line<'static>> {
    let mut out = vec![heading("Gate and review per run")];
    let mut any = false;
    for r in task.runs.iter().rev() {
        if r.gate.is_none() && r.review.is_none() {
            continue;
        }
        any = true;
        let mut spans = vec![Span::styled(format!("  {:<4}", r.id), Style::new().fg(FG))];
        match &r.gate {
            Some(g) => {
                let color = match g.result {
                    GateResult::Pass => GREEN,
                    GateResult::Fail => RED,
                    GateResult::Running => BLUE,
                };
                spans.push(Span::styled(
                    format!("gate {}", g.result.as_str()),
                    Style::new().fg(color),
                ));
                spans.push(Span::styled(
                    format!(" @{} {}", short(&g.sha), when(g.at)),
                    theme::dim(),
                ));
            }
            None => spans.push(Span::styled("gate —", theme::dim())),
        }
        spans.push(Span::styled("  │  ", theme::dim()));
        match &r.review {
            Some(rv) => {
                let (text, color) = match rv.verdict {
                    Some(Verdict::Approve) => ("approve", GREEN),
                    Some(Verdict::Changes) => ("changes", YELLOW),
                    Some(Verdict::Escalate) => ("escalate", YELLOW),
                    None if r.review_interrupted() => ("interrupted", YELLOW),
                    None => ("in progress", BLUE),
                };
                spans.push(Span::styled(
                    format!("review {text}"),
                    Style::new().fg(color),
                ));
                spans.push(Span::styled(
                    format!(" by {} @{} {}", rv.reviewer, short(&rv.sha), when(rv.at)),
                    theme::dim(),
                ));
            }
            None => spans.push(Span::styled("review —", theme::dim())),
        }
        out.push(Line::from(spans));
        if let Some(cmd) = r.gate.as_ref().and_then(|g| g.failed_cmd.as_deref()) {
            out.push(Line::styled(
                format!("      failed: {cmd}"),
                Style::new().fg(RED),
            ));
        }
        if let Some(notes) = r.review.as_ref().and_then(|rv| rv.notes.as_deref()) {
            out.push(Line::styled("      notes:", theme::dim()));
            out.extend(
                notes
                    .lines()
                    .map(|l| Line::styled(format!("        {l}"), Style::new().fg(FG))),
            );
        }
    }
    if !any {
        out.push(Line::styled(
            "  no gate or review has run yet",
            theme::dim(),
        ));
    }
    out
}

fn history_lines(history: Option<Result<&[Event], &str>>) -> Vec<Line<'static>> {
    let mut out = vec![heading("History")];
    let events = match history {
        None => {
            out.push(Line::styled("  loading…", theme::dim()));
            return out;
        }
        Some(Err(e)) => {
            out.push(Line::styled(
                format!("  history unavailable: {e}"),
                Style::new().fg(RED),
            ));
            return out;
        }
        Some(Ok([])) => {
            out.push(Line::styled("  no review events yet", theme::dim()));
            return out;
        }
        Some(Ok(events)) => events,
    };
    for e in events.iter().rev() {
        let d = &e.detail;
        let s = |k: &str| d.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let run = s("run");
        let text = match e.kind.as_str() {
            "autopilot" => {
                let reason = s("reason");
                let tail = if reason.is_empty() { s("note") } else { reason };
                format!("{} → {}  {tail}", s("from"), s("to"))
            }
            "gate_started" => "gate started".into(),
            "gate" => {
                let failed = s("failed_cmd");
                if failed.is_empty() {
                    format!("gate {}", s("result"))
                } else {
                    format!("gate {}: {failed}", s("result"))
                }
            }
            "review_started" => format!("review started ({})", s("reviewer")),
            "verdict" => format!("verdict {} by {}", s("verdict"), s("reviewer")),
            other => other.to_string(),
        };
        let first = text.lines().next().unwrap_or("").to_string();
        out.push(Line::from(vec![
            Span::styled(format!("  {}  ", when(e.ts)), theme::dim()),
            Span::styled(format!("{run:<4}"), Style::new().fg(FG)),
            Span::styled(first, Style::new().fg(FG)),
        ]));
    }
    out
}

/// The tab's lines, and the line index of the first acceptance record row (records are consecutive rows, in the
/// order of [`records`]).
pub(super) fn lines(
    task: &Task,
    config: &Config,
    history: Option<Result<&[Event], &str>>,
    sel: usize,
) -> (Vec<Line<'static>>, usize) {
    let mut out = policy_lines(task, config);
    out.push(Line::default());
    out.extend(pipeline_lines(task, config));
    out.push(Line::default());
    out.extend(run_lines(task));
    out.push(Line::default());
    out.push(Line::from(vec![
        Span::styled("Records", theme::bold()),
        Span::styled(
            "  gate logs, reviews, change requests · ⏎ preview",
            theme::dim(),
        ),
    ]));
    let first = out.len();
    let recs = records(task);
    if recs.is_empty() {
        out.push(Line::styled("  no records yet", theme::dim()));
    }
    for (n, i) in recs.iter().enumerate() {
        if let Some(a) = task.artifacts.get(*i) {
            out.push(ui::artifact_line(a, n == sel, None, false));
        }
    }
    out.push(Line::default());
    out.extend(history_lines(history));
    (out, first)
}
