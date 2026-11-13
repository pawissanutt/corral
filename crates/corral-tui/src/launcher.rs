//! The `corral pi` launcher: one screen that asks what to open. Its rows come from the caller (start
//! a build session, an assistant or the PM, resume a stopped session) and sit under their section's
//! heading in the order given. `↑↓` or `j`/`k` select, `⏎` opens, a row's key opens that row
//! directly, `q`, Esc or Ctrl+C leave without opening anything. A row with options (where a build
//! session writes) shows the chosen one on a line under it while it is selected; `←→` cycle it, and
//! a click on that line moves to the next option (on `‹`, the previous one). A click selects a row
//! and a click on the selected row opens it; the wheel moves the selection and every footer hint
//! presses its key. Below [`FOLD_BELOW`] columns each row folds to two lines, the name and then the
//! detail, and its option line folds to the label and then the option.
//!
//! [`State`] and [`draw`] are pure so tests drive them on a `TestBackend`; [`pick`] owns the
//! terminal.

use anyhow::{Context, Result};
use ratatui::Frame;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent,
};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::hit::{self, Hit, HitMap};
use crate::overlays::{hint_spans, truncate};
use crate::theme::{self, CYAN, FG, SELECTED_BG};

/// The heading a launcher row sits under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Something new: a build session, an assistant, the PM.
    Start,
    /// A stopped session that can continue.
    Resume,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Start => "Start",
            Section::Resume => "Resume",
        }
    }
}

/// One launcher row. [`pick`] returns `id` with the index of the chosen option; `key` opens the row
/// directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherEntry {
    pub id: String,
    pub section: Section,
    pub key: Option<char>,
    pub name: String,
    pub detail: String,
    /// Where the row writes, cycled with `←→`; the first is chosen until the owner changes it. A
    /// double space separates an option's name from its explanation.
    pub options: Vec<String>,
}

/// Terminals narrower than this fold each row to two lines.
pub const FOLD_BELOW: u16 = 76;
/// The launcher column never grows wider than this; wider terminals center it.
const MAX_WIDTH: u16 = 104;
/// The name column of an unfolded row.
const NAME_COL: usize = 11;
/// Bodies at least this tall keep a blank line above the heading.
const MARGIN_FROM: u16 = 12;
const HEADING: &str = "What do you want to open?";
const MARKER: &str = "▌";
/// What a row's options choose; it heads the option line and names the footer's `←→`.
const OPTION_LABEL: &str = "where it writes";
/// Mouse zone of the rows ([`Hit::Item`]).
pub(crate) const Z_ROW: u8 = 1;

/// What one input asks of the launcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Stay,
    /// Open the row at this index: [`pick`] returns its id and chosen option.
    Open(usize),
    /// Leave without opening anything: [`pick`] returns `None`.
    Leave,
}

/// The selected row, each row's chosen option and the first visible body line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct State {
    pub(crate) selected: usize,
    /// Chosen option per row index; rows past its end have chosen their first option.
    chosen: Vec<usize>,
    offset: usize,
}

