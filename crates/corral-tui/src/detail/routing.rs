//! Cached route explanation and ops-backed task overrides.

use corral_core::Task;
use corral_ops::{Ctx, ReviewMapping, RouteExplanation};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent, MouseEvent},
    layout::Rect,
    style::Style,
    text::Line,
    widgets::Paragraph,
};

use crate::artifacts::ui::{self, Scroll};
use crate::choice::{RunChoice, Z_PROFILE};
use crate::hit;
use crate::overlays::{centered, popup};
use crate::routes::RouteCache;
use crate::theme;
use crate::{Action, Hit, HitMap, MouseCtx, Toast, Tone};

const Z_REVIEWER: u8 = 22;

pub(super) enum Picker {
    Implementer(Box<RunChoice>),
    Reviewer { names: Vec<String>, selected: usize },
}

#[derive(Default)]
pub(super) struct Routing {
    pub open: bool,
    pub cache: RouteCache,
    scroll: Scroll,
    pub(super) picker: Option<Picker>,
}

impl Routing {
    pub fn refresh(&mut self, ctx: &Ctx, task: &Task) {
        self.cache.refresh(ctx, std::slice::from_ref(task));
    }

    fn edit(&mut self, ctx: &Ctx, task: &Task, implementer: bool) {
        self.open = true;
        self.picker = Some(if implementer {
            let profile = self
                .cache
                .get(&task.id)
                .and_then(|r| r.next.as_ref().ok())
                .map(|r| r.implementer.profile.as_str())
                .or(task.profile.as_deref())
                .unwrap_or("");
            Picker::Implementer(Box::new(RunChoice::new(
                &ctx.store.config,
                profile,
                None,
                None,
            )))
        } else {
            let mut names = vec![
                "inherit".into(),
                "cross-vendor".into(),
                "pm".into(),
                "none".into(),
            ];
            for name in ctx.store.config.profile_names() {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            let selected = task
                .reviewer
                .as_ref()
                .and_then(|r| names.iter().position(|n| n == r))
                .unwrap_or(0);
            Picker::Reviewer { names, selected }
        });
    }

    fn save(&mut self, ctx: &Ctx, task: &Task) -> Action {
        let result = match &self.picker {
            Some(Picker::Implementer(choice)) => {
                corral_ops::implementer_set(ctx, &task.id, choice.profile())
            }
            Some(Picker::Reviewer { names, selected }) => {
                let reviewer = names
                    .get(*selected)
                    .filter(|n| n.as_str() != "inherit")
                    .map(String::as_str);
                corral_ops::reviewer_set(ctx, &task.id, reviewer)
            }
            None => return Action::None,
        };
        self.changed(ctx, result)
    }

    fn changed(&mut self, ctx: &Ctx, result: anyhow::Result<Task>) -> Action {
        match result {
            Ok(task) => {
                self.picker = None;
                self.refresh(ctx, &task);
                Action::Toast(Toast::new(
                    Tone::Good,
                    "Route updated",
                    "Task override saved; refreshing route",
                ))
            }
            Err(e) => Action::Toast(Toast::new(Tone::Bad, "Route not updated", format!("{e:#}"))),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent, ctx: &Ctx, task: &Task) -> Option<Action> {
        if !self.open {
            match key.code {
                KeyCode::Char('r') => self.open = true,
                KeyCode::Char('p') => self.edit(ctx, task, true),
                KeyCode::Char('v') => self.edit(ctx, task, false),
                _ => return None,
            }
            return Some(Action::None);
        }
        let action = match key.code {
            KeyCode::Esc => {
                if self.picker.is_some() {
                    self.picker = None;
                } else {
                    self.open = false;
                }
                Action::None
            }
            KeyCode::Enter if self.picker.is_some() => self.save(ctx, task),
            _ if self.picker.is_some() => {
                match &mut self.picker {
                    Some(Picker::Implementer(choice)) => {
                        if matches!(
                            key.code,
                            KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l')
                        ) {
                            choice.on_key(&key, ctx);
                        }
                    }
                    Some(Picker::Reviewer { names, selected }) => match key.code {
                        KeyCode::Char('j') | KeyCode::Down => {
                            *selected = (*selected + 1).min(names.len().saturating_sub(1))
                        }
                        KeyCode::Char('k') | KeyCode::Up => *selected = selected.saturating_sub(1),
                        _ => {}
                    },
                    None => {}
                }
                Action::None
            }
            KeyCode::Char('p') => {
                self.edit(ctx, task, true);
                Action::None
            }
            KeyCode::Char('v') => {
                self.edit(ctx, task, false);
                Action::None
            }
            KeyCode::Char('c') => self.changed(ctx, corral_ops::reviewer_set(ctx, &task.id, None)),
            _ => {
                self.scroll.on_key(&key, 10);
                Action::None
            }
        };
        Some(action)
    }

    pub fn mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Option<Action> {
        if !self.open {
            return None;
        }
        if let Some(dir) = hit::wheel(&ev)
            && self.picker.is_none()
        {
            self.scroll.wheel(dir);
        }
        if hit::left(&ev) {
            match (&mut self.picker, &m.hit) {
                (Some(Picker::Implementer(choice)), Some(Hit::Item(Z_PROFILE, _))) => {
                    choice.on_mouse(&ev, m);
                }
                (Some(Picker::Reviewer { names, selected }), Some(Hit::Item(Z_REVIEWER, i)))
                    if *i < names.len() =>
                {
                    *selected = *i
                }
                _ => {}
            }
        }
        Some(Action::None)
    }

    pub fn hints(&self) -> Vec<(&'static str, &'static str)> {
        match &self.picker {
            Some(Picker::Implementer(_)) => {
                vec![("h/l", "profile"), ("⏎", "save"), ("esc", "cancel")]
            }
            Some(Picker::Reviewer { .. }) => {
                vec![("↑↓", "reviewer"), ("⏎", "save"), ("esc", "cancel")]
            }
            None => vec![
                ("p", "pin implementer"),
                ("v", "set reviewer"),
                ("c", "clear reviewer"),
                ("j/k", "scroll"),
                ("esc", "close"),
            ],
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, task: &Task, hits: &mut HitMap) {
        self.cache.poll();
        if !self.open {
            return;
        }
        let rect = centered(area, 100, area.height.saturating_sub(2));
        let inner = popup(f, rect, &format!("Route · {}", task.id), hits);
        if inner.height < 3 {
            return;
        }
        hit::buttons(
            f,
            hits,
            inner.x,
            inner.y,
            inner.right(),
            &[
                ("Pin implementer", hit::press(KeyCode::Char('p'))),
                ("Set reviewer", hit::press(KeyCode::Char('v'))),
                ("Clear reviewer", hit::press(KeyCode::Char('c'))),
            ],
        );
        let lines = match self.cache.get(&task.id) {
            None => vec![Line::styled("Loading route...", theme::dim())],
            Some(route) => {
                let mut lines = explanation("Next dispatch (corral route --json)", &route.next);
                if let Some(recorded) = &route.recorded {
                    lines.extend(explanation("Recorded run (board)", recorded));
                }
                lines
            }
        };
        ui::scrolled(
            f,
            Rect {
                y: inner.y + 2,
                height: inner.height - 2,
                ..inner
            },
            lines,
            &mut self.scroll,
            true,
        );
        if let Some(picker) = &mut self.picker {
            let height = match picker {
                Picker::Implementer(_) => 7,
                Picker::Reviewer { names, .. } => {
                    u16::try_from(names.len()).unwrap_or(12).min(12) + 4
                }
            };
            let inner = popup(
                f,
                centered(area, 84, height),
                match picker {
                    Picker::Implementer(_) => "Pin implementer",
                    Picker::Reviewer { .. } => "Set reviewer",
                },
                hits,
            );
            if inner.height < 3 {
                return;
            }
            match picker {
                Picker::Implementer(choice) => {
                    choice.draw_profiles(f, Rect { height: 1, ..inner }, hits);
                    f.render_widget(
                        Paragraph::new(
                            "Applies to the next dispatch; running identity is retained.",
                        ),
                        Rect {
                            y: inner.y + 2,
                            height: 1,
                            ..inner
                        },
                    );
                }
                Picker::Reviewer { names, selected } => {
                    let rows = usize::from(inner.height.saturating_sub(2));
                    let offset = selected.saturating_sub(rows.saturating_sub(1));
                    for (i, name) in names.iter().enumerate().skip(offset).take(rows) {
                        let row = Rect::new(
                            inner.x,
                            inner.y + u16::try_from(i - offset).unwrap_or(0),
                            inner.width,
                            1,
                        );
                        let label = if name == "inherit" {
                            "inherit (clear task override)"
                        } else {
                            name
                        };
                        let style = if i == *selected {
                            theme::bold().fg(theme::CYAN)
                        } else {
                            theme::dim()
                        };
                        f.render_widget(
                            Paragraph::new(format!(
                                "{} {label}",
                                if i == *selected { ">" } else { " " }
                            ))
                            .style(style),
                            row,
                        );
                        hits.push(row, Hit::Item(Z_REVIEWER, i));
                    }
                }
            }
            hit::buttons(
                f,
                hits,
                inner.x,
                inner.bottom() - 1,
                inner.right(),
                &[
                    ("Save", hit::press(KeyCode::Enter)),
                    ("Cancel", hit::press(KeyCode::Esc)),
                ],
            );
        }
    }
}

fn explanation(title: &str, route: &Result<RouteExplanation, String>) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        title.to_string(),
        theme::bold().fg(theme::CYAN),
    )];
    match route {
        Err(error) => lines.push(Line::styled(error.clone(), Style::new().fg(theme::RED))),
        Ok(route) => {
            let r = &route.implementer;
            lines.push(Line::from(format!(
                "Implementer: {} · {} · model {} · effort {}",
                r.profile,
                r.harness,
                r.model.as_deref().unwrap_or("default"),
                r.effort.as_deref().unwrap_or("default")
            )));
            lines.push(Line::from(format!("Implementer source: {}", r.route)));
            warnings(&mut lines, &r.warnings);
            mapping(&mut lines, "Reviewer", &route.review.reviewer);
            if let Some(p) = &route.review.planner {
                lines.push(Line::from(format!(
                    "Planner: {} · {} · model {} · vendor {}",
                    p.profile,
                    p.harness,
                    p.model.as_deref().unwrap_or("default"),
                    p.vendor.as_deref().unwrap_or("unknown")
                )));
                lines.push(Line::from(format!("Planner source: {}", p.source)));
                warnings(&mut lines, &p.warnings);
            }
            if let Some(reviewer) = &route.review.plan_reviewer {
                mapping(&mut lines, "Plan reviewer", reviewer);
            }
        }
    }
    lines.push(Line::from(""));
    lines
}

fn mapping(lines: &mut Vec<Line<'static>>, role: &str, r: &ReviewMapping) {
    lines.push(Line::from(format!(
        "{role}: {} -> {}",
        r.selection,
        r.profile.as_deref().unwrap_or(&r.selection)
    )));
    lines.push(Line::from(format!(
        "  harness {} · model {} · vendor {}",
        r.harness.as_deref().unwrap_or("-"),
        r.model.as_deref().unwrap_or("default"),
        r.vendor.as_deref().unwrap_or("unknown")
    )));
    lines.push(Line::from(format!(
        "{role} source: {} · policy {}{}",
        r.source,
        r.policy_source,
        r.rule
            .as_deref()
            .map(|rule| format!(" · {rule}"))
            .unwrap_or_default()
    )));
    warnings(lines, &r.warnings);
}

fn warnings(lines: &mut Vec<Line<'static>>, warnings: &[String]) {
    lines.extend(
        warnings
            .iter()
            .map(|w| Line::styled(format!("Warning: {w}"), Style::new().fg(theme::YELLOW))),
    );
}
