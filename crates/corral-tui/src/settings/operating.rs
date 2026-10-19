//! Finite owner selection is separate from ordinary staged configuration.
use corral_core::{OperatingPreview, OperatingSelection};
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use super::SettingsView;
use super::model::{Field, Kind, Row};
use crate::hit::{self, Hit, HitMap};
use crate::overlays::{self, LineInput};
use crate::{Action, Toast, Tone, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Open,
    Status,
    Assist,
    Pause,
    Resume,
    Fact(usize),
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
    pub stale: bool,
    pub current: Vec<String>,
}

impl Controls {
    pub fn new(ctx: &Ctx) -> Self {
        let policy = &ctx.store.config.automation;
        let mut controls = Self {
            selection: OperatingSelection {
                tasks: policy.scope.tasks.keys().cloned().collect(),
                exclusions: policy.scope.exclusions.clone(),
                plan_scopes: policy
                    .scope
                    .plans
                    .iter()
                    .map(|plan| (plan.task.clone(), plan.scopes.keys().cloned().collect()))
                    .collect(),
                authority: policy.authority.clone(),
                ..Default::default()
            },
            ..Default::default()
        };
        controls.refresh(ctx);
        controls
    }

    pub fn refresh(&mut self, ctx: &Ctx) {
        self.current = super::operating_state::facts(ctx).unwrap_or_else(|error| {
            vec![format!(
                "Policy unavailable; no automatic authority: {error:#}"
            )]
        });
        if let Some(preview) = &self.preview {
            self.stale = !corral_ops::automation_preview(&ctx.store, &preview.selection)
                .is_ok_and(|fresh| fresh.id == preview.id);
        }
    }

    pub fn rows(&self) -> Vec<Row> {
        let selection = &self.selection;
        let list = |values: &std::collections::BTreeSet<String>| {
            values.iter().cloned().collect::<Vec<_>>().join(", ")
        };
        let row = |key, label, kind, value| Row::new(Field::Operating(key), label, kind, value);
        let mut rows = vec![
            row(Key::Status, "Operating mode", Kind::Info, self.current.first().cloned().unwrap_or_default()),
            row(Key::Assist, "Use Assist", Kind::Entry, "withdraw automatic authority".into()),
            row(Key::Pause, "Pause Autopilot", Kind::Entry, "no new actions; keep running work".into()),
            row(Key::Resume, "Resume Autopilot", Kind::Entry, "existing matching grant only".into()),
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
        ];
        rows.extend(
            self.current
                .iter()
                .skip(1)
                .enumerate()
                .map(|(i, value)| row(Key::Fact(i), "Current policy", Kind::Info, value.clone())),
        );
        rows
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
            Key::Open
            | Key::Preview
            | Key::Status
            | Key::Fact(_)
            | Key::Assist
            | Key::Pause
            | Key::Resume => {
                return Err("not a staged selection field".into());
            }
        }
        self.preview = None;
        Ok(())
    }
}

impl SettingsView {
    /// The board key, header pointer target and Automation section share this editor.
    pub(crate) fn operating(ctx: &Ctx) -> Self {
        let mut view = Self::new(ctx);
        view.set_section(5);
        view.operating_only = true;
        view
    }