impl State {
    pub(crate) fn key(&mut self, entries: &[LauncherEntry], key: KeyEvent) -> Step {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return if key.code == KeyCode::Char('c') {
                Step::Leave
            } else {
                Step::Stay
            };
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            return Step::Stay;
        }
        let last = entries.len().saturating_sub(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            KeyCode::Left => self.cycle(entries, false),
            KeyCode::Right => self.cycle(entries, true),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            KeyCode::Enter if !entries.is_empty() => return Step::Open(self.selected),
            KeyCode::Esc | KeyCode::Char('q') => return Step::Leave,
            KeyCode::Char(c) => {
                if let Some(i) = entries.iter().position(|e| e.key == Some(c)) {
                    self.selected = i;
                    return Step::Open(i);
                }
            }
            _ => {}
        }
        Step::Stay
    }

    /// Moves the selected row's option one step, wrapping; rows without options ignore it.
    fn cycle(&mut self, entries: &[LauncherEntry], forward: bool) {
        let count = entries.get(self.selected).map_or(0, |e| e.options.len());
        if count == 0 {
            return;
        }
        if self.chosen.len() <= self.selected {
            self.chosen.resize(self.selected + 1, 0);
        }
        let at = &mut self.chosen[self.selected];
        *at = if forward {
            (*at + 1) % count
        } else {
            (*at + count - 1) % count
        };
    }

    /// The option chosen on row `index` (0 for rows without options).
    pub(crate) fn option(&self, index: usize) -> usize {
        self.chosen.get(index).copied().unwrap_or(0)
    }

    /// What [`pick`] returns when row `index` opens: its id and chosen option.
    pub(crate) fn picked(
        &self,
        entries: &[LauncherEntry],
        index: usize,
    ) -> Option<(String, usize)> {
        entries
            .get(index)
            .map(|entry| (entry.id.clone(), self.option(index)))
    }

    /// A click selects a row; a click on the selected row opens it.
    pub(crate) fn click(&mut self, index: usize) -> Step {
        if index == self.selected {
            Step::Open(index)
        } else {
            self.selected = index;
            Step::Stay
        }
    }

    /// `hits` are the regions of the frame the pointer was over.
    pub(crate) fn mouse(
        &mut self,
        entries: &[LauncherEntry],
        hits: &HitMap,
        ev: &MouseEvent,
    ) -> Step {
        if let Some(dir) = hit::wheel(ev) {
            let code = if dir > 0 { KeyCode::Down } else { KeyCode::Up };
            return self.key(entries, hit::press(code));
        }
        if !hit::left(ev) {
            return Step::Stay;
        }
        match hits.at(ev.column, ev.row) {
            Some(Hit::Item(Z_ROW, i)) if *i < entries.len() => self.click(*i),
            Some(Hit::Key(key)) => self.key(entries, *key),
            _ => Step::Stay,
        }
    }
}

/// What a body line is to the selection and the mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// The heading, a section title or spacing.
    Gap,
    /// A row's name and detail: a click selects or opens the row.
    Row(usize),
    /// A row's option line: a click moves to the next option, or to the previous one on `‹`, which
    /// sits at this column when the line carries it.
    Option(usize, Option<usize>),
}

impl Part {
    fn row(self) -> Option<usize> {
        match self {
            Part::Gap => None,
            Part::Row(i) | Part::Option(i, _) => Some(i),
        }
    }
}

/// Spaces after `text` up to display width `width`.
fn padded(text: String, width: usize) -> String {
    let fill = width.saturating_sub(text.width());
    text + &" ".repeat(fill)
}

/// `‹ option ›` starting `indent` columns in, the option cut to fit `width`: its name in the
/// foreground and its explanation dim.
fn option_line(
    option: &str,
    indent: usize,
    width: usize,
    lead: Vec<Span<'static>>,
) -> Line<'static> {
    let text = truncate(option, width.saturating_sub(indent + 4));
    let (name, rest) = match text.split_once("  ") {
        Some((name, rest)) => (name.to_string(), format!("  {rest}")),
        None => (text, String::new()),
    };
    let chevron = Style::new().fg(CYAN);
    let mut spans = lead;
    spans.extend([
        Span::styled("‹ ", chevron),
        Span::styled(name, Style::new().fg(FG)),
        Span::styled(rest, theme::dim()),
        Span::styled(" ›", chevron),
    ]);
    Line::from(spans)
}

