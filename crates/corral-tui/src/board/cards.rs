//! Board card rendering: the id/project/priority row, the title and the per-status meta lines.

use corral_core::{ArtifactKind, Config, Status, Task};
use corral_ops::Finish;
use jiff::Timestamp;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::autopilot;
use crate::artifacts::ui;
use crate::choice;
use crate::overlays::truncate;
use crate::theme;

/// What a card shows beyond the task file: for Done cards the finish record; on every card a ★ when the task or
/// one of its artifacts is pinned.
#[derive(Clone, Copy, Default)]
pub(super) struct CardExtra<'a> {
    pub finish: Option<&'a Finish>,
    pub pinned: bool,
}

fn ago(ts: Timestamp, now: Timestamp) -> String {
    let s = (now.as_second() - ts.as_second()).max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
        _ => format!("{}d", s / 86400),
    }
}

pub(super) fn wrap2(text: &str, width: usize) -> Vec<String> {
    let mut first = String::new();
    let mut rest = text;
    for (i, word) in text.split(' ').enumerate() {
        let candidate = if i == 0 {
            word.to_string()
        } else {
            format!("{first} {word}")
        };
        if candidate.width() > width {
            break;
        }
        first = candidate;
        rest = text[first.len()..].trim_start();
    }
    if first.is_empty() {
        return vec![truncate(text, width)];
    }
    if rest.is_empty() {
        vec![first]
    } else {
        vec![first, truncate(rest, width)]
    }
}

/// `text` (the profile, or a run's harness·model·effort) in the profile's color.
fn agent_span(agent: &str, text: &str) -> Span<'static> {
    Span::styled(text.to_string(), Style::new().fg(theme::agent_color(agent)))
}

/// "+A −R" parsed from the run's "Final diff +A −R" artifact title.
fn diff_counts(task: &Task, run_id: &str) -> Option<(String, String)> {
    let a = task
        .artifacts
        .iter()
        .rev()
        .find(|a| a.run == run_id && a.kind == ArtifactKind::Diff)?;
    let mut words = a
        .title
        .split_whitespace()
        .filter(|w| w.starts_with('+') || w.starts_with('−') || w.starts_with('-'));
    Some((words.next()?.to_string(), words.next()?.to_string()))
}

fn meta_lines(
    t: &Task,
    all: &[Task],
    config: &Config,
    width: usize,
    now: Timestamp,
    extra: CardExtra,
) -> Vec<Line<'static>> {
    let dim = theme::dim();
    let sep = || Span::styled(" · ", theme::dim());
    let open_deps: Vec<&String> = t
        .depends_on
        .iter()
        .filter(|d| !all.iter().any(|x| &x.id == *d && x.status == Status::Done))
        .collect();
    let deps_queued = |spans: &mut Vec<Span<'static>>| {
        if !open_deps.is_empty() {
            let deps = open_deps
                .iter()
                .map(|d| d.as_str())
                .collect::<Vec<_>>()
                .join(",");
            if !spans.is_empty() {
                spans.push(sep());
            }
            spans.push(Span::styled(
                format!("⛓ {deps}"),
                Style::new().fg(theme::RED),
            ));
        }
        if t.queued {
            if !spans.is_empty() {
                spans.push(sep());
            }
            spans.push(Span::styled("⏳ queued", Style::new().fg(theme::YELLOW)));
        }
    };
    let run = t.current_run();
    let agent = run.map(|r| r.agent.clone()).unwrap_or_else(|| {
        config
            .route_task(t, &corral_core::ProfileChoice::default(), |_| None)
            .map(|r| r.profile)
            .unwrap_or_else(|_| config.default_profile(t.profile.as_deref(), t.project.as_deref()))
    });
    let who = run.map_or_else(|| agent.clone(), choice::run_label);
    let who_span = agent_span(&agent, &who);
    let mut out = Vec::new();
    match t.status {
        Status::Idea => {
            let mut spans = vec![Span::styled("◌ idea", dim)];
            deps_queued(&mut spans);
            out.push(Line::from(spans));
        }
        Status::Backlog => {
            let mut spans = Vec::new();
            deps_queued(&mut spans);
            if !spans.is_empty() {
                out.push(Line::from(spans));
            }
        }
        Status::Ready => {
            let mut spans = vec![Span::styled("⇢ ", dim), who_span.clone()];
            deps_queued(&mut spans);
            out.push(Line::from(spans));
        }
        Status::Running => {
            let mut spans = vec![
                who_span.clone(),
                Span::styled(" ⟳ ", Style::new().fg(theme::BLUE)),
            ];
            if let Some(r) = run {
                spans.push(Span::styled(ago(r.started, now), dim));
                if r.stuck.is_none() {
                    if r.idle_no_report {
                        spans.push(sep());
                        spans.push(Span::styled(
                            "⚠ idle, no report",
                            Style::new().fg(theme::YELLOW),
                        ));
                    } else if let Some(state) = &r.agent_state {
                        spans.push(sep());
                        spans.push(Span::styled(state.clone(), dim));
                    }
                }
            }
            out.push(Line::from(spans));
            out.extend(run.and_then(|r| autopilot::changes_line(t, r, config)));
            if let Some(s) = run.and_then(|r| r.stuck.as_ref()) {
                out.push(Line::styled(
                    format!("⚠ stuck: {}", s.argv.join(" ")),
                    Style::new().fg(theme::RED),
                ));
            }
        }
        Status::NeedsMe => {
            out.push(Line::from(vec![
                who_span.clone(),
                Span::styled(" ? ", Style::new().fg(theme::YELLOW)),
                Span::styled(format!("waiting {}", ago(t.updated, now)), dim),
            ]));
            let question = autopilot::escalation(t).unwrap_or_else(|| {
                run.and_then(|r| r.blocked_excerpt.as_deref())
                    .and_then(|e| e.lines().map(str::trim).rfind(|l| !l.is_empty()))
                    .unwrap_or("the agent is waiting for you")
                    .to_string()
            });
            let style = Style::new()
                .fg(theme::YELLOW)
                .add_modifier(Modifier::ITALIC);
            out.extend(
                wrap2(&format!("\"{question}\""), width)
                    .into_iter()
                    .map(|l| Line::styled(l, style)),
            );
        }
        Status::Review => {
            let mut spans = vec![who_span.clone()];
            if let Some(r) = run {
                if let Some((add, del)) = diff_counts(t, &r.id) {
                    spans.push(sep());
                    spans.push(Span::styled(add, Style::new().fg(theme::GREEN)));
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(del, Style::new().fg(theme::RED)));
                }
                let n = t.artifacts.iter().filter(|a| a.run == r.id).count();
                spans.push(sep());
                spans.push(Span::styled(format!("◆ {n} art"), dim));
            }
            out.push(Line::from(spans));
            out.extend(autopilot::review_lines(t, config, width));
        }
        Status::Done => out.extend(done_lines(t, extra)),
        Status::Dropped => {}
    }
    out
}

