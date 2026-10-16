//! Detail tab "Plan": the plan stage, child task links, verdict history per plan revision and the rendered
//! stored snapshot. `a` / `c` (or the buttons) open the owner decision sheet while a decision is due.

use corral_core::{PlanAcceptance, PlanDocument, Status, Task, Verdict};
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::decision::{Kind, Outcome, Sheet, Z_FIELD};
use super::{PlanStage, plan_first, stage};
use crate::artifacts::ui;
use crate::hit::{self, Hit};
use crate::render::{self, Format};
use crate::theme;
use crate::{Action, HitMap, MouseCtx, Toast, Tone};

/// `Hit::Item` zone of the child task rows.
pub(crate) const Z_CHILD: u8 = 5;

#[derive(Default)]
pub(crate) struct PlanTab {
    /// The task is plan-first, so detail shows the tab.
    pub(crate) enabled: bool,
    stage: Option<PlanStage>,
    sel: usize,
    first_child: usize,
    /// Child ids with their title and status, when the child task still exists.
    children: Vec<(String, Option<(String, Status)>)>,
    /// The latest plan's stored snapshot; loaded on first draw, reset on refresh.
    snapshot: Option<Result<String, String>>,
    sheet: Option<Sheet>,
}

fn markdown_list(out: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    out.push_str(&format!("\n## {title}\n\n"));
    for item in items {
        out.push_str(&format!("- {item}\n"));
    }
}

fn acceptance_text(a: &PlanAcceptance) -> String {
    let id = a
        .criterion
        .as_deref()
        .map(|c| format!("[{c}] "))
        .unwrap_or_default();
    format!("{id}{} → {}", a.scenario, a.expected)
}

/// The structured frontmatter as Markdown, then the plan's own Markdown body.
fn snapshot_markdown(d: &PlanDocument, text: &str) -> String {
    let mut out = format!("## Goal\n\n{}\n", d.goal);
    markdown_list(&mut out, "Non-goals", &d.non_goals);
    markdown_list(&mut out, "Decisions", &d.decisions);
    markdown_list(&mut out, "Owner decisions", &d.owner_decisions);
    let paths: Vec<String> = d.affected_paths.iter().map(|p| format!("`{p}`")).collect();
    markdown_list(&mut out, "Affected paths", &paths);
    markdown_list(&mut out, "Contracts", &d.contracts);
    let tasks: Vec<String> = d
        .dependencies
        .iter()
        .map(|s| {
            let mut line = format!("**{}**", s.task);
            if !s.write_scope.is_empty() {
                line.push_str(&format!(" writes {}", s.write_scope.join(", ")));
            }
            if !s.depends_on.is_empty() {
                line.push_str(&format!(" · after {}", s.depends_on.join(", ")));
            }
            for a in &s.acceptance {
                line.push_str(&format!("\n  - accept: {}", acceptance_text(a)));
            }
            line
        })
        .collect();
    markdown_list(&mut out, "Tasks", &tasks);
    let acceptance: Vec<String> = d.acceptance.iter().map(acceptance_text).collect();
    markdown_list(&mut out, "Acceptance", &acceptance);
    markdown_list(&mut out, "Verification", &d.verification);
    let body = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n").map(|(_, body)| body))
        .unwrap_or("");
    if !body.trim().is_empty() {
        out.push('\n');
        out.push_str(body);
    }
    out
}

fn indented(lines: Vec<Line<'static>>, by: usize) -> impl Iterator<Item = Line<'static>> {
    lines.into_iter().map(move |mut l| {
        l.spans.insert(0, Span::raw(" ".repeat(by)));
        l
    })
}

fn verdict_span(verdict: Option<Verdict>) -> Span<'static> {
    let (text, color) = match verdict {
        Some(Verdict::Approve) => ("approve", theme::GREEN),
        Some(Verdict::Changes) => ("changes", theme::YELLOW),
        Some(Verdict::Escalate) => ("escalate", theme::RED),
        None => ("pending", theme::BLUE),
    };
    Span::styled(text, Style::new().fg(color))
}

impl PlanTab {
    pub(crate) fn new(ctx: &Ctx, task: &Task) -> PlanTab {
        let mut tab = PlanTab::default();
        tab.refresh(ctx, task);
        tab
    }

    pub(crate) fn refresh(&mut self, ctx: &Ctx, task: &Task) {
        let config = Some(&ctx.store.config);
        self.enabled = plan_first(task, config);
        self.stage = stage(task, config);
        self.children = task
            .children
            .iter()
            .map(|id| {
                let child = ctx.store.get(id).ok().map(|c| (c.title, c.status));
                (id.clone(), child)
            })
            .collect();
        self.sel = self.sel.min(self.children.len().saturating_sub(1));
        self.snapshot = None;
    }

    fn decidable(&self) -> bool {
        self.stage.is_some_and(PlanStage::decidable)
    }

