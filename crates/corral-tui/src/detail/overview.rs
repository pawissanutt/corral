//! Owner-facing task context. Run prose is activity, never verification evidence.
use corral_core::{GateResult, Status, Task};
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{DetailView, PLAN, Z_CONTENT, header};
use crate::artifacts::ui;
use crate::{Action, Hit, HitMap, hit, theme};

/// An excerpt of the canonical goal, not an inferred or generated task description.
pub(crate) fn purpose(task: &Task) -> String {
    let goal = task.body.lines().position(|line| {
        let text = line.trim();
        text.starts_with('#')
            && matches!(
                text.trim_start_matches('#')
                    .trim()
                    .to_ascii_lowercase()
                    .as_str(),
                "goal" | "purpose"
            )
    });
    let mut selected = Vec::new();
    for line in task.body.lines().skip(goal.map_or(0, |i| i + 1)) {
        let text = line.trim();
        if text.starts_with('#') {
            if !selected.is_empty() || goal.is_some() {
                break;
            }
            continue;
        }
        if text.is_empty() {
            if !selected.is_empty() {
                break;
            }
        } else {
            selected.push(text);
        }
    }
    let text = selected.join(" ");
    if text.is_empty() {
        "No task description yet. Open the spec for the original request.".into()
    } else {
        excerpt(&text, 420)
    }
}

fn excerpt(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() > max {
        format!("{}...", text.chars().take(max).collect::<String>())
    } else {
        text
    }
}

fn section(lines: &mut Vec<Line<'static>>, title: &str, text: &str) {
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    lines.push(Line::styled(title.to_string(), theme::bold()));
    lines.extend(ui::text_lines(text));
}

impl DetailView {
    pub(super) fn overview_hints(&self) -> Vec<(&'static str, &'static str)> {
        let mut hints = Vec::new();
        if self.task.awaiting_owner() {
            hints.push(("r", "review"));
        } else if self.task.status.holds_slot() {
            hints.push(("a", "open agent"));
        }
        if self.plan.enabled {
            hints.push(("8", "plan / children"));
        }
        hints.extend([
            ("1", "spec"),
            ("2", "reports"),
            ("3", "evidence"),
            ("5", "checks"),
            ("6", "comments"),
            ("7", "history"),
            ("L", "live"),
            ("j/k", "scroll"),
            (
                "*",
                if self.pins.contains(&self.task.id) {
                    "unpin"
                } else {
                    "pin"
                },
            ),
            ("esc", "back"),
        ]);
        hints
    }

