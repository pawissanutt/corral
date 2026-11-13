//! The `corral pi` launcher: one screen that asks what to open, drawn as `launchHTML` in
//! `docs/mockup/corral-pi-build.html`. A header row names `corral / pi` and the directory it started
//! in. Under the question its rows sit under their section's heading in the order given (start a
//! build session, an assistant or the PM, resume a stopped session): a bar that marks the
//! selection, an 11-cell name column, the description and the row's key as a key cap. A block under
//! the list shows what the row with options (a build session) would open with: the project, where it
//! writes and the model providers its agents may use ([`LauncherContext`]). The footer, under a
//! rule, names the keys as key caps.
//!
//! `↑↓` or `j`/`k` select, `⏎` opens, a row's key opens that row directly, `q`, Esc or Ctrl+C leave
//! without opening anything. While the row with options is selected `←→` cycle its option, and a
//! click on the option line moves to the next option (on `‹`, the previous one). A click selects a
//! row and a click on the selected row opens it; the wheel moves the selection and every footer key
//! cap presses its key. Below [`FOLD_BELOW`] columns each row folds: the name and key cap on one
//! line, the description wrapped under it.
//!
//! [`State`] and [`draw`] are pure so tests drive them on a `TestBackend`; [`pick`] owns the
//! terminal.

use std::path::PathBuf;

use anyhow::{Context, Result};
use ratatui::Frame;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::hit::{self, Hit, HitMap};
use crate::overlays::truncate;

/// The canvas behind every cell.
const BG: Color = Color::Rgb(0x19, 0x1b, 0x20);
/// Names and the chosen option.
const TEXT: Color = Color::Rgb(0xe0, 0xe3, 0xe9);
/// Section headings, descriptions, labels.
const MUTED: Color = Color::Rgb(0x9b, 0xa2, 0xb0);
/// The question, the title, the selection bar, key cap glyphs and the option chevrons.
pub(crate) const ACCENT: Color = Color::Rgb(0x83, 0xa8, 0xef);
/// The rule above the footer.
const LINE: Color = Color::Rgb(0x35, 0x3a, 0x47);
/// The selected row's surface.
pub(crate) const SELECTED: Color = Color::Rgb(0x2c, 0x35, 0x48);
/// A key cap's surface.
pub(crate) const KEY: Color = Color::Rgb(0x26, 0x2b, 0x37);
/// A provider's login is present.
pub(crate) const OK: Color = Color::Rgb(0x9a, 0xc7, 0xa3);
/// A provider has no login.
pub(crate) const WARN: Color = Color::Rgb(0xe3, 0xbd, 0x7c);

#[path = "launcher_block.rs"]
mod block;

use block::option_block;
pub use block::{LauncherContext, Login, ProviderLogin};

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

/// Terminals narrower than this fold each row: name and key cap, then the wrapped description.
pub const FOLD_BELOW: u16 = 76;
/// The launcher column never grows wider than this; wider terminals center it.
const MAX_WIDTH: u16 = 104;
/// The name column of an unfolded row.
const NAME_COL: usize = 11;
/// From this many columns the footer names every key, ending with how to leave.
const FULL_FOOTER_FROM: u16 = 105;
/// Below this many columns the launcher is compact: no blank row under the header, one cell between
/// footer hints and [`COMPACT_HINTS`] of them.
const COMPACT_BELOW: u16 = 64;
/// Footer hints kept between [`COMPACT_BELOW`] and [`FULL_FOOTER_FROM`] columns.
const NARROW_HINTS: usize = 4;
/// Footer hints kept below [`COMPACT_BELOW`] columns.
const COMPACT_HINTS: usize = 2;
/// Terminals shorter than this drop the blank row under the header.
const SHORT_BELOW: u16 = 28;
/// Terminals shorter than this drop the rule above the footer.
const RULE_FROM: u16 = 16;
const TITLE: &str = "corral / pi";
const HEADING: &str = "What do you want to open?";
const MARKER: &str = "▌";
/// What a row's options choose; it names the footer's `←→`.
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
    /// The question, a section heading, spacing, the Project and Models rows, or where it writes
    /// while its row is not selected.
    Gap,
    /// A line of a row: a click selects or opens the row.
    Row(usize),
    /// A line of the selected row's option block: a click moves to the next option, or to the
    /// previous one on `‹`, which sits at this column when the line carries it.
    Option(Option<usize>),
}

