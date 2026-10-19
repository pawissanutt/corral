//! Finite owner selection is separate from ordinary staged configuration.
use corral_core::{OperatingPreview, OperatingSelection};
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use super::SettingsView;
use super::model::{Field, Kind, Row, Section};
use crate::hit::HitMap;
use crate::overlays::{self, LineInput};
use crate::{Action, Toast, Tone, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Projects,
    Tasks,
    Exclusions,
    PlanScopes,
    Dispatch,
    Accept,
    PlanAccept,
    Preview,
}

#[derive(Default)]
pub(super) struct Controls {
    pub selection: OperatingSelection,
    pub preview: Option<OperatingPreview>,
    pub input: LineInput,
    pub error: Option<String>,
    pub scroll: u16,
}

impl Controls {
    pub fn rows(&self) -> Vec<Row> {
        let selection = &self.selection;
        let list = |values: &std::collections::BTreeSet<String>| {
            values.iter().cloned().collect::<Vec<_>>().join(", ")
        };
        let row = |key, label, kind, value| Row::new(Field::Operating(key), label, kind, value);
        vec![
            row(Key::Projects, "Grant projects", Kind::Text { optional: true }, list(&selection.projects))
                .help("Comma-separated projects; snapshots current Ready/Backlog tasks only."),
            row(Key::Tasks, "Grant tasks", Kind::Text { optional: true }, list(&selection.tasks))
                .help("Comma-separated task IDs. Future tasks are never included."),
            row(Key::Exclusions, "Exclude tasks", Kind::Text { optional: true }, list(&selection.exclusions))
                .help("Comma-separated task IDs to exclude from this finite grant."),
            row(Key::PlanScopes, "Reviewed scopes", Kind::Text { optional: true },
                selection.plan_scopes.iter().flat_map(|(task, labels)| labels.iter().map(move |label| format!("{task}:{label}"))).collect::<Vec<_>>().join(", "))
                .help("Comma-separated TASK:LABEL entries from independently reviewed plans."),
            row(Key::Dispatch, "Delegate dispatch", Kind::Toggle, if selection.authority.dispatch { "on" } else { "off" }.into())
                .help("Also required for automatic gate, review and repair. Existing verification still applies."),
            row(Key::Accept, "Delegate landing", Kind::Toggle, if selection.authority.accept { "on" } else { "off" }.into())
                .help("Existing owner-only acceptance exceptions still apply."),
            row(Key::PlanAccept, "Delegate plan acceptance", Kind::Toggle, if selection.authority.plan_accept { "on" } else { "off" }.into()),
            row(Key::Preview, "Preview finite grant", Kind::Entry, "review exact scope and digest".into())
                .help("Owner only. Saving settings does not grant authority; type the exact digest after review."),
        ]
    }

    pub fn set(&mut self, key: Key, text: &str) -> Result<(), String> {
        let values = || {
            text.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        };
        match key {
            Key::Projects => self.selection.projects = values(),
            Key::Tasks => self.selection.tasks = values(),
            Key::Exclusions => self.selection.exclusions = values(),
            Key::PlanScopes => {
                let mut scopes =
                    std::collections::BTreeMap::<String, std::collections::BTreeSet<String>>::new();
                for item in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    let (task, label) = item
                        .split_once(':')
                        .ok_or("use TASK:LABEL for reviewed scopes")?;
                    if label.is_empty() {
                        return Err("scope label must not be empty".into());
                    }
                    let task = corral_core::Store::normalize_id(task).map_err(|e| e.to_string())?;
                    scopes.entry(task).or_default().insert(label.to_owned());
                }
                self.selection.plan_scopes = scopes;
            }
            Key::Dispatch => self.selection.authority.dispatch = text == "on",
            Key::Accept => self.selection.authority.accept = text == "on",
            Key::PlanAccept => self.selection.authority.plan_accept = text == "on",
            Key::Preview => return Err("open preview to review this grant".into()),
        }
        self.preview = None;
        Ok(())
    }
}

impl SettingsView {
    pub(super) fn operating_preview(&mut self, ctx: &Ctx) -> Action {
        let result = (|| {
            anyhow::ensure!(
                !self.dirty(),
                "save or discard other settings before previewing authority"
            );
            corral_ops::require_automation_owner(&ctx.store)?;
            corral_ops::automation_preview(&ctx.store, &self.operating.selection)
        })();
        match result {
            Ok(preview) => {
                self.operating.preview = Some(preview);
                self.operating.input.text.clear();
                self.operating.error = None;
                self.operating.scroll = 0;
                Action::None
            }
            Err(error) => Action::Toast(Toast::new(
                Tone::Bad,
                "Grant not previewed",
                format!("{error:#}"),
            )),
        }
    }

