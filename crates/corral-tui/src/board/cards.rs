//! Board card rendering: the id/project/priority row, the title and the per-status meta lines.

use corral_core::{Config, Density, RunProgress, Status, Task, diff_counts};
use corral_ops::Finish;
use jiff::Timestamp;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::autopilot;
use super::compact;
use crate::artifacts::ui;
use crate::card_fields;
use crate::choice;
use crate::obligations;
use crate::overlays::truncate;
use crate::resources::MACHINE_BUSY;
use crate::theme;

/// What a card shows beyond the task file: for Done cards the finish record; on every card a ★ when the task or
/// one of its artifacts is pinned.
#[derive(Clone, Copy, Default)]
pub(crate) struct CardExtra<'a> {
    pub attention: Option<&'a corral_core::attention::Assessment>,
    pub plan_progress: Option<&'a crate::plan::tracker::Progress>,
    pub route: Option<&'a crate::routes::CardRoute>,
    pub finish: Option<&'a Finish>,
    pub pinned: bool,
    /// Review cards: keep-tier artifacts not yet marked reviewed.
    pub unreviewed: usize,
    pub stage: Option<&'a str>,
    /// Ready cards: the shared dispatch-queue admission reason (`eligible` or the exact wait).
    pub wait: Option<&'a str>,
    pub automation_block: Option<&'a str>,
    /// Plan-first cards: why automation will not start the submitted plan's review.
    pub plan_review_wait: Option<&'a str>,
    /// Ids of the board's Done tasks; without it, open dependencies are found by scanning `all`.
    pub done_ids: Option<&'a std::collections::HashSet<String>>,
}

fn ago(ts: Timestamp, now: Timestamp) -> String {
    corral_core::age_text(now.as_second() - ts.as_second())
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

/// A Ready card's admission color by how much it needs the owner: routine waits stay quiet, a held machine or
/// paused dispatch is yellow, and anything the queue cannot explain is red.
pub(super) fn wait_color(reason: &str) -> Color {
    if reason == "eligible" {
        theme::GREEN
    } else if reason.starts_with("waiting on ") {
        theme::DIM
    } else if reason.starts_with("waiting for ") || reason == "launch reservation held" {
        theme::BLUE
    } else if reason.starts_with("waiting: machine busy")
        || reason == "dispatch paused"
        || reason.starts_with("dequeued")
        || reason.starts_with("project is not admitted")
    {
        theme::YELLOW
    } else {
        theme::RED
    }
}

/// A Ready card's admission state in [`wait_color`].
fn wait_lines(reason: &str, width: usize) -> Vec<Line<'static>> {
    let color = wait_color(reason);
    let (glyph, text) = if reason == "eligible" {
        ("▸", "eligible, dispatches on next sync")
    } else if reason.starts_with(MACHINE_BUSY) {
        ("⧗", MACHINE_BUSY)
    } else {
        ("⧗", reason)
    };
    // The glyph leads the first word, and yields to it on the narrowest cards.
    let glyph_fits = text.split(' ').next().unwrap_or_default().width() + 2 <= width;
    let mut lines = wrap2(text, if glyph_fits { width - 2 } else { width });
    if glyph_fits && let Some(first) = lines.first_mut() {
        *first = format!("{glyph} {first}");
    }
    lines
        .into_iter()
        .map(|line| Line::styled(line, Style::new().fg(color)))
        .collect()
}

/// `text` (the profile, or a run's harness·model·effort) in the profile's color.
fn agent_span(agent: &str, text: &str) -> Span<'static> {
    Span::styled(text.to_string(), Style::new().fg(theme::agent_color(agent)))
}

/// What the owner can do about an automation refusal, in a few words; the task detail keeps the exact reason.
fn automation_short(reason: &str) -> String {
    let detail = reason
        .split_once(": ")
        .filter(|(head, _)| head.starts_with("automatic "))
        .map_or(reason, |(_, rest)| rest);
    let first = detail.split("; ").next().unwrap_or(detail);
    if first.contains("outside the confirmed workspace grant") {
        "not in Autopilot · O to grant".into()
    } else if first.contains("no matching confirmed workspace grant") {
        "Autopilot is off · O to grant".into()
    } else if first.contains("grant expired") {
        "Autopilot grant ended · O to renew".into()
    } else if first.contains("operating policy is paused") {
        "Autopilot paused".into()
    } else if first.contains("requires owner disposition") {
        "needs your decision".into()
    } else {
        first.to_owned()
    }
}

fn automation_lines(reason: Option<&str>, width: usize) -> Vec<Line<'static>> {
    reason
        .map(|reason| {
            Line::styled(
                truncate(&format!("⚠ {}", automation_short(reason)), width),
                Style::new().fg(theme::YELLOW),
            )
        })
        .into_iter()
        .collect()
}