    fn overview_lines(&self) -> Vec<Line<'static>> {
        let task = &self.task;
        let run = task.current_run();
        let mut lines = Vec::new();
        section(&mut lines, "What this is for", &purpose(task));
        let state = match task.status {
            Status::Idea => "Idea - not scheduled.",
            Status::Backlog => "Not started.",
            Status::Ready => "Ready for execution.",
            Status::Running => "Work is in progress.",
            Status::NeedsMe => "Work needs attention.",
            Status::Review => "Submitted for review; not yet accepted.",
            Status::Done => "Accepted.",
            Status::Dropped => "Dropped.",
        };
        section(&mut lines, "Where we are", state);
        if task.tracker {
            lines.push(Line::from(format!(
                "{} child tasks. Open Plan [8] for their status.",
                task.children.len()
            )));
        }
        match &task.result {
            Some(result) => {
                let current = corral_ops::completion_spec_digest(task)
                    .is_ok_and(|digest| digest == result.spec_digest);
                lines.push(Line::styled(
                    format!(
                        "Outcome reported in {}{}",
                        result.run,
                        if current {
                            ""
                        } else {
                            " for an older or unverifiable spec"
                        }
                    ),
                    if current {
                        theme::dim()
                    } else {
                        Style::new().fg(theme::YELLOW)
                    },
                ));
                lines.extend(ui::text_lines(&excerpt(&result.text, 600)));
                lines.push(Line::styled(
                    "Full outcome and run reports: [2].",
                    theme::dim(),
                ));
            }
            None => lines.push(Line::styled(
                "No whole-task outcome recorded yet.",
                theme::dim(),
            )),
        }
        let decisions: Vec<_> = run
            .into_iter()
            .filter(|r| r.outcome.is_none())
            .flat_map(|r| &r.obligations.decisions)
            .filter(|d| d.pending())
            .collect();
        let need = if !decisions.is_empty() {
            decisions
                .iter()
                .map(|d| format!("{}\n{}", d.request.question, d.request.requested_action))
                .collect::<Vec<_>>()
                .join("\n")
        } else if task.awaiting_owner() {
            "Review the result and evidence [r].".into()
        } else if task.status == Status::NeedsMe {
            run.and_then(|r| r.blocked_excerpt.clone())
                .unwrap_or_else(|| "Open the agent [a] to inspect the blocker.".into())
        } else {
            "No owner action requested.".into()
        };
        section(&mut lines, "Needs you", &need);
        lines.push(Line::default());
        lines.extend(crate::attention::waiting(
            task,
            &self.attention,
            jiff::Timestamp::now(),
        ));
        if let Some(stuck) = self.stuck() {
            lines.push(Line::styled(
                format!(
                    "Stuck: {}. Open Runs [2] to inspect or unstick.",
                    stuck.argv.join(" ")
                ),
                Style::new().fg(theme::YELLOW),
            ));
        }
        if let Some(run) = run.filter(|r| r.outcome.is_none()) {
            lines.extend(crate::obligations::card_lines(run, false));
        }
        let gate = run
            .and_then(|r| r.gate.as_ref())
            .map_or("not recorded", |g| {
                if g.process_lost {
                    "process lost"
                } else {
                    match g.result {
                        GateResult::Running => "running",
                        GateResult::Pass => "passed",
                        GateResult::Fail => "failed",
                        GateResult::Stale => "stale",
                    }
                }
            });
        let review = run
            .and_then(|r| r.review.as_ref())
            .map_or("not recorded", |r| {
                if r.process_lost {
                    "process lost"
                } else {
                    r.verdict.map_or("pending", |v| v.as_str())
                }
            });
        section(
            &mut lines,
            "Recorded checks",
            &format!("Gate: {gate} | Review: {review}"),
        );
        lines.push(Line::styled(
            "Recorded results, not revalidated here. Open Acceptance [5] for receipts.",
            theme::dim(),
        ));
        if let Some(summary) = run.and_then(|r| r.summary.as_deref()) {
            let activity = summary
                .lines()
                .take_while(|l| !l.starts_with("Verified:") && !l.starts_with("Not verified:"))
                .collect::<Vec<_>>()
                .join(" ");
            section(&mut lines, "Latest activity", &excerpt(&activity, 300));
            lines.push(Line::styled(
                "Full run report and commands: Runs [2]. Earlier reports: Evidence [3].",
                theme::dim(),
            ));
        }
        lines
    }

    pub(super) fn draw_overview(&mut self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        ui::paint_bg(f, area);
        let [head, title, controls, _, body] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .horizontal_margin(1)
        .areas(area);
        header::draw(
            f,
            head,
            Line::from(format!(
                "{}{}  Overview",
                self.task.id,
                if self.pins.contains(&self.task.id) {
                    " *"
                } else {
                    ""
                }
            )),
            self.task.parent.as_deref(),
            hits,
        );
        f.render_widget(
            Paragraph::new(self.task.title.as_str())
                .style(theme::bold())
                .wrap(ratatui::widgets::Wrap { trim: false }),
            title,
        );
        let mut x = controls.x;
        let mut y = controls.y;
        for (key, label) in self
            .overview_hints()
            .into_iter()
            .filter(|(k, _)| k.len() == 1)
        {
            let text = format!("[{key}] {label}  ");
            let width = u16::try_from(text.len())
                .unwrap_or(controls.width)
                .min(controls.width);
            if x + width > controls.right() {
                x = controls.x;
                y += 1;
            }
            if y >= controls.bottom() {
                break;
            }
            let rect = Rect::new(x, y, width, 1);
            f.render_widget(Span::styled(text, Style::new().fg(theme::CYAN)), rect);
            if let Some(c) = key.chars().next() {
                hits.push(rect, Hit::Key(hit::press(KeyCode::Char(c))));
            }
            x += width;
        }
        let content = ui::boxed(f, body, "Task overview", true, None);
        hits.push(content, Hit::Item(Z_CONTENT, 0));
        ui::scrolled(f, content, self.overview_lines(), &mut self.scroll, true);
    }

    pub(super) fn overview_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc => Action::Back,
            KeyCode::Char('r') if self.task.awaiting_owner() => {
                Action::OpenReview(self.task.id.clone())
            }
            KeyCode::Char('a') if self.task.status.holds_slot() => {
                Action::Jump(self.task.id.clone())
            }
            KeyCode::Char('P') => self
                .task
                .parent
                .as_ref()
                .map_or(Action::None, |id| Action::OpenDetail(id.clone())),
            KeyCode::Char('L') => {
                self.set_tab(0, ctx);
                self.show_live = true;
                Action::None
            }
            KeyCode::Char('*') => crate::shelf::toggle(ctx, &self.task.id),
            KeyCode::Char(c @ '1'..='8') => {
                let tab = usize::try_from(u32::from(c) - u32::from('1')).unwrap_or(0);
                if tab < self.tab_names().len() {
                    self.set_tab(tab, ctx);
                }
                Action::None
            }
            KeyCode::Tab | KeyCode::Enter if self.task.tracker => {
                self.set_tab(PLAN, ctx);
                Action::None
            }
            KeyCode::Tab => {
                self.set_tab(0, ctx);
                Action::None
            }
            _ => {
                self.scroll.on_key(&key, 10);
                Action::None
            }
        }
    }
}