/// Done card meta: `✓ <landed sha> <strategy>` (plus the autopilot note), then the finish date and the artifact
/// count badge.
fn done_lines(t: &Task, extra: CardExtra) -> [Line<'static>; 2] {
    let green = Style::new().fg(theme::GREEN);
    let mut first = match extra.finish.and_then(|f| f.landed.as_ref()) {
        Some(landed) => vec![
            Span::styled(format!("✓ {}", landed.short_sha()), green),
            Span::styled(format!(" {}", landed.strategy_label()), theme::dim()),
        ],
        None => vec![Span::styled("✓ done", green)],
    };
    first.extend(autopilot::done_span(t.current_run()));
    let first = Line::from(first);
    let mut second = Vec::new();
    if let Some(f) = extra.finish {
        second.push(Span::styled(
            format!("{} ", ui::local_date(f.at, "%b %d")),
            theme::dim(),
        ));
    }
    second.push(Span::styled(
        format!("◆ {}", t.artifacts.len()),
        Style::new().fg(theme::MAGENTA),
    ));
    [first, Line::from(second)]
}

pub(super) fn card_lines(
    t: &Task,
    all: &[Task],
    config: &Config,
    width: usize,
    selected: bool,
    now: Timestamp,
    extra: CardExtra,
) -> Vec<Line<'static>> {
    let bar = if selected {
        Span::styled("▌ ", Style::new().fg(theme::CYAN))
    } else if t.status == Status::NeedsMe {
        // Distinct from the selection bar so the focused card is never ambiguous.
        Span::styled("▲ ", Style::new().fg(theme::YELLOW))
    } else {
        Span::raw("  ")
    };
    let star = if extra.pinned { "★ " } else { "" };
    let id_w = t.id.width() + star.width();
    let prio_w = t.priority.width();
    let proj_room = width.saturating_sub(id_w + prio_w + 4);
    let proj = t
        .project
        .as_deref()
        .map(|p| truncate(p, proj_room))
        .unwrap_or_default();
    let pad = width.saturating_sub(id_w + 1 + proj.width() + prio_w + 2);
    let proj_style = Style::new().fg(t
        .project
        .as_deref()
        .map_or(theme::DIM, theme::project_color));
    let r1 = Line::from(vec![
        bar.clone(),
        Span::styled(t.id.clone(), theme::bold()),
        Span::raw(" "),
        Span::styled(star, Style::new().fg(theme::YELLOW)),
        Span::styled(proj, proj_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(t.priority.clone(), theme::dim()),
        Span::styled(" ⋯", theme::dim()),
    ]);
    let mut title_style = Style::new().fg(theme::FG);
    if selected {
        title_style = title_style.add_modifier(Modifier::BOLD);
    }
    let mut lines = vec![
        r1,
        Line::from(vec![
            bar.clone(),
            Span::styled(truncate(&t.title, width), title_style),
        ]),
    ];
    for meta in meta_lines(t, all, config, width, now, extra) {
        let mut spans = vec![bar.clone()];
        let mut used = 0;
        for s in meta.spans {
            let room = width.saturating_sub(used);
            if room == 0 {
                break;
            }
            let text = truncate(&s.content, room);
            used += text.width();
            spans.push(Span::styled(text, s.style.patch(meta.style)));
        }
        lines.push(Line::from(spans));
    }
    lines
}