/// The body lines, each with what it is to the selection and the mouse.
fn body_lines(
    entries: &[LauncherEntry],
    state: &State,
    width: usize,
    folded: bool,
    margin: bool,
) -> Vec<(Line<'static>, Part)> {
    let selected = state.selected;
    let mut out = Vec::new();
    if margin {
        out.push((Line::default(), Part::Gap));
    }
    out.push((
        Line::from(Span::styled(
            truncate(HEADING, width),
            Style::new().fg(CYAN).add_modifier(Modifier::BOLD),
        )),
        Part::Gap,
    ));
    let mut section = None;
    for (i, entry) in entries.iter().enumerate() {
        if section != Some(entry.section) {
            section = Some(entry.section);
            out.push((Line::default(), Part::Gap));
            out.push((
                Line::from(Span::styled(entry.section.title(), theme::dim())),
                Part::Gap,
            ));
        }
        let marker = Span::styled(
            if i == selected { MARKER } else { " " },
            Style::new().fg(CYAN),
        );
        let key = entry.key.map(|k| k.to_string());
        // The key sits at the right edge after a one-column gap; rows without one use the room.
        let key_cols = key.as_ref().map_or(0, |k| k.width() + 1);
        let key_spans = key
            .into_iter()
            .flat_map(|k| [Span::raw(" "), Span::styled(k, Style::new().fg(CYAN))]);
        if folded {
            let name_cols = width.saturating_sub(2 + key_cols);
            let name = padded(truncate(&entry.name, name_cols), name_cols);
            let mut first = vec![marker, Span::raw(" "), Span::styled(name, theme::bold())];
            first.extend(key_spans);
            out.push((Line::from(first), Part::Row(i)));
            let detail = truncate(&entry.detail, width.saturating_sub(2));
            out.push((
                Line::from(vec![Span::raw("  "), Span::styled(detail, theme::dim())]),
                Part::Row(i),
            ));
        } else {
            let detail_cols = width.saturating_sub(2 + NAME_COL + 1 + key_cols);
            let name = padded(truncate(&entry.name, NAME_COL), NAME_COL);
            let detail = padded(truncate(&entry.detail, detail_cols), detail_cols);
            let mut spans = vec![
                marker,
                Span::raw(" "),
                Span::styled(name, theme::bold()),
                Span::raw(" "),
                Span::styled(detail, theme::dim()),
            ];
            spans.extend(key_spans);
            out.push((Line::from(spans), Part::Row(i)));
        }
        let Some(option) = entry.options.get(state.option(i)).filter(|_| i == selected) else {
            continue;
        };
        if folded {
            let label = truncate(OPTION_LABEL, width.saturating_sub(2));
            out.push((
                Line::from(vec![Span::raw("  "), Span::styled(label, theme::dim())]),
                Part::Option(i, None),
            ));
            let line = option_line(option, 2, width, vec![Span::raw("  ")]);
            out.push((line, Part::Option(i, Some(2))));
        } else {
            // Under the detail column, so the option reads as part of the row it belongs to.
            let lead = " ".repeat(2 + NAME_COL + 1);
            let indent = lead.width() + OPTION_LABEL.width() + 2;
            let spans = vec![
                Span::raw(lead),
                Span::styled(OPTION_LABEL, theme::dim()),
                Span::raw("  "),
            ];
            let line = option_line(option, indent, width, spans);
            out.push((line, Part::Option(i, Some(indent))));
        }
    }
    out
}

/// The footer's hints: movement, the selected row's options, opening, each row key, leaving.
fn hints(entries: &[LauncherEntry], selected: usize) -> Vec<(String, String)> {
    let mut hints = vec![("↑↓".to_string(), "select".to_string())];
    if entries.get(selected).is_some_and(|e| !e.options.is_empty()) {
        hints.push(("←→".to_string(), OPTION_LABEL.to_string()));
    }
    hints.push(("⏎".to_string(), "open".to_string()));
    hints.extend(
        entries
            .iter()
            .filter_map(|e| e.key.map(|k| (k.to_string(), e.name.clone()))),
    );
    hints.push(("q/esc".to_string(), "leave".to_string()));
    hints
}

