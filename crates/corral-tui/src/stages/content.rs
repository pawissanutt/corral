use corral_core::{ArtifactKind, Run};
use corral_ops::watch::{ProgressState, ReviewerAttempt};
use corral_ops::{Ctx, Preview};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use super::{Stage, StageView, gate, strip};
use crate::HitMap;
use crate::artifacts::ui;
use crate::render::{self, Format};

const LOG_BYTES: usize = 256 * 1024;

impl StageView {
    pub(super) fn review_index(&self) -> usize {
        self.review_sel
            .as_ref()
            .and_then(|key| {
                self.history
                    .reviews
                    .iter()
                    .position(|r| r.accounting.id == *key)
            })
            .unwrap_or(self.history.reviews.len().saturating_sub(1))
    }

    fn selected_review(&self) -> Option<&ReviewerAttempt> {
        self.history.reviews.get(self.review_index())
    }

    pub(super) fn load_content(&mut self, ctx: &Ctx) {
        let loaded = match self.stage {
            Stage::Gate => self.gate_content(ctx),
            Stage::Review => self.reviewer_content(ctx),
            Stage::Agent => self.agent_content(ctx),
            Stage::Land => {
                self.source = "acceptance record".into();
                self.text = self
                    .task
                    .current_run()
                    .and_then(|r| r.autopilot.as_ref())
                    .map(|a| {
                        format!(
                            "## Landing\n\nStage: {:?}\n\n{}\n\nUpdated: {}",
                            a.stage,
                            a.reason.as_deref().unwrap_or(""),
                            a.at
                        )
                    })
                    .unwrap_or_else(|| {
                        "Landing has not started. See Acceptance for policy and records.".into()
                    });
                Ok(())
            }
        };
        if let Err(error) = loaded {
            self.source = "unavailable".into();
            self.text = format!("Unable to read stage output: {error:#}");
        }
    }

    fn gate_content(&mut self, ctx: &Ctx) -> anyhow::Result<()> {
        let Some(g) = self.selected_gate() else {
            self.text.clear();
            return Ok(());
        };
        let key = (g.run.clone(), g.attempt.clone());
        let new_failure = g.state == ProgressState::Failed && self.failed.as_ref() != Some(&key);
        if new_failure {
            self.step = None;
        }
        let Some(g) = self.selected_gate() else {
            return Ok(());
        };
        let Some(command) = g.commands.get(self.step_index()) else {
            self.text.clear();
            return Ok(());
        };
        let end = if command.state == ProgressState::Running {
            std::fs::metadata(&g.log)
                .map(|m| m.len())
                .unwrap_or(command.log_end)
        } else {
            command.log_end
        };
        let mut start = end
            .saturating_sub(u64::try_from(LOG_BYTES)?)
            .max(command.log_start);
        if !self.follow
            && let Some(previous) = self.window_start
        {
            start = previous.max(command.log_start).min(end);
        }
        // Failure opens at the first error, even when it precedes the live tail window.
        let mut jump = None;
        if new_failure {
            let header = format!("\n$ {}\n", command.command);
            let mut position = command
                .log_start
                .saturating_add(u64::try_from(header.len())?);
            while position < end {
                let bytes = corral_ops::watch::read_gate_log(ctx, g, position, LOG_BYTES)?;
                if bytes.is_empty() {
                    break;
                }
                let text = String::from_utf8_lossy(&bytes);
                if let Some(line) = render::first_error_line(&text) {
                    start = position;
                    jump = Some(line);
                    break;
                }
                position = position.saturating_add(u64::try_from(bytes.len())?);
            }
        }
        let cap = usize::try_from(end.saturating_sub(start))?.min(LOG_BYTES);
        let bytes = corral_ops::watch::read_gate_log(ctx, g, start, cap)?;
        self.source = format!(
            "{}; bytes {start}..{}",
            if g.live_pane.is_some() {
                "live"
            } else {
                "saved log"
            },
            start.saturating_add(u64::try_from(bytes.len())?)
        );
        self.window_start = Some(start);
        self.text = String::from_utf8_lossy(&bytes).into_owned();
        if new_failure {
            self.failed = Some(key);
            self.follow = false;
            self.scroll.offset = jump.unwrap_or(0);
        }
        Ok(())
    }

