//! Read-only views of the agent, gate, reviewer and landing stages.

mod strip;
pub(crate) use strip::{StripState, load as load_strip};
pub(crate) use strip::{action, draw, height};
pub(crate) fn gate_state(state: corral_ops::watch::ProgressState) -> &'static str {
    gate::state(state).0
}
mod content;
mod gate;
mod plan_cells;
#[cfg(test)]
mod tests;

use anyhow::Result;
use corral_core::Task;
use corral_ops::Ctx;
use corral_ops::watch::StageHistory;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::artifacts::ui::{self, Scroll};
use crate::hit;
use crate::render::{self, Format};
use crate::theme;
use crate::{Action, Hit, HitMap, MouseCtx, View};

const Z_LOG: u8 = 0;
const Z_STEP: u8 = 1;
const Z_CHECKLIST: u8 = 2;

pub struct StageView {
    task: Task,
    stage: Stage,
    history: StageHistory,
    error: Option<String>,
    gate_sel: Option<(String, String)>,
    review_sel: Option<String>,
    step: Option<usize>,
    scroll: Scroll,
    follow: bool,
    text: String,
    source: String,
    failed: Option<(String, String)>,
    raw: bool,
    window_start: Option<u64>,
    completed_review: Option<String>,
    strip: Option<StripState>,
    viewport: usize,
}

impl StageView {
    pub fn new(ctx: &Ctx, id: &str, stage: Stage) -> Result<Self> {
        let mut view = Self {
            task: ctx.store.get(id)?,
            stage,
            history: StageHistory {
                gates: vec![],
                reviews: vec![],
            },
            error: None,
            gate_sel: None,
            review_sel: None,
            step: None,
            scroll: Scroll::default(),
            follow: true,
            text: String::new(),
            source: String::new(),
            failed: None,
            raw: false,
            window_start: None,
            completed_review: None,
            strip: None,
            viewport: 10,
        };
        view.refresh(ctx);
        Ok(view)
    }

    fn select_stage(&mut self, stage: Stage, ctx: &Ctx) {
        self.stage = stage;
        self.scroll = Scroll::default();
        self.window_start = None;
        self.follow = true;
        self.load_content(ctx);
    }

    fn previous_next(&mut self, dir: isize, ctx: &Ctx) {
        match self.stage {
            Stage::Gate => {
                let len = self.history.gates.len();
                if len == 0 {
                    return;
                }
                let index = self.gate_index().saturating_add_signed(dir).min(len - 1);
                let g = &self.history.gates[index];
                self.gate_sel = Some((g.run.clone(), g.attempt.clone()));
                self.step = None;
            }
            Stage::Review => {
                let len = self.history.reviews.len();
                if len == 0 {
                    return;
                }
                let index = self.review_index().saturating_add_signed(dir).min(len - 1);
                self.review_sel = Some(self.history.reviews[index].accounting.id.clone());
            }
            Stage::Agent | Stage::Land => return,
        }
        self.scroll = Scroll::default();
        self.window_start = None;
        self.follow = true;
        self.load_content(ctx);
    }

    fn draw_text(&mut self, f: &mut Frame, area: Rect, format: Format, hits: &mut HitMap) {
        self.viewport = usize::from(area.height);
        let lines = if self.raw {
            render::raw(&self.text)
        } else {
            render::render(&format, &self.text, area.width)
        };
        if self.follow {
            self.scroll.offset = lines.len().saturating_sub(usize::from(area.height));
        }
        let y = self.scroll.clamp(lines.len(), usize::from(area.height));
        let x = u16::try_from(self.scroll.x).unwrap_or(u16::MAX);
        f.render_widget(
            Paragraph::new(lines).scroll((u16::try_from(y).unwrap_or(u16::MAX), x)),
            area,
        );
        hits.push(area, Hit::Item(Z_LOG, 0));
    }
}

