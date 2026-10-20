//! Finite owner selection is separate from ordinary staged configuration.
mod pick;

use corral_core::{DEFAULT_GRANT_DAYS, OperatingPreview, OperatingSelection};
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use super::SettingsView;
use super::model::{Field, Kind, Row};
use crate::hit::{self, Hit, HitMap};
use crate::overlays;
use crate::{Action, Toast, Tone, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Open,
    Status,
    Overview(usize),
    Assist,
    Pause,
    Resume,
    Details,
    Fact(usize),
    Projects,
    Tasks,
    IncludeNew,
    Expiry,
    MaxNew,
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
    pub pick: Option<pick::ScopePicker>,
    pub error: Option<String>,
    pub scroll: u16,
    pub stale: bool,
    pub summary: String,
    pub overview: Vec<(&'static str, String)>,
    /// The complete policy, shown only when the owner opens the details.
    pub current: Vec<String>,
    pub details: bool,
}

fn list(values: &std::collections::BTreeSet<String>) -> String {
    values.iter().cloned().collect::<Vec<_>>().join(", ")
}

fn on_off(on: bool) -> String {
    if on { "on" } else { "off" }.into()
}

fn days(value: Option<u32>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

impl Controls {
    pub fn new(ctx: &Ctx) -> Self {
        let policy = &ctx.store.config.automation;
        let scope = &policy.scope;
        let mut controls = Self {
            selection: OperatingSelection {
                projects: scope.projects.keys().cloned().collect(),
                tasks: scope.tasks.keys().cloned().collect(),
                exclusions: scope.exclusions.clone(),
                plan_scopes: scope
                    .plans
                    .iter()
                    .map(|plan| (plan.task.clone(), plan.scopes.keys().cloned().collect()))
                    .collect(),
                authority: policy.authority.clone(),
                include_new: !scope.projects.is_empty(),
                expires_after_days: scope.expires_after_days,
                max_new_tasks: scope.max_new_tasks,
            },
            ..Default::default()
        };
        controls.refresh(ctx);
        controls
    }

    pub fn refresh(&mut self, ctx: &Ctx) {
        match super::operating_state::facts(ctx) {
            Ok(facts) => {
                self.summary = facts.summary;
                self.overview = facts.overview;
                self.current = facts.details;
            }
            Err(error) => {
                self.summary = format!("Policy unavailable; no automatic authority: {error:#}");
                self.overview.clear();
                self.current = vec![self.summary.clone()];
            }
        }
        if let Some(preview) = &self.preview {
            self.stale = !corral_ops::automation_preview(&ctx.store, &preview.selection)
                .is_ok_and(|fresh| fresh.id == preview.id);
        }
    }

    pub fn rows(&self) -> Vec<Row> {
        let selection = &self.selection;
        let row = |key, label, kind, value| Row::new(Field::Operating(key), label, kind, value);
        let mut rows = vec![row(
            Key::Status,
            "Operating mode",
            Kind::Info,
            self.summary.clone(),
        )];
        rows.extend(
            self.overview
                .iter()
                .enumerate()
                .map(|(i, (label, value))| row(Key::Overview(i), label, Kind::Info, value.clone())),
        );
        rows.extend([
            row(Key::Projects, "Grant projects", Kind::Entry, list(&selection.projects))
                .help("Pick projects. With Include new tasks on, work filed later is covered too."),
            row(Key::Tasks, "Grant tasks", Kind::Entry, list(&selection.tasks))
                .help("Pick individual tasks; their agreed scope is fixed at grant time."),
            row(Key::IncludeNew, "Include new tasks", Kind::Toggle, on_off(selection.include_new))
                .help("Project grants cover every unfinished task, including ones filed later, until the grant ends."),
            row(Key::Expiry, "Ends after (days)", Kind::Text { optional: true }, days(selection.expires_after_days))
                .help("Only for grants that include new tasks. Unset means 7 days."),
            row(Key::MaxNew, "Max new tasks", Kind::Text { optional: true }, days(selection.max_new_tasks))
                .help("Optional cap on tasks filed after you grant. Unset means no cap."),
            row(Key::Exclusions, "Exclude tasks", Kind::Text { optional: true }, list(&selection.exclusions))
                .help("Comma-separated task IDs this grant never covers."),
            row(Key::PlanScopes, "Reviewed scopes", Kind::Text { optional: true },
                selection.plan_scopes.iter().flat_map(|(task, labels)| labels.iter().map(move |label| format!("{task}:{label}"))).collect::<Vec<_>>().join(", "))
                .help("Comma-separated TASK:LABEL entries from independently reviewed plans."),
            row(Key::Dispatch, "Start & review", Kind::Toggle, on_off(selection.authority.dispatch))
                .help("Start agents, run gates and reviews, and send back fixes. Existing checks still apply."),
            row(Key::Accept, "Land", Kind::Toggle, on_off(selection.authority.accept))
                .help("Merge reviewed work. Owner-only projects still wait for you."),
            row(Key::PlanAccept, "Accept plans", Kind::Toggle, on_off(selection.authority.plan_accept)),
            row(Key::Preview, "Grant Autopilot", Kind::Entry, "review, then press y".into())
                .help("Owner only. Saving settings never grants authority."),
            row(Key::Assist, "Use Assist", Kind::Entry, "withdraw automatic authority".into()),
            row(Key::Pause, "Pause Autopilot", Kind::Entry, "no new actions; keep running work".into()),
            row(Key::Resume, "Resume Autopilot", Kind::Entry, "existing matching grant only".into()),
            row(Key::Details, "Policy details", Kind::Entry,
                if self.details { "hide".into() } else { format!("show {} lines", self.current.len()) }),
        ]);
        if self.details {
            rows.extend(
                self.current
                    .iter()
                    .enumerate()
                    .map(|(i, value)| row(Key::Fact(i), "Detail", Kind::Info, value.clone())),
            );
        }
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
        let number = |what: &str| -> Result<Option<u32>, String> {
            let text = text.trim();
            if text.is_empty() {
                return Ok(None);
            }
            text.parse()
                .map(Some)
                .map_err(|_| format!("{what} must be a whole number"))
        };
        match key {
            Key::Projects => self.selection.projects = values(),
            Key::Tasks => self.selection.tasks = values(),
            Key::Exclusions => self.selection.exclusions = values(),
            Key::IncludeNew => self.selection.include_new = text == "on",
            Key::Expiry => {
                self.selection.expires_after_days = number("days")?;
                if self
                    .selection
                    .expires_after_days
                    .is_some_and(|d| !(1..=365).contains(&d))
                {
                    return Err("use 1 to 365 days".into());
                }
            }
            Key::MaxNew => self.selection.max_new_tasks = number("max new tasks")?,
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
            | Key::Overview(_)
            | Key::Details
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

fn yes(on: bool) -> &'static str {
    if on { "yes" } else { "no" }
}

fn preview_lines(preview: &OperatingPreview, stale: bool) -> Vec<Line<'static>> {
    let policy = &preview.policy;
    let scope = &policy.scope;
    let mut lines = Vec::new();
    if stale {
        lines.push(Line::styled(
            "Something changed since this preview. Press esc and preview again.",
            theme::bold().fg(theme::RED),
        ));
    }
    lines.push(Line::raw(format!(
        "Allowed: start work {} · check & review {} · land {} · accept plans {}",
        yes(policy.authority.dispatch),
        yes(policy.authority.dispatch),
        yes(policy.authority.accept),
        yes(policy.authority.plan_accept),
    )));
    for project in scope.projects.keys() {
        let mut text = format!(
            "Project {project}: every unfinished task, including new ones; ends {} days after you grant",
            scope.expires_after_days.unwrap_or(DEFAULT_GRANT_DAYS)
        );
        if let Some(max) = scope.max_new_tasks {
            text.push_str(&format!("; at most {max} new tasks"));
        }
        lines.push(Line::raw(text));
    }
    if !scope.tasks.is_empty() {
        lines.push(Line::raw(format!(
            "Tasks: {}",
            scope.tasks.keys().cloned().collect::<Vec<_>>().join(", ")
        )));
    }
    if !scope.exclusions.is_empty() {
        lines.push(Line::raw(format!("Never: {}", list(&scope.exclusions))));
    }
    let owner_only = preview
        .constraints
        .iter()
        .filter(|(_, c)| c.owner_acceptance)
        .map(|(id, _)| id.as_str())
        .collect::<Vec<_>>();
    if policy.authority.accept && !owner_only.is_empty() {
        lines.push(Line::raw(format!(
            "Still lands only with your approval: {}",
            owner_only.join(", ")
        )));
    }
    for plan in &scope.plans {
        lines.push(Line::raw(format!(
            "Reviewed plan {}: {}",
            plan.task,
            plan.scopes.keys().cloned().collect::<Vec<_>>().join(", ")
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled("Covered right now:", theme::bold()));
    for (id, constraints) in &preview.constraints {
        let mut text = format!(
            "  {id}  {}  gate {}  reviewer {:?}",
            constraints.lifecycle.as_str(),
            if constraints.review.gate.is_empty() {
                "none"
            } else {
                "set"
            },
            constraints.review.reviewer,
        );
        if constraints.dispatch_hold {
            text.push_str("  held");
        }
        if let Some(decision) = constraints.pending_decisions.first() {
            text.push_str(&format!(
                "  waiting on you: {}",
                crate::app::operating::readable_question(&decision.question)
            ));
        }
        lines.push(Line::raw(text));
    }
    lines.push(Line::styled(
        "Existing checks, reviews and owner-only rules still apply.",
        theme::dim(),
    ));
    lines
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
        match key {
            Key::Open => return Action::OpenOperating,
            Key::Preview => return self.operating_preview(ctx),
            Key::Details => {
                self.operating.details = !self.operating.details;
                return Action::None;
            }
            Key::Projects | Key::Tasks => {
                return match pick::ScopePicker::new(
                    ctx,
                    &self.operating.selection,
                    key == Key::Tasks,
                ) {
                    Ok(picker) => {
                        self.operating.pick = Some(picker);
                        Action::None
                    }
                    Err(error) => Action::Toast(Toast::new(
                        Tone::Bad,
                        "Tasks unavailable",
                        format!("{error:#}"),
                    )),
                };
            }
            _ => {}
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

    /// Only `y` (or the Grant button) confirms, so a repeated Enter cannot grant authority.
    pub(super) fn operating_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if let Some(picker) = &mut self.operating.pick {
            match picker.on_key(key) {
                pick::Outcome::Stay => {}
                pick::Outcome::Cancel => self.operating.pick = None,
                pick::Outcome::Apply { projects, tasks } => {
                    self.operating.selection.projects = projects;
                    self.operating.selection.tasks = tasks;
                    self.operating.preview = None;
                    self.operating.pick = None;
                }
            }
            return Action::None;
        }
        match key.code {
            KeyCode::Esc => self.operating.preview = None,
            KeyCode::PageDown => self.operating.scroll = self.operating.scroll.saturating_add(8),
            KeyCode::PageUp => self.operating.scroll = self.operating.scroll.saturating_sub(8),
            KeyCode::Char('y') => {
                let Some(preview) = &self.operating.preview else {
                    return Action::None;
                };
                match corral_ops::automation_confirm(&ctx.store, preview, &preview.id) {
                    Ok(config) => {
                        self.base = config.clone();
                        self.draft = config;
                        self.operating.preview = None;
                        self.operating.refresh(ctx);
                        self.layers = super::layers::Cache::load(&ctx.store, self.layer_loader);
                    }
                    Err(error) => {
                        self.operating.error = Some(format!("{error:#}"));
                        self.operating.refresh(ctx);
                    }
                }
            }
            _ => {}
        }
        Action::None
    }

    pub(super) fn draw_operating(&mut self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        if let Some(picker) = &mut self.operating.pick {
            picker.draw(f, area, hits);
            return;
        }
        let Some(preview) = &self.operating.preview else {
            return;
        };
        overlays::dim_backdrop(f.buffer_mut(), area);
        let rect = overlays::centered(area, 108, area.height.saturating_sub(2));
        let inner = overlays::popup(f, rect, "grant Autopilot?", hits);
        let body_height = inner.height.saturating_sub(4);
        // Paragraph wraps long lines; PageUp/PageDown expose every line.
        let paragraph =
            Paragraph::new(preview_lines(preview, self.operating.stale)).wrap(Wrap { trim: false });
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
        f.render_widget(
            Paragraph::new(self.operating.error.as_deref().unwrap_or(""))
                .style(ratatui::style::Style::new().fg(theme::RED))
                .wrap(Wrap { trim: false }),
            Rect::new(inner.x, y, inner.width, 2),
        );
        for (offset, label, code) in [
            (0, "[Grant] y", KeyCode::Char('y')),
            (12, "[Cancel] esc", KeyCode::Esc),
        ] {
            let rect = Rect::new(
                inner.x + offset,
                y + 2,
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
                "Lines {}-{}/{} | PgUp/PgDn or wheel",
                usize::from(self.operating.scroll) + 1,
                (usize::from(self.operating.scroll) + usize::from(body_height)).min(line_count),
                line_count,
            ))
            .style(theme::dim()),
            Rect::new(inner.x, y + 3, inner.width, 1),
        );
    }
}
