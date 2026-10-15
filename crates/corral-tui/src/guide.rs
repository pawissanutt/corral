//! Guidance for people who do not know the terminal engine: the first-run welcome tour (remembered in
//! `.corral/state/tour-seen`, replayed with `t` or from `?`) and the `q` popup that explains leaving.

use anyhow::{Context, Result};
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use crate::hit::{self, HitMap};
use crate::nav::NavKeys;
use crate::overlays::{centered, popup};
use crate::theme;
use crate::{Action, Toast, Tone, View};

const SEEN_FILE: &str = "tour-seen";

/// True once the tour was finished or skipped in this workspace.
pub(crate) fn tour_seen(ctx: &Ctx) -> bool {
    ctx.store.state_dir().join(SEEN_FILE).exists()
}

fn mark_seen(ctx: &Ctx) -> Result<()> {
    let dir = ctx.store.state_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(SEEN_FILE);
    std::fs::write(&path, format!("{}\n", jiff::Timestamp::now()))
        .with_context(|| format!("writing {}", path.display()))
}

/// Six short cards: the columns, opening an agent, getting back, Needs me, approving, leaving.
pub(crate) struct TourView {
    pub(crate) step: usize,
    nav: NavKeys,
}

impl TourView {
    pub(crate) const STEPS: usize = 6;

    pub(crate) fn new(nav: NavKeys) -> TourView {
        TourView { step: 0, nav }
    }

    fn card(&self) -> (&'static str, String) {
        match self.step {
            0 => (
                "This is your board",
                "Every task is a card. Cards move left to right as work happens: Backlog → Ready → Running → \
                 Needs me → Review → Done. Press n to add one."
                    .into(),
            ),
            1 => (
                "Open an agent",
                "Each running task has an agent working in its own pane. Select the card and press a, or \
                 right-click it and pick \"jump to agent\"."
                    .into(),
            ),
            2 => (
                "Get back here",
                format!(
                    "From any agent pane, {} to come back to this board. With the mouse, click \
                     \"corral board\" in the sidebar on the left.",
                    self.nav.board_words()
                ),
            ),
            3 => (
                "Needs me",
                "When an agent asks a question, its card moves to Needs me. Press r to answer right here, \
                 or a to answer in its pane."
                    .into(),
            ),
            4 => (
                "Review and approve",
                "Finished work lands in Review. Press ⏎ to read the summary, diff and evidence, then \
                 approve to merge it."
                    .into(),
            ),
            _ => (
                "Leaving and coming back",
                format!(
                    "q closes the board; agents keep running in the background. To leave the whole session, \
                     {}. Run `corral up` to come back. Press ? for help or t to see this tour again.",
                    self.nav.detach.words()
                ),
            ),
        }
    }

    fn last(&self) -> bool {
        self.step + 1 >= Self::STEPS
    }

    fn finish(ctx: &Ctx) -> Action {
        match mark_seen(ctx) {
            Ok(()) => Action::Back,
            Err(e) => Action::BackWithToast(Toast::new(
                Tone::Warn,
                "Tour not remembered",
                format!("{e:#}"),
            )),
        }
    }
}

impl View for TourView {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        let rect = centered(area, area.width.saturating_sub(4).clamp(30, 64), 11);
        let title = format!("welcome to corral · {}/{}", self.step + 1, Self::STEPS);
        let inner = popup(f, rect, &title, hits);
        if inner.height < 3 {
            return;
        }
        let (heading, body) = self.card();
        let text = vec![
            Line::styled(
                heading,
                Style::new().fg(theme::CYAN).add_modifier(Modifier::BOLD),
            ),
            Line::from(""),
            Line::raw(body),
        ];
        let text_area = Rect {
            height: inner.height - 1,
            ..inner
        };
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), text_area);
        let mut buttons = Vec::new();
        if self.step > 0 {
            buttons.push(("‹ Back", hit::press(KeyCode::Left)));
        }
        if self.last() {
            buttons.push(("Done", hit::press(KeyCode::Enter)));
        } else {
            buttons.push(("Next ›", hit::press(KeyCode::Right)));
            buttons.push(("Skip tour", hit::press(KeyCode::Esc)));
        }
        let y = inner.bottom() - 1;
        hit::buttons(f, hits, inner.x, y, inner.right(), &buttons);
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ' | 'l' | 'n') => {
                if self.last() {
                    return Self::finish(ctx);
                }
                self.step += 1;
                Action::None
            }
            KeyCode::Left | KeyCode::Char('h' | 'p') => {
                self.step = self.step.saturating_sub(1);
                Action::None
            }
            KeyCode::Esc | KeyCode::Char('s' | 'q') => Self::finish(ctx),
            _ => Action::None,
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.last() {
            vec![("⏎", "done"), ("←", "back")]
        } else {
            vec![("→", "next"), ("←", "back"), ("esc", "skip tour")]
        }
    }

    fn is_overlay(&self) -> bool {
        true
    }
}

/// What `q` on the board shows: running agents are not stopped, and how to come back.
pub(crate) struct LeaveView {
    running: usize,
    nav: NavKeys,
}

impl LeaveView {
    pub(crate) fn new(running: usize, nav: NavKeys) -> LeaveView {
        LeaveView { running, nav }
    }

    fn lines(&self) -> Vec<String> {
        let running = match self.running {
            0 => "Nothing is running right now; your tasks are saved.".to_string(),
            1 => "1 agent keeps running in the background.".to_string(),
            n => format!("{n} agents keep running in the background."),
        };
        let back = match &self.nav.board {
            Some(_) => format!(
                "Come back with `corral up`, or {} inside the session.",
                self.nav.board_words()
            ),
            None => "Come back with `corral up`.".to_string(),
        };
        vec![
            running,
            back,
            format!("To leave the whole session, {}.", self.nav.detach.words()),
        ]
    }
}

impl View for LeaveView {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        let rect = centered(area, area.width.saturating_sub(4).clamp(30, 66), 10);
        let inner = popup(f, rect, "close the board?", hits);
        if inner.height < 2 {
            return;
        }
        let mut text = Vec::new();
        for line in self.lines() {
            text.push(Line::raw(line));
            text.push(Line::from(""));
        }
        let text_area = Rect {
            height: inner.height - 1,
            ..inner
        };
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), text_area);
        let buttons = [
            ("Close board", hit::press(KeyCode::Char('q'))),
            ("Stay", hit::press(KeyCode::Esc)),
        ];
        hit::buttons(
            f,
            hits,
            inner.x,
            inner.bottom() - 1,
            inner.right(),
            &buttons,
        );
    }

    fn on_key(&mut self, key: KeyEvent, _ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Char('q' | 'y') | KeyCode::Enter => Action::Quit,
            KeyCode::Esc | KeyCode::Char('n') => Action::Back,
            _ => Action::None,
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![("q", "close board"), ("esc", "stay")]
    }

    fn is_overlay(&self) -> bool {
        true
    }
}