impl View for StageView {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, hits: &mut HitMap) {
        ui::paint_bg(f, area);
        let [head, stages, controls, body] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(height(&self.task, self.strip.as_ref(), area.width)),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .horizontal_margin(1)
        .areas(area);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("< back", Style::new().fg(theme::CYAN)),
                Span::styled(
                    format!("  {} {} - read-only", self.task.id, self.stage.label()),
                    theme::bold(),
                ),
            ])),
            head,
        );
        hits.push(
            Rect {
                width: 6.min(head.width),
                ..head
            },
            Hit::Key(hit::press(KeyCode::Esc)),
        );
        draw(
            f,
            stages,
            &self.task,
            Some(self.stage),
            self.strip.as_ref(),
            hits,
        );
        hit::buttons(
            f,
            hits,
            controls.x,
            controls.y,
            controls.right(),
            &[
                ("Previous round", hit::press(KeyCode::Char('['))),
                ("Next round", hit::press(KeyCode::Char(']'))),
                ("Latest", hit::press(KeyCode::Char('L'))),
                ("Follow tail", hit::press(KeyCode::End)),
                ("First error", hit::press(KeyCode::Char('e'))),
                ("Raw", hit::press(KeyCode::Char('r'))),
            ],
        );
        if let Some(error) = &self.error {
            f.render_widget(
                Paragraph::new(format!("Stage history unavailable: {error}"))
                    .style(Style::new().fg(theme::RED)),
                body,
            );
            return;
        }
        match self.stage {
            Stage::Gate => self.draw_gate(f, body, hits),
            Stage::Review => self.draw_reviewer(f, body, hits),
            Stage::Agent => {
                let inner = ui::boxed(f, body, "Agent screen", true, Some(self.source.clone()));
                self.draw_text(f, inner, Format::Log, hits);
            }
            Stage::Land => {
                let inner = ui::boxed(f, body, "Landing", true, None);
                self.draw_text(f, inner, Format::Markdown, hits);
            }
        }
        let _ = ctx;
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if !ui::is_press(&key) {
            return Action::None;
        }
        if let Some(stage) = Stage::from_key(&key) {
            self.select_stage(stage, ctx);
            return Action::None;
        }
        match key.code {
            KeyCode::Esc => return Action::Back,
            KeyCode::F(5) => return Action::OpenPlan(self.task.id.clone()),
            KeyCode::Tab | KeyCode::BackTab => {
                let index = usize::from(self.stage.number() - 1);
                let dir = if key.code == KeyCode::Tab { 1 } else { 3 };
                self.select_stage(Stage::ALL[(index + dir) % 4], ctx);
            }
            KeyCode::Char('[') => self.previous_next(-1, ctx),
            KeyCode::Char(']') => self.previous_next(1, ctx),
            KeyCode::Char('L') => {
                self.gate_sel = None;
                self.review_sel = None;
                self.step = None;
                self.scroll = Scroll::default();
                self.window_start = None;
                self.follow = true;
                self.load_content(ctx);
            }
            KeyCode::Char('j') | KeyCode::Down if self.stage == Stage::Gate => {
                self.move_step(1, ctx)
            }
            KeyCode::Char('k') | KeyCode::Up if self.stage == Stage::Gate => {
                self.move_step(-1, ctx)
            }
            KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home if self.stage == Stage::Gate => {
                self.page_gate(key.code, ctx);
            }
            KeyCode::End => {
                self.follow = true;
                self.window_start = None;
                self.scroll.x = 0;
                self.load_content(ctx);
            }
            KeyCode::Char('e') => {
                if self.stage == Stage::Gate
                    && self
                        .selected_gate()
                        .is_some_and(|g| g.state == corral_ops::watch::ProgressState::Failed)
                {
                    self.failed = None;
                    self.window_start = None;
                    self.load_content(ctx);
                } else if let Some(index) = render::first_error_line(&self.text) {
                    self.follow = false;
                    self.scroll.offset = index;
                    self.scroll.x = 0;
                }
            }
            KeyCode::Char('r') => self.raw = !self.raw,
            KeyCode::Left | KeyCode::Char('h') => self.scroll.scroll_x(-1),
            KeyCode::Right | KeyCode::Char('l') => self.scroll.scroll_x(1),
            _ => {
                if self.scroll.on_key(&key, 10) {
                    self.follow = false;
                }
            }
        }
        Action::None
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if let Some(dir) = hit::wheel(&ev) {
            if matches!(m.hit, Some(Hit::Item(Z_CHECKLIST | Z_STEP, _))) {
                self.move_step(dir, m.ctx);
            } else if matches!(m.hit, Some(Hit::Item(Z_LOG, _))) {
                self.follow = false;
                self.scroll.wheel(dir);
            }
        } else if hit::left(&ev)
            && let Some(Hit::Item(Z_STEP, index)) = m.hit
        {
            self.step = Some(index);
            self.scroll = Scroll::default();
            self.window_start = None;
            self.follow = false;
            self.load_content(m.ctx);
        }
        Action::None
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("F1-F4", "stages"),
            ("[/]", "round"),
            ("L", "latest"),
            (
                "j/k",
                if self.stage == Stage::Gate {
                    "command"
                } else {
                    "scroll"
                },
            ),
            ("pgup/pgdn", "log"),
            ("home", "log start"),
            ("end", "follow"),
            ("e", "error"),
            ("h/l", "sideways"),
            ("r", "raw"),
            ("esc", "back"),
        ]
    }

    fn refresh(&mut self, ctx: &Ctx) {
        match ctx.store.get(&self.task.id).and_then(|task| {
            let history = corral_ops::watch::load(ctx, &task.id)?;
            Ok((task, history))
        }) {
            Ok((task, history)) => {
                self.task = task;
                self.history = history;
                self.strip = load_strip(ctx, &self.task);
                self.error = None;
                self.load_content(ctx);
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Agent,
    Gate,
    Review,
    Land,
}

impl Stage {
    const ALL: [Self; 4] = [Self::Agent, Self::Gate, Self::Review, Self::Land];

    const fn number(self) -> u8 {
        match self {
            Self::Agent => 1,
            Self::Gate => 2,
            Self::Review => 3,
            Self::Land => 4,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Agent => "Agent",
            Self::Gate => "Gate",
            Self::Review => "Review",
            Self::Land => "Land",
        }
    }

    fn from_key(key: &ratatui::crossterm::event::KeyEvent) -> Option<Self> {
        match key.code {
            ratatui::crossterm::event::KeyCode::F(1) => Some(Self::Agent),
            ratatui::crossterm::event::KeyCode::F(2) => Some(Self::Gate),
            ratatui::crossterm::event::KeyCode::F(3) => Some(Self::Review),
            ratatui::crossterm::event::KeyCode::F(4) => Some(Self::Land),
            _ => None,
        }
    }
}