    fn reviewer_content(&mut self, ctx: &Ctx) -> anyhow::Result<()> {
        let Some(review) = self.selected_review() else {
            self.source = "No reviewer attempts yet".into();
            self.text =
                "The reviewer screen and verdict will appear here when review starts.".into();
            return Ok(());
        };
        if let Some(pane) = &review.live_pane {
            let text = ctx.herdr.pane_read(pane, 500)?;
            self.source = format!("live screen {pane}");
            self.text = text;
        } else {
            let mut text = String::new();
            for artifact in &review.artifacts {
                if matches!(artifact.kind, ArtifactKind::Log | ArtifactKind::Evidence)
                    && let Preview::Text(body) = corral_ops::artifact_document(ctx, artifact)?
                {
                    text.push_str(&format!(
                        "## {}\n\n```text\n{body}\n```\n\n",
                        artifact.title
                    ));
                }
            }
            self.source = "retained transcript / screen".into();
            self.text = if text.is_empty() {
                "No captured screen or transcript is available for this reviewer.".into()
            } else {
                text
            };
        }
        let completed = self
            .selected_review()
            .and_then(|r| r.record.as_ref())
            .filter(|r| r.verdict.is_some())
            .map(|r| (r.verdict, r.notes.clone()));
        if let Some((verdict, notes)) = completed {
            let id = self.selected_review().map(|r| r.accounting.id.clone());
            self.text = format!(
                "## Verdict: {}\n\n{}\n\n{}",
                verdict.map_or("pending", |v| v.as_str()),
                notes.unwrap_or_default(),
                self.text
            );
            if self.completed_review != id {
                self.follow = false;
                self.scroll = ui::Scroll::default();
                self.completed_review = id;
            }
        }
        Ok(())
    }

    fn agent_content(&mut self, ctx: &Ctx) -> anyhow::Result<()> {
        let Some(run) = self.task.current_run() else {
            self.text = "No agent run yet.".into();
            return Ok(());
        };
        if run.ended.is_none()
            && let Some(pane) = &run.herdr
            && ctx.herdr.pane_get(&pane.pane_id)?.is_some()
        {
            self.text = ctx.herdr.pane_read(&pane.pane_id, 500)?;
            self.source = format!("live {}", pane.pane_id);
        } else {
            self.text = run
                .summary
                .clone()
                .unwrap_or_else(|| "No captured agent output.".into());
            self.source = "saved summary".into();
        }
        Ok(())
    }

    pub(super) fn draw_reviewer(&mut self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        let mut metadata = Vec::new();
        if let Some(review) = self.selected_review() {
            let run: &Run = &review.accounting;
            let status = gate::state(review.state).0;
            metadata.push(Line::from(format!(
                "{} - round {} - reviewer {} - {status}",
                review.run,
                review.round + 1,
                self.review_index() + 1
            )));
            metadata.push(Line::from(format!(
                "Profile: {}   Model: {}",
                run.profile.as_deref().unwrap_or(&run.agent),
                run.model.as_deref().unwrap_or("unknown")
            )));
            metadata.push(Line::from(format!(
                "Elapsed: {}   {}   {}",
                gate::duration(Some(run.started), run.ended),
                strip::cost(run.usage.as_ref()),
                self.source
            )));
            if let Some(record) = &review.record {
                metadata.push(Line::from(format!(
                    "Verdict: {}",
                    record.verdict.map_or("pending", |v| v.as_str())
                )));
            }
        } else {
            metadata.push(Line::from("No reviewer attempts yet."));
        }
        let metadata = Paragraph::new(metadata).wrap(Wrap { trim: false });
        let [header, screen] = Layout::vertical([
            Constraint::Length(u16::try_from(metadata.line_count(area.width)).unwrap_or(u16::MAX)),
            Constraint::Fill(1),
        ])
        .areas(area);
        f.render_widget(metadata, header);
        let inner = ui::boxed(
            f,
            screen,
            "Reviewer screen - read-only",
            true,
            Some(self.source.clone()),
        );
        self.draw_text(
            f,
            inner,
            if self.source.starts_with("live") {
                Format::Log
            } else {
                Format::Markdown
            },
            hits,
        );
    }
}
