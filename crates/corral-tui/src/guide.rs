//! Guidance for people who do not know the terminal engine: the first-run welcome tour (remembered in
//! `.corral/state/tour-seen`, replayed with `t` or from `?`) and the `q` popup that explains leaving.

use anyhow::{Context, Result};
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

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

/// Greedy word wrap to `width` columns. The guide popups lay out their own lines so their height always matches
/// the text they show (a word longer than the line keeps a line of its own).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Draws a popup sized to its wrapped `text` (paragraphs with blank lines between them, dropped when the pane is
/// too short for them) with `buttons` on its last row. `size` is the widest it gets and the shortest it gets.
fn guide_popup(
    f: &mut Frame,
    area: Rect,
    title: &str,
    (max_w, min_h): (u16, u16),
    text: &[(String, Style)],
    buttons: &[(&str, KeyEvent)],
    hits: &mut HitMap,
) {
    let w = area.width.saturating_sub(4).clamp(30, max_w);
    let text_w = usize::from(w.saturating_sub(2));
    let paragraphs: Vec<Vec<Line<'static>>> = text
        .iter()
        .map(|(t, style)| {
            let lines = wrap(t, text_w).into_iter();
            lines.map(|l| Line::styled(l, *style)).collect()
        })
        .collect();
    let rows: usize = paragraphs.iter().map(Vec::len).sum();
    // Text, a blank line between paragraphs and before the buttons, the button row and both borders.
    let spaced = rows + paragraphs.len() + 3 <= usize::from(area.height);
    let mut lines = Vec::new();
    for (i, paragraph) in paragraphs.into_iter().enumerate() {
        if i > 0 && spaced {
            lines.push(Line::default());
        }
        lines.extend(paragraph);
    }
    if spaced {
        lines.push(Line::default());
    }
    let h = u16::try_from(lines.len() + 3)
        .unwrap_or(u16::MAX)
        .max(min_h);
    let inner = popup(f, centered(area, w, h), title, hits);
    if inner.height < 2 {
        return;
    }
    let text_area = Rect {
        height: inner.height - 1,
        ..inner
    };
    f.render_widget(Paragraph::new(lines), text_area);
    hit::buttons(f, hits, inner.x, inner.bottom() - 1, inner.right(), buttons);
}

/// Seven short cards: the columns, opening an agent, getting back, Blocked, approving, Autopilot, leaving.
pub(crate) struct TourView {
    pub(crate) step: usize,
    nav: NavKeys,
}

impl TourView {
    pub(crate) const STEPS: usize = 7;
    /// Keeps every card the same size on wide screens, so the buttons do not move between cards.
    const MIN_HEIGHT: u16 = 11;

    pub(crate) fn new(nav: NavKeys) -> TourView {
        TourView { step: 0, nav }
    }

    fn card(&self, ui: &corral_core::UiCfg) -> (&'static str, String) {
        match self.step {
            0 => (
                "This is your board",
                format!(
                    "Every task is a card. Cards move left to right as work happens: Backlog → Ready → Running → \
                     Blocked → Review → Done. Press {} to add one.", ui.bound_key('n')
                ),
            ),
            1 => (
                "Open an agent",
                format!(
                    "Each running task has an agent working in its own pane. Select the card and press {}, or \
                     right-click it and pick \"jump to agent\".", ui.bound_key('a')
                ),
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
                "Blocked",
                format!(
                    "Tasks that cannot continue appear in Blocked with a reason, unblocker and age. Press ⏎ \
                     for full details. For an agent question, press {} to answer here, or {} to open its pane.",
                    ui.bound_key('r'), ui.bound_key('a')
                ),
            ),
            4 => (
                "Review and approve",
                "Finished work lands in Review. Press ⏎ to read the summary, diff and evidence, then \
                 approve to merge it."
                    .into(),
            ),
            5 => (
                "Let corral drive",
                format!(
                    "By default corral waits for you before each check, review and landing. Press {}, pick \
                     tasks or projects, review the summary and press y to let it run those steps itself. \
                     Turn on Include new tasks so a project grant also covers work filed later.",
                    ui.bound_key('O')
                ),
            ),
            _ => (
                "Leaving and coming back",
                format!(
                    "{} closes the board; agents keep running in the background. To leave the whole session, \
                     {}. Run `corral up` to come back. Press {} for help or {} to see this tour again.",
                    ui.bound_key('q'), self.nav.detach.words(), ui.bound_key('?'), ui.bound_key('t')
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
        let title = format!("welcome to corral · {}/{}", self.step + 1, Self::STEPS);
        let (heading, body) = self.card(&_ctx.store.config.ui);
        let heading_style = Style::new().fg(theme::CYAN).add_modifier(Modifier::BOLD);
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
        let text = [(heading.to_string(), heading_style), (body, Style::new())];
        let size = (64, Self::MIN_HEIGHT);
        guide_popup(f, area, &title, size, &text, &buttons, hits);
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
        let text: Vec<(String, Style)> = self
            .lines()
            .into_iter()
            .map(|l| (l, Style::new()))
            .collect();
        let buttons = [
            ("Close board", hit::press(KeyCode::Char('q'))),
            ("Stay", hit::press(KeyCode::Esc)),
        ];
        guide_popup(f, area, "close the board?", (66, 0), &text, &buttons, hits);
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