    pub(super) fn operating_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc => self.operating.preview = None,
            KeyCode::PageDown => self.operating.scroll = self.operating.scroll.saturating_add(8),
            KeyCode::PageUp => self.operating.scroll = self.operating.scroll.saturating_sub(8),
            KeyCode::Enter => {
                let Some(preview) = &self.operating.preview else {
                    return Action::None;
                };
                match corral_ops::automation_confirm(
                    &ctx.store,
                    preview,
                    &self.operating.input.text,
                ) {
                    Ok(config) => {
                        self.base = config.clone();
                        self.draft = config;
                        self.operating.preview = None;
                        return Action::Toast(Toast::new(
                            Tone::Good,
                            "Finite grant confirmed",
                            "Only the reviewed scope and delegated actions are authorized.",
                        ));
                    }
                    Err(error) => self.operating.error = Some(format!("{error:#}")),
                }
            }
            _ => {
                self.operating.input.edit(&key);
                self.operating.error = None;
            }
        }
        Action::None
    }

    pub(super) fn draw_operating(&mut self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        let Some(preview) = &self.operating.preview else {
            return;
        };
        overlays::dim_backdrop(f.buffer_mut(), area);
        let rect = overlays::centered(area, 108, area.height.saturating_sub(2));
        let inner = overlays::popup(f, rect, "review finite operating grant", hits);
        let body_height = inner.height.saturating_sub(10);
        let mut lines = vec![
            Line::raw(format!("Workspace: {}", preview.workspace.display())),
            Line::raw(format!(
                "Dispatch: {} | Landing: {} | Plan acceptance: {}",
                preview.policy.authority.dispatch,
                preview.policy.authority.accept,
                preview.policy.authority.plan_accept
            )),
        ];
        for (id, member) in &preview.policy.scope.tasks {
            lines.push(Line::raw(format!(
                "{id}: {} / {}",
                member.project,
                member.repository.display()
            )));
            if let Some(constraints) = preview.constraints.get(id) {
                lines.push(Line::raw(format!(
                    "  {:?}; owner landing: {}; owner plan: {}; dispatch hold: {}",
                    constraints.lifecycle,
                    constraints.owner_acceptance,
                    constraints.owner_plan_acceptance,
                    constraints.dispatch_hold
                )));
                lines.push(Line::raw(format!(
                    "  Gate: {:?}; reviewer: {:?}; rounds: {}; diff limit: {}",
                    constraints.review.gate,
                    constraints.review.reviewer,
                    constraints.review.max_rounds,
                    constraints.review.max_diff_lines
                )));
                let review = &constraints.review;
                lines.push(Line::raw(format!(
                    "  Acceptance: {:?}; plan acceptance: {:?}; plan required: {}",
                    review.accept, review.plan_accept, review.plan_required
                )));
                lines.push(Line::raw(format!(
                    "  Cross-family review: {}; merge: {:?}; remove worktree: {}",
                    review.require_cross_family, review.merge, review.remove_worktree
                )));
                lines.push(Line::raw(format!(
                    "  Owner paths: {}",
                    if review.owner_paths.is_empty() {
                        "none".into()
                    } else {
                        review.owner_paths.join(", ")
                    }
                )));
                lines.push(Line::raw(format!(
                    "  Size limits: {}",
                    review
                        .size_limits
                        .iter()
                        .map(|(extension, limit)| format!("{extension} <= {limit}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
                lines.push(Line::raw(format!(
                    "  Gate environment: {}",
                    if review.env.is_empty() {
                        "none".into()
                    } else {
                        review
                            .env
                            .iter()
                            .map(|(key, value)| format!("{key}={value}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                )));
                lines.push(Line::raw(format!(
                    "  Cancelled plan: {}",
                    constraints.cancelled_plan
                )));
                for decision in &constraints.pending_decisions {
                    lines.push(Line::raw(format!(
                        "  Pending decision: {}",
                        decision.question
                    )));
                }
            }
        }
        lines.push(Line::raw(format!(
            "Exclusions: {:?}",
            preview.policy.scope.exclusions
        )));
        for plan in &preview.policy.scope.plans {
            lines.push(Line::raw(format!(
                "Plan {} @ {} (artifact {}): {:?}",
                plan.task,
                plan.revision,
                plan.artifact_sha256,
                plan.scopes.keys().collect::<Vec<_>>()
            )));
        }
        lines.push(Line::styled(
            "End of finite scope. Existing safety and owner exceptions remain.",
            theme::dim(),
        ));
        // Paragraph wraps long paths and policy details; PageUp/PageDown expose every line.
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        let max_scroll = paragraph
            .line_count(inner.width)
            .saturating_sub(usize::from(body_height));
        self.operating.scroll = self
            .operating
            .scroll
            .min(u16::try_from(max_scroll).unwrap_or(u16::MAX));
        f.render_widget(
            paragraph.scroll((self.operating.scroll, 0)),
            Rect {
                height: body_height,
                ..inner
            },
        );
        let y = inner.y + body_height;
        let footer = vec![
            Line::styled(
                "Confirmation digest (type all characters below):",
                theme::dim(),
            ),
            Line::raw(preview.id.clone()),
            Line::styled("Stored grant digest (approval hash):", theme::dim()),
            Line::raw(preview.grant_digest.clone()),
            Line::raw(format!("Confirm digest: {}▏", self.operating.input.text)),
            Line::styled(
                self.operating.error.clone().unwrap_or_default(),
                ratatui::style::Style::new().fg(theme::RED),
            ),
            Line::raw("Enter confirms | Esc cancels | PgUp/PgDn review scope"),
        ];
        f.render_widget(
            Paragraph::new(footer).wrap(Wrap { trim: false }),
            Rect {
                y,
                height: inner.height.saturating_sub(body_height),
                ..inner
            },
        );
    }

    pub(super) fn operating_rows(&self, rows: &mut Vec<Row>, section: Section) {
        if section == Section::Automation {
            rows.extend(self.operating.rows());
        }
    }
}