fn fg(color: Color) -> Style {
    Style::new().fg(color)
}

/// A key cap: the key in accent on the key surface, one cell of padding each side.
fn cap(key: &str) -> Span<'static> {
    Span::styled(format!(" {key} "), Style::new().fg(ACCENT).bg(KEY))
}

fn cap_width(key: &str) -> usize {
    key.width() + 2
}

/// Spaces after `text` up to display width `width`.
fn padded(text: String, width: usize) -> String {
    let fill = width.saturating_sub(text.width());
    text + &" ".repeat(fill)
}

/// One row's lines. Unfolded it follows the mock's grid: bar, gap, 11-cell name, gap, description,
/// gap, key cap. Folded the name runs to the key cap and the description wraps under the name.
fn row_lines(
    entry: &LauncherEntry,
    selected: bool,
    width: usize,
    folded: bool,
) -> Vec<Line<'static>> {
    let bar = Span::styled(if selected { MARKER } else { " " }, fg(ACCENT));
    let name_style = fg(TEXT).add_modifier(Modifier::BOLD);
    let key = entry.key.map(String::from);
    // The key column and the gap before it, which the grid keeps even when a row has no key.
    let tail = key.as_deref().map_or(0, cap_width) + 1;
    let key_spans = key
        .iter()
        .flat_map(|k| [Span::raw(" "), cap(k)])
        .collect::<Vec<_>>();
    if folded {
        let name_cols = width.saturating_sub(2 + tail);
        let name = padded(truncate(&entry.name, name_cols), name_cols);
        let mut first = vec![bar, Span::raw(" "), Span::styled(name, name_style)];
        first.extend(key_spans);
        let mut lines = vec![Line::from(first)];
        if !entry.detail.is_empty() {
            let cols = width.saturating_sub(2);
            let detail = [(entry.detail.clone(), fg(MUTED))];
            lines.extend(block::wrap(&detail, cols).into_iter().map(|pieces| {
                let mut line = vec![Span::raw("  ")];
                line.extend(block::spans(pieces));
                Line::from(line)
            }));
        }
        return lines;
    }
    let detail_cols = width.saturating_sub(2 + NAME_COL + 1 + tail);
    let name = padded(truncate(&entry.name, NAME_COL), NAME_COL);
    let detail = padded(truncate(&entry.detail, detail_cols), detail_cols);
    let mut spans = vec![
        bar,
        Span::raw(" "),
        Span::styled(name, name_style),
        Span::raw(" "),
        Span::styled(detail, fg(MUTED)),
    ];
    spans.extend(key_spans);
    vec![Line::from(spans)]
}

/// The body lines, each with what it is to the selection and the mouse.
fn body_lines(
    entries: &[LauncherEntry],
    state: &State,
    context: &LauncherContext,
    width: usize,
    folded: bool,
    margin: bool,
) -> Vec<(Line<'static>, Part)> {
    let mut out = Vec::new();
    if margin {
        out.push((Line::default(), Part::Gap));
    }
    out.push((
        Line::from(Span::styled(
            truncate(HEADING, width),
            fg(ACCENT).add_modifier(Modifier::BOLD),
        )),
        Part::Gap,
    ));
    let mut section = None;
    for (i, entry) in entries.iter().enumerate() {
        if section != Some(entry.section) {
            section = Some(entry.section);
            out.push((Line::default(), Part::Gap));
            out.push((
                Line::from(Span::styled(entry.section.title(), fg(MUTED))),
                Part::Gap,
            ));
        }
        out.extend(
            row_lines(entry, i == state.selected, width, folded)
                .into_iter()
                .map(|line| (line, Part::Row(i))),
        );
    }
    let block = option_block(entries, state, context, width, folded);
    if !block.is_empty() {
        out.push((Line::default(), Part::Gap));
        out.extend(block);
    }
    out
}

/// A footer hint: its key caps, each pressing its own key, then the label, which presses `press`.
struct Hint {
    caps: Vec<(String, KeyCode)>,
    label: String,
    press: KeyCode,
}