    pub(super) fn operating_action(&mut self, key: Key, ctx: &Ctx) -> Action {
        if key == Key::Open {
            return Action::OpenOperating;
        }
        if key == Key::Preview {
            return self.operating_preview(ctx);
        }
        let result = (|| {
            anyhow::ensure!(
                !self.dirty(),
                "save or discard other settings before changing operating policy"
            );
            let config = match key {
                Key::Assist => ctx.store.assist_operating_policy()?,
                Key::Pause => ctx.store.pause_operating_policy(true)?,
                Key::Resume => {
                    corral_ops::require_automation_owner(&ctx.store)?;
                    ctx.store.pause_operating_policy(false)?
                }
                _ => return Ok(()),
            };
            self.base = config.clone();
            self.draft = config;
            self.operating.refresh(ctx);
            self.layers = super::layers::Cache::load(&ctx.store, self.layer_loader);
            Ok::<_, anyhow::Error>(())
        })();
        match result {
            Ok(()) => Action::None,
            Err(error) => Action::Toast(Toast::new(
                Tone::Bad,
                "Mode not changed",
                format!("{error:#}"),
            )),
        }
    }

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
                self.operating.stale = false;
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
                        self.operating.refresh(ctx);
                        self.layers = super::layers::Cache::load(&ctx.store, self.layer_loader);
                        return Action::None;
                    }
                    Err(error) => {
                        self.operating.error = Some(format!("{error:#}"));
                        self.operating.refresh(ctx);
                    }
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
        let body_height = inner.height.saturating_sub(7);
        let mut lines = vec![
            Line::raw("Proposed Autopilot; no authority until exact confirmation."),
            Line::raw(format!(
                "Dispatch: {} | Landing: {} | Plan acceptance: {}",
                preview.policy.authority.dispatch,
                preview.policy.authority.accept,
                preview.policy.authority.plan_accept
            )),
            Line::raw(format!(
                "Finite tasks ({}): {}",
                preview.policy.scope.tasks.len(),
                preview
                    .policy
                    .scope
                    .tasks
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            Line::raw(format!(
                "Exclusions: {}",
                preview
                    .policy
                    .scope
                    .exclusions
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            Line::raw(format!(
                "Owner-only landing: {}",
                preview
                    .constraints
                    .iter()
                    .filter(|(_, c)| c.owner_acceptance)
                    .map(|(id, _)| id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            Line::raw(format!(
                "Owner-only plans: {}",
                preview
                    .constraints
                    .iter()
                    .filter(|(_, c)| c.owner_plan_acceptance)
                    .map(|(id, _)| id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        ];
        if self.operating.stale {
            lines.insert(
                0,
                Line::raw("STALE: cancel and preview current policy before confirming."),
            );
        }
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
            "Workspace: {}",
            preview.workspace.display()
        )));
        lines.push(Line::raw(
            self.operating.current.first().cloned().unwrap_or_default(),
        ));
        lines.push(Line::raw(format!(
            "Exclusions: {:?}",
            preview.policy.scope.exclusions
        )));
        lines.push(Line::raw(format!(
            "Stored grant digest: {}",
            preview.grant_digest
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
        let line_count = paragraph.line_count(inner.width);
        let max_scroll = line_count.saturating_sub(usize::from(body_height));
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
        ];
        f.render_widget(
            Paragraph::new(footer),
            Rect::new(inner.x, y, inner.width, 2),
        );
        self.operating.input.draw(
            f,
            Rect::new(inner.x, y + 2, inner.width, 1),
            "Digest:",
            "type exact confirmation digest",
        );
        f.render_widget(
            Paragraph::new(self.operating.error.as_deref().unwrap_or(""))
                .style(ratatui::style::Style::new().fg(theme::RED))
                .wrap(Wrap { trim: false }),
            Rect::new(inner.x, y + 3, inner.width, 2),
        );
        for (offset, label, code) in [
            (0, "[Confirm]", KeyCode::Enter),
            (12, "[Cancel]", KeyCode::Esc),
        ] {
            let rect = Rect::new(
                inner.x + offset,
                y + 5,
                u16::try_from(label.len()).unwrap_or(0),
                1,
            );
            f.render_widget(
                Paragraph::new(label).style(theme::bold().fg(theme::CYAN)),
                rect,
            );
            hits.push(rect, Hit::Key(hit::press(code)));
        }
        f.render_widget(
            Paragraph::new(format!(
                "Scope {}-{}/{} | PgUp/PgDn or wheel | Enter/Esc",
                usize::from(self.operating.scroll) + 1,
                (usize::from(self.operating.scroll) + usize::from(body_height)).min(line_count),
                line_count,
            ))
            .style(theme::dim()),
            Rect::new(inner.x, y + 6, inner.width, 1),
        );
    }
}
