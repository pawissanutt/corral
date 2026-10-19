use corral_core::{RunOutcome, Status};
use corral_ops::Ctx;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::usage::summary as cost_summary;
use super::{DetailView, progress, run_state};
use crate::artifacts::ui;
use crate::choice::run_label;
use crate::theme::{self, BLUE, DIM, FG, GREEN, RED, SELECTED_BG, YELLOW};

impl DetailView {
    pub(super) fn runs_lines(&self, ctx: &Ctx, width: usize) -> Vec<Line<'static>> {
        let t = &self.task;
        if t.runs.is_empty() {
            return vec![Line::from(Span::styled(
                if t.tracker {
                    "Tracker cards have no runs. Dispatch a child task instead."
                } else {
                    "No runs yet. Press d on the board to dispatch."
                },
                theme::dim(),
            ))];
        }
        let cols = [4, 10, 20, 12, 8, 16, 11, 6];
        let head = [
            "run",
            "profile",
            "harness·model·effort",
            "mode",
            "pane",
            "branch",
            "outcome",
            "time",
        ];
        let mut lines = vec![Line::from(
            head.iter()
                .zip(cols)
                .map(|(h, w)| Span::styled(ui::fit(h, w), theme::dim()))
                .collect::<Vec<_>>(),
        )];
        let sel = self.run_index();
        for (i, r) in t.runs.iter().enumerate() {
            let (outcome, color) = match r.outcome {
                Some(RunOutcome::Done) => ("done", GREEN),
                Some(RunOutcome::Superseded) => ("superseded", DIM),
                Some(RunOutcome::Abandoned) => ("abandoned", DIM),
                Some(RunOutcome::Discarded) => ("discarded", RED),
                Some(RunOutcome::Interrupted) => ("interrupted", YELLOW),
                None if t.status == Status::NeedsMe => ("blocked", YELLOW),
                None => ("running", BLUE),
            };
            let mode = format!("{:?}", r.mode).to_lowercase();
            let pane = r
                .herdr
                .as_ref()
                .map(|h| h.pane_id.clone())
                .unwrap_or_else(|| "—".into());
            let mut spans = vec![
                Span::styled(ui::fit(&r.id, cols[0]), Style::new().fg(FG)),
                Span::styled(
                    ui::fit(&r.agent, cols[1]),
                    Style::new().fg(theme::agent_color(&r.agent)),
                ),
                Span::styled(ui::fit(&run_label(r), cols[2]), Style::new().fg(FG)),
                Span::styled(ui::fit(&mode, cols[3]), Style::new().fg(FG)),
                Span::styled(ui::fit(&pane, cols[4]), Style::new().fg(FG)),
                Span::styled(ui::fit(&r.branch, cols[5]), Style::new().fg(FG)),
                Span::styled(ui::fit(outcome, cols[6]), Style::new().fg(color)),
                Span::styled(ui::elapsed(r.started, r.ended), theme::dim()),
            ];
            if i == sel {
                spans
                    .iter_mut()
                    .for_each(|s| s.style = s.style.bg(SELECTED_BG));
            }
            lines.push(Line::from(spans));
            let usage = cost_summary(r.total_usage());
            lines.push(Line::from(Span::styled(
                format!("  cost: {usage}"),
                theme::dim(),
            )));
        }
        let usage = cost_summary(t.total_usage());
        lines.push(Line::from(Span::styled(
            format!("Total: {usage}"),
            theme::dim(),
        )));
        if let Some(r) = t.runs.get(sel) {
            let sha: String = r.base_sha.chars().take(7).collect();
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                format!(
                    "worktree {}{} · base {}@{sha}",
                    ui::rel_path(ctx, &r.worktree),
                    if r.worktree.exists() {
                        ""
                    } else {
                        " (removed)"
                    },
                    r.base
                ),
                theme::dim(),
            )));
            lines.extend(run_state::lines(r, width));
            if let Some(a) = &r.autopilot {
                let round = if t.rounds > 0 {
                    format!(" · round {}", t.rounds)
                } else {
                    String::new()
                };
                let reason = a
                    .reason
                    .as_deref()
                    .and_then(|x| x.lines().find(|l| !l.trim().is_empty()))
                    .map(|x| format!(": {x}"))
                    .unwrap_or_default();
                lines.push(Line::from(Span::styled(
                    format!("autopilot {}{round}{reason}", a.stage.as_str()),
                    Style::new().fg(BLUE),
                )));
            }
            if let Some(result) = &t.result {
                lines.push(Line::default());
                lines.push(Line::styled(
                    format!("Whole-task outcome (reported in {})", result.run),
                    theme::bold(),
                ));
                lines.extend(ui::text_lines(&result.text));
            }
            if let Some(s) = &r.summary {
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(
                    "Run activity and verification",
                    theme::bold(),
                )));
                lines.extend(ui::text_lines(s));
            }
            lines.extend(progress::lines(r, width));
        }
        lines
    }
}