impl Hint {
    fn new(caps: &[(&str, KeyCode)], label: &str, press: KeyCode) -> Self {
        Hint {
            caps: caps
                .iter()
                .map(|(k, code)| ((*k).to_string(), *code))
                .collect(),
            label: label.to_string(),
            press,
        }
    }
}

/// A row's name as the footer names it: lowercase, unless it is written in capitals (`PM`).
fn hint_label(name: &str) -> String {
    if name.chars().any(char::is_lowercase) {
        name.to_lowercase()
    } else {
        name.to_string()
    }
}

/// The footer's hints in the mock's order: selecting, opening, the selected row's options, each
/// row key, then leaving.
fn hints(entries: &[LauncherEntry], selected: usize) -> Vec<Hint> {
    let mut hints = vec![
        Hint::new(
            &[("↑", KeyCode::Up), ("↓", KeyCode::Down)],
            "select",
            KeyCode::Down,
        ),
        Hint::new(&[("⏎", KeyCode::Enter)], "open", KeyCode::Enter),
    ];
    if entries.get(selected).is_some_and(|e| !e.options.is_empty()) {
        hints.push(Hint::new(
            &[("←", KeyCode::Left), ("→", KeyCode::Right)],
            OPTION_LABEL,
            KeyCode::Right,
        ));
    }
    hints.extend(entries.iter().filter_map(|e| {
        e.key.map(|k| Hint {
            caps: vec![(k.to_string(), KeyCode::Char(k))],
            label: hint_label(&e.name),
            press: KeyCode::Char(k),
        })
    }));
    hints.push(Hint::new(&[("esc", KeyCode::Esc)], "leave", KeyCode::Esc));
    hints
}

/// The footer row from `x`: each hint that fits before `right` whole, `gap` cells apart, its key
/// caps and its label registered in `hits`.
fn draw_footer(
    f: &mut Frame,
    hints: &[Hint],
    x: u16,
    y: u16,
    right: u16,
    gap: u16,
    hits: &mut HitMap,
) {
    let mut cx = x;
    for hint in hints {
        let label = format!(" {}", hint.label);
        let caps: usize = hint.caps.iter().map(|(k, _)| cap_width(k)).sum();
        let Ok(w) = u16::try_from(caps + label.width()) else {
            break;
        };
        if cx.saturating_add(w) > right {
            break;
        }
        let mut spans = Vec::new();
        let mut kx = cx;
        for (key, code) in &hint.caps {
            let kw = u16::try_from(cap_width(key)).unwrap_or(0);
            hits.push(Rect::new(kx, y, kw, 1), Hit::Key(hit::press(*code)));
            spans.push(cap(key));
            kx += kw;
        }
        hits.push(
            Rect::new(kx, y, cx + w - kx, 1),
            Hit::Key(hit::press(hint.press)),
        );
        spans.push(Span::styled(label, fg(MUTED)));
        f.render_widget(Paragraph::new(Line::from(spans)), Rect::new(cx, y, w, 1));
        cx = cx.saturating_add(w).saturating_add(gap);
    }
}

/// `corral / pi` in accent at the left of `row`, `place` muted at its right.
fn draw_header(f: &mut Frame, place: &str, row: Rect) {
    let width = usize::from(row.width);
    let title = truncate(TITLE, width);
    let place = truncate(place, width.saturating_sub(title.width() + 2));
    let fill = width.saturating_sub(title.width() + place.width());
    let line = Line::from(vec![
        Span::styled(title, fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(fill)),
        Span::styled(place, fg(MUTED)),
    ]);
    f.render_widget(Paragraph::new(line), row);
}