    pub(crate) fn sheet_open(&self) -> bool {
        self.sheet.is_some()
    }

    pub(crate) fn has_unsaved_text(&self) -> bool {
        self.sheet.as_ref().is_some_and(Sheet::has_unsaved_text)
    }

    /// Buttons above the tab content while a decision is due or children exist; returns the content area.
    pub(crate) fn toolbar(&self, f: &mut Frame, area: Rect, hits: &mut HitMap) -> Rect {
        let mut buttons = Vec::new();
        if self.decidable() {
            buttons.push(("Approve plan", hit::press(KeyCode::Char('a'))));
            buttons.push(("Request changes", hit::press(KeyCode::Char('c'))));
        }
        if !self.children.is_empty() {
            buttons.push(("Open child", hit::press(KeyCode::Enter)));
        }
        if buttons.is_empty() {
            return area;
        }
        let [bar, _, rest] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(area);
        hit::buttons(f, hits, bar.x, bar.y, bar.right(), &buttons);
        rest
    }

    pub(crate) fn lines(&mut self, ctx: &Ctx, task: &Task, width: u16) -> Vec<Line<'static>> {
        let Some(stage) = self.stage else {
            return vec![Line::styled("This task has no plan.", theme::dim())];
        };
        let plan = task.plans.last();
        let mut head = vec![Span::styled(
            stage.label(),
            Style::new().fg(stage.color()).add_modifier(Modifier::BOLD),
        )];
        if let Some(p) = plan {
            head.push(Span::styled(
                format!(
                    " · plan {} (revision {} of {})",
                    p.id,
                    task.plans.len(),
                    task.plans.len()
                ),
                theme::dim(),
            ));
        }
        if let Some(r) = task.plan_runs.last() {
            head.push(Span::styled(
                format!(" · planner {} · run {}", r.profile, r.id),
                theme::dim(),
            ));
        }
        let mut out = vec![Line::from(head)];
        if self.decidable() {
            out.push(Line::styled(
                "a approve · c request changes: recorded as your owner decision",
                Style::new().fg(theme::YELLOW),
            ));
        }
        out.push(Line::default());
        out.push(Line::styled(
            format!("Child tasks ({})", self.children.len()),
            theme::bold(),
        ));
        self.first_child = out.len();
        for (i, (id, child)) in self.children.iter().enumerate() {
            let mut spans = vec![
                Span::styled(format!("▸ {id}  "), Style::new().fg(theme::CYAN)),
                match child {
                    Some((title, status)) => Span::styled(
                        format!("{title} · {}", status.label()),
                        Style::new().fg(theme::FG),
                    ),
                    None => Span::styled("(task missing)", Style::new().fg(theme::RED)),
                },
            ];
            if i == self.sel {
                spans
                    .iter_mut()
                    .for_each(|s| s.style = s.style.bg(theme::SELECTED_BG));
            }
            out.push(Line::from(spans));
        }
        if self.children.is_empty() {
            let n = plan.map_or(0, |p| p.document.dependencies.len());
            out.push(Line::styled(
                match n {
                    0 => "Single-task plan: approval attaches it to this task.".to_string(),
                    n => format!("{n} planned tasks are created in Backlog on owner approval."),
                },
                theme::dim(),
            ));
        }
        out.push(Line::default());
        out.push(Line::styled("Revisions", theme::bold()));
        if task.plans.is_empty() {
            out.push(Line::styled("No plan submitted yet.", theme::dim()));
        }
        let notes_w = width.saturating_sub(6);
        for (n, p) in task.plans.iter().enumerate().rev() {
            let mut spans = vec![
                Span::styled(p.id.clone(), theme::bold()),
                Span::styled(
                    format!(
                        " · revision {} · submitted {} · spec {}",
                        n + 1,
                        ui::local_date(p.artifact.added, "%b %d %H:%M"),
                        p.spec_revision.chars().take(7).collect::<String>()
                    ),
                    theme::dim(),
                ),
            ];
            spans.push(match (&p.approval, n + 1 == task.plans.len()) {
                (Some(a), _) => Span::styled(
                    format!(" · approved by {} {}", a.by, ui::local_date(a.at, "%b %d")),
                    Style::new().fg(theme::GREEN),
                ),
                (None, true) => Span::styled(
                    format!(" · {}", stage.label()),
                    Style::new().fg(stage.color()),
                ),
                (None, false) => Span::styled(" · superseded", theme::dim()),
            });
            out.push(Line::from(spans));
            if p.reviews.is_empty() {
                out.push(Line::styled("  no review yet", theme::dim()));
            }
            for r in &p.reviews {
                out.push(Line::from(vec![
                    Span::styled(
                        format!("  {} {} · ", r.id, r.reviewer),
                        Style::new().fg(theme::FG),
                    ),
                    verdict_span(r.verdict),
                    Span::styled(
                        format!(" · {}", ui::local_date(r.at, "%b %d %H:%M")),
                        theme::dim(),
                    ),
                ]));
                if let Some(notes) = r.notes.as_deref().filter(|n| !n.trim().is_empty()) {
                    out.extend(indented(
                        render::render(&Format::Markdown, notes, notes_w),
                        4,
                    ));
                }
            }
        }
        if let Some(p) = plan {
            out.push(Line::default());
            out.push(Line::from(vec![
                Span::styled(format!("Plan snapshot {}", p.id), theme::bold()),
                Span::styled(format!(" · .corral/{}", p.artifact.path), theme::dim()),
            ]));
            let snapshot =
                self.snapshot.get_or_insert_with(|| {
                    match corral_ops::artifact_document(ctx, &p.artifact) {
                        Ok(corral_ops::Preview::Text(text)) => Ok(text),
                        Ok(_) => Err("the snapshot is not text".into()),
                        Err(e) => Err(format!("{e:#}")),
                    }
                });
            match snapshot {
                Ok(text) => {
                    let md = snapshot_markdown(&p.document, text);
                    out.extend(render::render(&Format::Markdown, &md, width));
                }
                Err(e) => out.push(Line::styled(
                    format!("snapshot unavailable: {e}"),
                    Style::new().fg(theme::RED),
                )),
            }
        }
        out
    }

    pub(crate) fn hits(&self, hits: &mut HitMap, area: Rect, offset: usize) {
        let n = self.children.len();
        hit::rows(hits, area, Z_CHILD, self.first_child, n, offset);
    }

    pub(crate) fn draw_sheet(&self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        if let Some(sheet) = &self.sheet {
            sheet.draw(f, area, hits);
        }
    }

    fn open_child(&self) -> Action {
        match self.children.get(self.sel) {
            Some((id, _)) => Action::OpenDetail(id.clone()),
            None => Action::None,
        }
    }

    /// The sheet takes every key while open; on the tab, `a` / `c` open it, j/k select a child, ⏎ opens it.
    /// None leaves the key to the detail view.
    pub(crate) fn on_key(
        &mut self,
        key: &KeyEvent,
        ctx: &Ctx,
        task: &Task,
        on_tab: bool,
    ) -> Option<Action> {
        if let Some(sheet) = &mut self.sheet {
            return Some(match sheet.on_key(key, ctx) {
                Outcome::Stay(action) => action,
                Outcome::Close(action) => {
                    self.sheet = None;
                    action
                }
            });
        }
        if !on_tab {
            return None;
        }
        let last = self.children.len().saturating_sub(1);
        match key.code {
            KeyCode::Char(c @ ('a' | 'c')) if self.decidable() => {
                let kind = if c == 'a' {
                    Kind::Approve
                } else {
                    Kind::Changes
                };
                self.sheet = Sheet::new(kind, task);
                Some(match self.sheet {
                    Some(_) => Action::None,
                    None => Action::Toast(Toast::new(
                        Tone::Info,
                        "No plan",
                        "nothing was submitted to decide on",
                    )),
                })
            }
            KeyCode::Char('j') | KeyCode::Down if !self.children.is_empty() => {
                self.sel = (self.sel + 1).min(last);
                Some(Action::None)
            }
            KeyCode::Char('k') | KeyCode::Up if !self.children.is_empty() => {
                self.sel = self.sel.saturating_sub(1);
                Some(Action::None)
            }
            KeyCode::Enter if !self.children.is_empty() => Some(self.open_child()),
            _ => None,
        }
    }

    /// A click on an input row focuses it; a click selects a child row, a second click (or double) opens it.
    pub(crate) fn mouse(&mut self, ev: &MouseEvent, m: &MouseCtx) -> Option<Action> {
        if !hit::left(ev) {
            return self.sheet.as_ref().map(|_| Action::None);
        }
        match (&m.hit, &mut self.sheet) {
            (Some(Hit::Item(Z_FIELD, i)), Some(sheet)) => sheet.focus(*i),
            (_, Some(_)) => {}
            (Some(Hit::Item(Z_CHILD, i)), None) if m.double || *i == self.sel => {
                self.sel = *i;
                return Some(self.open_child());
            }
            (Some(Hit::Item(Z_CHILD, i)), None) => self.sel = *i,
            _ => return None,
        }
        Some(Action::None)
    }

    pub(crate) fn hints(&self, on_tab: bool) -> Option<Vec<(&'static str, &'static str)>> {
        if let Some(sheet) = &self.sheet {
            return Some(sheet.hints());
        }
        let mut h = Vec::new();
        if on_tab && self.decidable() {
            h.extend([("a", "approve plan"), ("c", "request changes")]);
        }
        if on_tab && !self.children.is_empty() {
            h.extend([("j/k", "child"), ("⏎", "open child")]);
        }
        if on_tab {
            h.push(("pgup/pgdn", "scroll"));
        }
        (!h.is_empty()).then_some(h)
    }
}