/// Draws the launcher over the whole frame and registers its rows and footer hints in `hits`.
pub(crate) fn draw(f: &mut Frame, entries: &[LauncherEntry], state: &mut State, hits: &mut HitMap) {
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(theme::BG).fg(FG)), area);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let pad = if area.width > 8 { 2 } else { 0 };
    let width = (area.width - pad * 2).min(MAX_WIDTH);
    let x = area.x + (area.width - width) / 2;
    let footer = u16::from(area.height > 1);
    let body = Rect::new(x, area.y, width, area.height - footer);
    let lines = body_lines(
        entries,
        state,
        usize::from(width),
        area.width < FOLD_BELOW,
        body.height >= MARGIN_FROM,
    );
    let height = usize::from(body.height);
    let selected = state.selected;
    if let (Some(first), Some(last)) = (
        lines
            .iter()
            .position(|(_, part)| part.row() == Some(selected)),
        lines
            .iter()
            .rposition(|(_, part)| part.row() == Some(selected)),
    ) {
        // Scrolling up also brings back the headings directly above the row.
        let mut top = first;
        while top > 0 && lines[top - 1].1 == Part::Gap {
            top -= 1;
        }
        if first < state.offset {
            state.offset = top;
        }
        if last >= state.offset + height {
            state.offset = last + 1 - height;
        }
    }
    state.offset = state.offset.min(lines.len().saturating_sub(height));
    for (dy, (line, part)) in lines
        .into_iter()
        .skip(state.offset)
        .take(height)
        .enumerate()
    {
        let rect = Rect::new(x, body.y + dy as u16, width, 1);
        let style = if part.row() == Some(selected) {
            Style::new().bg(SELECTED_BG)
        } else {
            Style::new()
        };
        f.render_widget(Paragraph::new(line).style(style), rect);
        match part {
            Part::Gap => {}
            Part::Row(i) => hits.push(rect, Hit::Item(Z_ROW, i)),
            Part::Option(_, back) => {
                hits.push(rect, Hit::Key(hit::press(KeyCode::Right)));
                if let Some(col) = back.and_then(|col| u16::try_from(col).ok())
                    && col < width
                {
                    hits.push(
                        Rect::new(x + col, rect.y, 1, 1),
                        Hit::Key(hit::press(KeyCode::Left)),
                    );
                }
            }
        }
    }
    if footer == 1 {
        let owned = hints(entries, selected);
        let keys: Vec<(&str, &str)> = owned
            .iter()
            .map(|(k, l)| (k.as_str(), l.as_str()))
            .collect();
        let y = area.bottom() - 1;
        let row = Rect::new(x, y, width, 1);
        f.render_widget(Paragraph::new(Line::from(hint_spans(&keys))), row);
        hit::hint_hits(hits, &keys, x, y, row.right());
    }
}

/// Shows the launcher on the alternate screen and returns the id of the row the owner opened with
/// the index of its chosen option (0 for rows without options), or `None` when they left. The
/// terminal is restored on every return, including errors; a panic is restored by ratatui's hook.
pub fn pick(entries: &[LauncherEntry]) -> Result<Option<(String, usize)>> {
    let mut terminal = ratatui::try_init().context("initializing the terminal")?;
    let picked = run(&mut terminal, entries);
    ratatui::restore();
    picked
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    entries: &[LauncherEntry],
) -> Result<Option<(String, usize)>> {
    let env = std::env::var("CORRAL_MOUSE").ok();
    let mouse = crate::term::mouse_wanted(false, env.as_deref(), true);
    let _mouse = mouse.then(crate::term::MouseGuard::enable).transpose()?;
    let mut state = State::default();
    let mut hits = HitMap::default();
    loop {
        terminal
            .draw(|f| {
                hits.clear();
                draw(f, entries, &mut state, &mut hits);
            })
            .context("drawing the launcher")?;
        let step = match event::read().context("reading terminal event")? {
            Event::Key(key) if key.kind != KeyEventKind::Release => state.key(entries, key),
            Event::Mouse(ev) => state.mouse(entries, &hits, &ev),
            _ => Step::Stay,
        };
        match step {
            Step::Stay => {}
            Step::Open(i) => return Ok(state.picked(entries, i)),
            Step::Leave => return Ok(None),
        }
    }
}