/// Draws the launcher over the whole frame, `place` (the directory it started in) in the header and
/// `context` in the option block, and registers its rows, its live option line and its footer key
/// caps in `hits`.
pub(crate) fn draw(
    f: &mut Frame,
    entries: &[LauncherEntry],
    place: &str,
    context: &LauncherContext,
    state: &mut State,
    hits: &mut HitMap,
) {
    let area = f.area();
    f.render_widget(Block::new().style(Style::new().bg(BG).fg(TEXT)), area);
    if area.width == 0 || area.height == 0 {
        return;
    }
    let pad = u16::from(area.width > 8);
    let inner = area.width - pad * 2;
    let header = u16::from(area.height >= 3);
    let footer = u16::from(area.height >= 2);
    let rule = u16::from(area.height >= RULE_FROM);
    if header == 1 {
        draw_header(f, place, Rect::new(area.x + pad, area.y, inner, 1));
    }
    if footer == 1 {
        let y = area.bottom() - 1;
        if rule == 1 {
            let line = Span::styled("─".repeat(usize::from(area.width)), fg(LINE));
            f.render_widget(
                Paragraph::new(line),
                Rect::new(area.x, y - 1, area.width, 1),
            );
        }
        let (kept, gap) = if area.width >= FULL_FOOTER_FROM {
            (usize::MAX, 2)
        } else if area.width >= COMPACT_BELOW {
            (NARROW_HINTS, 2)
        } else {
            (COMPACT_HINTS, 1)
        };
        let shown: Vec<Hint> = hints(entries, state.selected)
            .into_iter()
            .take(kept)
            .collect();
        draw_footer(f, &shown, area.x + pad, y, area.x + pad + inner, gap, hits);
    }
    let width = inner.min(MAX_WIDTH);
    let x = area.x + pad + (inner - width) / 2;
    let body = Rect::new(
        x,
        area.y + header,
        width,
        area.height - header - footer - rule,
    );
    let margin = area.width >= COMPACT_BELOW && area.height >= SHORT_BELOW;
    let lines = body_lines(
        entries,
        state,
        context,
        usize::from(width),
        area.width < FOLD_BELOW,
        margin,
    );
    let height = usize::from(body.height);
    let selected = state.selected;
    if let (Some(first), Some(last)) = (
        lines
            .iter()
            .position(|(_, part)| *part == Part::Row(selected)),
        lines
            .iter()
            .rposition(|(_, part)| *part == Part::Row(selected)),
    ) {
        // The selected row's live option block, the last lines, comes into view with it when both
        // fit.
        let live = lines
            .iter()
            .any(|(_, part)| matches!(part, Part::Option(_)));
        let end = lines.len().saturating_sub(1);
        let last = if live && end >= first && end + 1 - first <= height {
            last.max(end)
        } else {
            last
        };
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
        let style = if part == Part::Row(selected) {
            Style::new().bg(SELECTED)
        } else {
            Style::new()
        };
        f.render_widget(Paragraph::new(line).style(style), rect);
        match part {
            Part::Gap => {}
            Part::Row(i) => hits.push(rect, Hit::Item(Z_ROW, i)),
            Part::Option(back) => {
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
}

/// Draws the launcher as it first opens (the first row selected, every row on its first option)
/// with `place` in the header and `context` in the option block, for captures that compare it with
/// the mock.
pub fn preview(f: &mut Frame, entries: &[LauncherEntry], place: &str, context: &LauncherContext) {
    draw(
        f,
        entries,
        place,
        context,
        &mut State::default(),
        &mut HitMap::default(),
    );
}

/// The directory the launcher started in, with the home directory written as `~`.
fn start_place() -> String {
    let Ok(dir) = std::env::current_dir() else {
        return String::new();
    };
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && let Ok(rest) = dir.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        };
    }
    dir.display().to_string()
}

/// Shows the launcher on the alternate screen, `context` in its option block, and returns the id of
/// the row the owner opened with the index of its chosen option (0 for rows without options), or
/// `None` when they left. The terminal is restored on every return, including errors; a panic is
/// restored by ratatui's hook.
pub fn pick(
    entries: &[LauncherEntry],
    context: &LauncherContext,
) -> Result<Option<(String, usize)>> {
    let mut terminal = ratatui::try_init().context("initializing the terminal")?;
    let picked = run(&mut terminal, entries, context);
    ratatui::restore();
    picked
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    entries: &[LauncherEntry],
    context: &LauncherContext,
) -> Result<Option<(String, usize)>> {
    let env = std::env::var("CORRAL_MOUSE").ok();
    let mouse = crate::term::mouse_wanted(false, env.as_deref(), true);
    let _mouse = mouse.then(crate::term::MouseGuard::enable).transpose()?;
    let place = start_place();
    let mut state = State::default();
    let mut hits = HitMap::default();
    loop {
        terminal
            .draw(|f| {
                hits.clear();
                draw(f, entries, &place, context, &mut state, &mut hits);
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