/// Agent questions can arrive as raw JSON or lead with a `Source:` path; show the question itself.
fn question_text(raw: &str) -> String {
    let readable = crate::app::operating::readable_question(raw);
    readable
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("Source:"))
        .unwrap_or("the agent is waiting for you")
        .to_owned()
}

pub(super) fn tracker_progress(t: &Task, is_done: &dyn Fn(&str) -> bool) -> String {
    let done = t.children.iter().filter(|id| is_done(id)).count();
    format!("{done}/{} done", t.children.len())
}

fn meta_lines(
    t: &Task,
    done: &dyn Fn(&str) -> bool,
    config: &Config,
    width: usize,
    now: Timestamp,
    extra: CardExtra,
) -> Vec<Line<'static>> {
    if t.tracker {
        if let Some(progress) = extra.plan_progress {
            return crate::plan::tracker::card_lines(progress, width);
        }
        return vec![Line::styled(
            tracker_progress(t, done),
            Style::new().fg(theme::CYAN),
        )];
    }
    let plan = crate::plan::stage(t, Some(config));
    if let Some(stage) = plan.filter(|s| s.active() && t.current_run().is_none()) {
        let waiting = extra.plan_review_wait.map(automation_short);
        return crate::plan::card_lines(t, stage, width, now, waiting.as_deref());
    }
    let dim = theme::dim();
    let sep = || Span::styled(" · ", theme::dim());
    let open_deps: Vec<&String> = t.depends_on.iter().filter(|d| !done(d)).collect();
    let deps = |spans: &mut Vec<Span<'static>>| {
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
            deps(&mut spans);
            out.push(Line::from(spans));
        }
        Status::Backlog => {
            let mut spans = Vec::new();
            deps(&mut spans);
            if !spans.is_empty() {
                out.push(Line::from(spans));
            }
        }
        Status::Ready => {
            let mut spans = vec![Span::styled("⇢ ", dim), who_span.clone()];
            deps(&mut spans);
            out.push(Line::from(spans));
            out.extend(
                extra
                    .wait
                    .map(|reason| wait_lines(reason, width))
                    .unwrap_or_default(),
            );
        }
        Status::Running => {
            let mut spans = vec![
                who_span.clone(),
                Span::styled(" ⟳ ", Style::new().fg(theme::BLUE)),
            ];
            if let Some(r) = run {
                spans.push(Span::styled(ago(r.started, now), dim));
                if r.stuck.is_none() {
                    if let Some(label) = obligations::parked_label(&t.id, r) {
                        spans.push(sep());
                        spans.push(Span::styled(label, Style::new().fg(theme::BLUE)));
                    } else if obligations::waiting(r) {
                        spans.push(sep());
                        spans.push(Span::styled("⧗ waiting", Style::new().fg(theme::BLUE)));
                    } else if r.idle_no_report {
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
            if let Some(stage) = extra.stage {
                out.extend(autopilot::active_lines(stage, width));
            }
            out.extend(run.and_then(|r| autopilot::changes_line(t, r, config)));
            if let Some(s) = run.and_then(|r| r.stuck.as_ref()) {
                out.push(Line::styled(
                    format!("⚠ stuck: {}", s.argv.join(" ")),
                    Style::new().fg(theme::RED),
                ));
            }
            out.extend(
                run.map(|r| obligations::card_lines(r, true))
                    .unwrap_or_default(),
            );
        }
        Status::NeedsMe => {
            out.push(Line::from(vec![
                who_span.clone(),
                Span::styled(" ? ", Style::new().fg(theme::YELLOW)),
                Span::styled(format!("waiting {}", ago(t.updated, now)), dim),
            ]));
            // An exact owner question beats the escalation reason and the terminal-excerpt heuristic.
            let decision = run.and_then(|r| r.obligations.pending_decision());
            let question = decision.map(|d| d.request.question.clone());
            let question = question
                .or_else(|| autopilot::escalation(t))
                .unwrap_or_else(|| {
                    run.and_then(|r| r.blocked_excerpt.as_deref())
                        .and_then(|e| e.lines().map(str::trim).rfind(|l| !l.is_empty()))
                        .unwrap_or("the agent is waiting for you")
                        .to_string()
                });
            let question = question_text(&question);
            let style = Style::new()
                .fg(theme::YELLOW)
                .add_modifier(Modifier::ITALIC);
            out.extend(
                wrap2(&format!("\"{question}\""), width)
                    .into_iter()
                    .map(|l| Line::styled(l, style)),
            );
            if let Some(d) = decision {
                out.extend(
                    wrap2(&format!("→ {}", d.request.requested_action), width)
                        .into_iter()
                        .map(|l| Line::styled(l, Style::new().fg(theme::FG))),
                );
            }
            out.extend(
                run.map(|r| obligations::card_lines(r, false))
                    .unwrap_or_default(),
            );
        }
        Status::Review => {
            out.extend(automation_lines(extra.automation_block, width));
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
            if let Some(stage) = extra.stage {
                out.extend(autopilot::active_lines(stage, width));
            } else {
                out.extend(autopilot::review_lines(t, config, width));
            }
            if extra.unreviewed > 0 {
                out.push(Line::styled(
                    format!("● {} new", extra.unreviewed),
                    Style::new().fg(theme::YELLOW).add_modifier(Modifier::BOLD),
                ));
            }
        }
        Status::Done => out.extend(done_lines(t, extra)),
        Status::Dropped => {}
    }
    out.extend(
        plan.into_iter()
            .flat_map(|s| crate::plan::stage_lines(s, width)),
    );
    if run.is_some_and(|r| {
        r.pending_reply.is_some()
            || r.review_runs
                .last()
                .is_some_and(|review| review.pending_reply.is_some())
    }) {
        out.extend(
            wrap2("reply not delivered", width)
                .into_iter()
                .map(|line| Line::styled(line, Style::new().fg(theme::RED))),
        );
    }
    if let Some(progress) = run.and_then(|r| r.progress.as_ref()) {
        out.push(Line::styled(
            compact_progress(progress, width),
            Style::new().fg(theme::CYAN),
        ));
    }
    out
}

/// Reserve the count before shortening a phase label, including labels containing spaces.
fn compact_progress(progress: &RunProgress, width: usize) -> String {
    let summary = progress.summary();
    match summary.rsplit_once(' ') {
        Some((label, counts)) => {
            let label = truncate(label, width.saturating_sub(counts.width() + 1));
            if label.is_empty() {
                counts.to_string()
            } else {
                format!("{label} {counts}")
            }
        }
        None => truncate(&summary, width),
    }
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

pub(crate) fn card_lines(
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
    let route_warning = config.ui.route_warnings && extra.route.is_some_and(|r| r.warning);
    let star = match (extra.pinned, route_warning) {
        (true, true) => "★ ⚠ ",
        (true, false) => "★ ",
        (false, true) => "⚠ ",
        (false, false) => "",
    };
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
    let scan = corral_core::done_in(all);
    let indexed = |id: &str| extra.done_ids.is_some_and(|ids| ids.contains(id));
    let done: &dyn Fn(&str) -> bool = match extra.done_ids {
        Some(_) => &indexed,
        None => &scan,
    };
    let card_cfg = &config.board.card;
    let attention = extra.attention.and_then(|assessment| {
        crate::attention::card_line(assessment, t, config, width, now).map(|mut line| {
            line.spans.insert(0, bar.clone());
            line
        })
    });
    if config.ui.compact_cards || card_cfg.density == Density::Compact {
        let plan = crate::plan::stage(t, Some(config));
        let plan = plan.map(crate::plan::PlanStage::label);
        let compact = compact::Compact {
            plan_progress: extra.plan_progress,
            t,
            done,
            cfg: card_cfg,
            now,
            pinned: extra.pinned,
            stage: extra.stage.or(plan.as_deref()),
            wait: extra.wait.filter(|_| t.status == Status::Ready),
            plan_wait: extra.plan_review_wait.is_some(),
            route: extra.route,
            route_warning,
        };
        let mut lines = vec![compact.line(bar.clone(), width, title_style)];
        if let Some(progress) = extra.plan_progress {
            for mut line in crate::plan::tracker::card_lines(progress, width) {
                line.spans.insert(0, bar.clone());
                lines.push(line);
            }
        }
        for mut line in automation_lines(extra.automation_block, width) {
            line.spans.insert(0, bar.clone());
            lines.push(line);
        }
        lines.extend(attention);
        return lines;
    }
    let mut lines = vec![
        r1,
        Line::from(vec![
            bar.clone(),
            Span::styled(truncate(&t.title, width), title_style),
        ]),
    ];
    for meta in meta_lines(t, done, config, width, now, extra) {
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
    if !t.tracker
        && let Some(fields) = card_fields::field_spans(
            t,
            done,
            card_cfg,
            width,
            now,
            extra.pinned,
            extra.route.map(|r| r.label.as_str()),
        )
    {
        let mut spans = vec![bar.clone()];
        spans.extend(fields);
        lines.push(Line::from(spans));
    }
    if !t.tracker
        && card_cfg.density == Density::Rich
        && let Some(bar_text) = t
            .current_run()
            .and_then(|r| r.progress.as_ref())
            .and_then(|p| card_fields::progress_bar(p, width))
    {
        lines.push(Line::from(vec![
            bar,
            Span::styled(bar_text, Style::new().fg(theme::CYAN)),
        ]));
    }
    lines.extend(attention);
    lines
}
