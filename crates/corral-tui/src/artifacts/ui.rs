//! Small render helpers shared by the detail, review and artifact views: scrolling text, diff coloring,
//! artifact rows, the preview popup (text or image) and the open toast.

use std::path::Path;

use corral_core::{Artifact, ArtifactKind};
use corral_ops::{Ctx, Preview};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui_image::protocol::StatefulProtocol;
use unicode_width::UnicodeWidthStr;

use super::image;

use crate::theme::{self, BG, BG2, CYAN, DIM, FG, GREEN, MAGENTA, RED, SELECTED_BG, YELLOW};

/// Width of the kind column in artifact lists (the mock's `.kd` is 11ch).
pub(crate) const KIND_COL: usize = 11;

/// Vertical scroll offset for a text box. The offset is clamped at draw time against the content height.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Scroll {
    pub offset: usize,
}

impl Scroll {
    /// Handles j/k, arrows and PgUp/PgDn. Returns true when the key was a scroll key.
    pub fn on_key(&mut self, key: &KeyEvent, page: usize) -> bool {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.offset = self.offset.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => self.offset = self.offset.saturating_sub(1),
            KeyCode::PageDown => self.offset = self.offset.saturating_add(page),
            KeyCode::PageUp => self.offset = self.offset.saturating_sub(page),
            KeyCode::Home | KeyCode::Char('g') => self.offset = 0,
            _ => return false,
        }
        true
    }

    /// Scrolls `dir` wheel notches (down when positive); the draw-time clamp bounds it.
    pub fn wheel(&mut self, dir: isize) {
        self.offset = self
            .offset
            .saturating_add_signed(dir * crate::hit::WHEEL_LINES);
    }

    /// Clamps the offset so the last page stays filled, and returns it.
    pub fn clamp(&mut self, content: usize, height: usize) -> usize {
        self.offset = self.offset.min(content.saturating_sub(height));
        self.offset
    }
}

/// Number of rows `lines` take when wrapped at `width` (approximation of ratatui's word wrap).
pub(crate) fn wrapped_height(lines: &[Line], width: u16) -> usize {
    let w = usize::from(width.max(1));
    lines.iter().map(|l| l.width().max(1).div_ceil(w)).sum()
}

/// Renders `lines` in `area` with wrapping (when `wrap`) and the scroll offset applied.
pub(crate) fn scrolled(
    f: &mut Frame,
    area: Rect,
    lines: Vec<Line<'static>>,
    scroll: &mut Scroll,
    wrap: bool,
) {
    let content = if wrap {
        wrapped_height(&lines, area.width)
    } else {
        lines.len()
    };
    let y = scroll.clamp(content, usize::from(area.height));
    let mut p = Paragraph::new(lines)
        .style(Style::new().fg(FG))
        .scroll((u16::try_from(y).unwrap_or(u16::MAX), 0));
    if wrap {
        p = p.wrap(Wrap { trim: false });
    }
    f.render_widget(p, area);
}

pub(crate) fn paint_bg(f: &mut Frame, area: Rect) {
    f.render_widget(Block::new().style(Style::new().bg(BG).fg(FG)), area);
}

/// Unified diff text colored like the mock: `+` green, `-` red, `@@` cyan, file headers dim.
pub(crate) fn diff_lines(text: &str) -> Vec<Line<'static>> {
    text.lines()
        .map(|l| {
            let style = if l.starts_with("+++")
                || l.starts_with("---")
                || l.starts_with("diff ")
                || l.starts_with("index ")
            {
                Style::new().fg(DIM)
            } else if l.starts_with('+') {
                Style::new().fg(GREEN)
            } else if l.starts_with('-') {
                Style::new().fg(RED)
            } else if l.starts_with("@@") {
                Style::new().fg(CYAN)
            } else if l.starts_with("...") {
                Style::new().fg(DIM)
            } else {
                Style::new().fg(FG)
            };
            Line::from(Span::styled(expand_tabs(l), style))
        })
        .collect()
}

/// Markdown-ish text: headings cyan bold, checked boxes green, everything else plain.
pub(crate) fn text_lines(text: &str) -> Vec<Line<'static>> {
    text.lines()
        .map(|l| {
            let t = l.trim_start();
            if t.starts_with('#') {
                Line::from(Span::styled(
                    l.to_string(),
                    Style::new().fg(CYAN).add_modifier(Modifier::BOLD),
                ))
            } else if let Some(rest) = t.strip_prefix("- [x]").or_else(|| t.strip_prefix("[x]")) {
                Line::from(vec![
                    Span::styled("[x]", Style::new().fg(GREEN)),
                    Span::raw(rest.to_string()),
                ])
            } else if t.starts_with('✓') {
                Line::from(Span::styled(l.to_string(), Style::new().fg(GREEN)))
            } else if t.starts_with('⚠') {
                Line::from(Span::styled(l.to_string(), Style::new().fg(YELLOW)))
            } else {
                Line::from(expand_tabs(l))
            }
        })
        .collect()
}

fn expand_tabs(s: &str) -> String {
    s.replace('\t', "    ")
}

/// Lines for an artifact preview: diffs colored, other text markdown-ish, images and external files a note.
pub(crate) fn preview_lines(artifact: &Artifact, preview: &Preview) -> Vec<Line<'static>> {
    match preview {
        Preview::Text(t) => {
            let p = artifact.path.to_ascii_lowercase();
            if artifact.kind == ArtifactKind::Diff || p.ends_with(".diff") || p.ends_with(".patch")
            {
                diff_lines(t)
            } else {
                text_lines(t)
            }
        }
        Preview::Image { path, note } | Preview::External { path, note } => vec![
            Line::from(Span::styled(note.clone(), theme::dim())),
            Line::default(),
            open_hint(),
            Line::default(),
            Line::from(Span::styled(path.clone(), theme::dim())),
        ],
    }
}

fn open_hint() -> Line<'static> {
    Line::from(vec![
        Span::styled("⏎", Style::new().fg(CYAN)),
        Span::raw(" open"),
    ])
}

/// A loaded artifact preview: text lines, or a decoded image plus a one-line caption.
pub(crate) enum PreviewBody {
    Lines(Vec<Line<'static>>),
    Image {
        state: Box<StatefulProtocol>,
        caption: String,
    },
}

/// Loads `a`'s preview. Images that fail to decode fall back to the note lines with the error appended;
/// other errors become the content (red) instead of failing.
pub(crate) fn load_preview(ctx: &Ctx, a: &Artifact) -> PreviewBody {
    match corral_ops::artifact_preview(ctx, a) {
        Ok(Preview::Image { path, note }) => match image::load(&path) {
            Ok(state) => PreviewBody::Image {
                state: Box::new(state),
                caption: note,
            },
            Err(e) => PreviewBody::Lines(preview_lines(
                a,
                &Preview::External {
                    path,
                    note: format!("{note}; preview failed: {e:#}"),
                },
            )),
        },
        Ok(p) => PreviewBody::Lines(preview_lines(a, &p)),
        Err(e) => PreviewBody::Lines(vec![Line::from(Span::styled(
            format!("preview failed: {e:#}"),
            Style::new().fg(RED),
        ))]),
    }
}

/// Draws a preview in `area`: text scrolls; an image fits above a caption + "⏎ open" line.
pub(crate) fn draw_preview(f: &mut Frame, area: Rect, body: &mut PreviewBody, scroll: &mut Scroll) {
    match body {
        PreviewBody::Lines(lines) => scrolled(f, area, lines.clone(), scroll, true),
        PreviewBody::Image { state, caption } => {
            let [img, foot] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
            image::draw(f, img, state);
            let mut spans = vec![Span::styled(format!("{caption}  "), theme::dim())];
            spans.extend(open_hint().spans);
            f.render_widget(Paragraph::new(Line::from(spans)), foot);
        }
    }
}

/// "◆ kind       title  R1 · 4 KB" row for an artifact list.
pub(crate) fn artifact_line(a: &Artifact, selected: bool, extra: Option<String>) -> Line<'static> {
    let mut spans = vec![
        Span::raw("◆ "),
        Span::styled(
            format!("{:<KIND_COL$}", a.kind.as_str()),
            Style::new().fg(MAGENTA),
        ),
        Span::raw(a.title.clone()),
        Span::styled(
            format!(
                "  {}{} · {}",
                extra.map(|e| format!("{e} ")).unwrap_or_default(),
                a.run,
                human_bytes(a.bytes)
            ),
            theme::dim(),
        ),
    ];
    if selected {
        spans
            .iter_mut()
            .for_each(|s| s.style = s.style.bg(SELECTED_BG));
        return Line::from(spans).style(Style::new().bg(SELECTED_BG));
    }
    Line::from(spans)
}

pub(crate) fn human_bytes(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1} MB", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{} KB", n.div_ceil(1_000))
    } else {
        format!("{n} B")
    }
}

/// File name part of an artifact snapshot path, without the "<sha8>-" prefix.
pub(crate) fn artifact_file_name(a: &Artifact) -> String {
    let name = Path::new(&a.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.split_once('-') {
        Some((sha, rest)) if sha.len() == 8 && sha.chars().all(|c| c.is_ascii_hexdigit()) => {
            rest.to_string()
        }
        _ => name,
    }
}

pub(crate) fn artifact_abs_path(ctx: &Ctx, a: &Artifact) -> String {
    ctx.store.dir().join(&a.path).to_string_lossy().into_owned()
}

/// `path` relative to the workspace root when inside it (mock: "mitiplay/.worktrees/T-042").
pub(crate) fn rel_path(ctx: &Ctx, path: &Path) -> String {
    path.strip_prefix(ctx.store.root())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// A rect of `w` x `h` cells centered in `area` (clamped to it).
pub(crate) fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Popup frame (mock `.pop`): clears the rect and draws a rounded BG2 box with a title; returns the inner rect.
pub(crate) fn popup(f: &mut Frame, rect: Rect, title: &str, right: Option<String>) -> Rect {
    f.render_widget(Clear, rect);
    let mut block = theme::titled(title).style(Style::new().bg(BG2).fg(FG));
    if let Some(r) = right {
        block = block.title_top(
            Line::from(Span::styled(format!(" {r} "), theme::dim())).alignment(Alignment::Right),
        );
    }
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    inner
}

/// A box (rounded border) with optional dim right title; returns the inner rect.
pub(crate) fn boxed(
    f: &mut Frame,
    rect: Rect,
    title: &str,
    focused: bool,
    right: Option<String>,
) -> Rect {
    let title_style = if focused {
        Style::new().fg(CYAN).add_modifier(Modifier::BOLD)
    } else {
        theme::bold()
    };
    let mut block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::BORDER))
        .title(Span::styled(format!(" {title} "), title_style));
    if let Some(r) = right {
        block = block.title_top(
            Line::from(Span::styled(format!(" {r} "), theme::dim())).alignment(Alignment::Right),
        );
    }
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    inner
}

/// An artifact preview opened from a list (detail / review): scrollable text or an image; ⏎ opens it with
/// its viewer (`corral_ops::open_path`).
pub(crate) struct PreviewPopup {
    title: String,
    right: String,
    path: String,
    body: PreviewBody,
    scroll: Scroll,
}

impl PreviewPopup {
    /// Loads the preview; errors become the popup content (red) instead of failing.
    pub fn open(ctx: &Ctx, a: &Artifact) -> PreviewPopup {
        PreviewPopup {
            title: format!("{} · {}", a.kind.as_str(), a.title),
            right: format!("{} · {}", artifact_file_name(a), human_bytes(a.bytes)),
            path: artifact_abs_path(ctx, a),
            body: load_preview(ctx, a),
            scroll: Scroll::default(),
        }
    }

    /// Draws the popup modal: `×` and an outside click close it (the owning view maps them to Esc).
    pub fn draw(&mut self, f: &mut Frame, area: Rect, hits: &mut crate::HitMap) {
        let w = (u32::from(area.width) * 85 / 100) as u16;
        let h = (u32::from(area.height) * 85 / 100) as u16;
        let rect = centered(area, w.max(40), h.max(8));
        // Trailing room keeps the right title clear of the `×`.
        let inner = popup(f, rect, &self.title, Some(format!("{}   ", self.right)));
        crate::hit::modal_popup(f, rect, hits);
        draw_preview(f, inner, &mut self.body, &mut self.scroll);
    }

    /// Wheel scrolling: `dir` notches down (positive) or up (negative).
    pub fn wheel(&mut self, dir: isize) {
        self.scroll.wheel(dir);
    }

    /// Scroll keys scroll; ⏎ opens the snapshot with its viewer. Returns Some(toast) for ⏎.
    pub fn on_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<crate::Toast> {
        if key.code == KeyCode::Enter {
            return Some(open_toast(ctx, &self.path));
        }
        self.scroll.on_key(key, 10);
        None
    }
}

/// Opens `path` through the shared viewer routing; the toast names the command, or the error with the command
/// that failed.
pub(crate) fn open_toast(ctx: &Ctx, path: &str) -> crate::Toast {
    match corral_ops::open_path(ctx, Path::new(path), false) {
        Ok(opened) => crate::Toast::new(crate::Tone::Info, "Opened", opened.command()),
        Err(e) => crate::Toast::new(crate::Tone::Bad, "Could not open", format!("{e:#}")),
    }
}

/// Pads/truncates `s` to `w` display columns.
pub(crate) fn fit(s: &str, w: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = UnicodeWidthStr::width(ch.to_string().as_str());
        if used + cw > w {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push_str(&" ".repeat(w - used));
    out
}

/// Elapsed time from `start` to `end` as "41m" / "2h05m".
pub(crate) fn elapsed(start: jiff::Timestamp, end: Option<jiff::Timestamp>) -> String {
    let end = end.unwrap_or_else(jiff::Timestamp::now);
    let mins = end.duration_since(start).as_secs().max(0) / 60;
    if mins >= 60 {
        format!("{}h{:02}m", mins / 60, mins % 60)
    } else {
        format!("{mins}m")
    }
}

/// `ts` as a date in the system time zone, formatted with `fmt` (strftime, e.g. "%Y-%m-%d" or "%b %d").
pub(crate) fn local_date(ts: jiff::Timestamp, fmt: &str) -> String {
    ts.to_zoned(jiff::tz::TimeZone::system())
        .strftime(fmt)
        .to_string()
}

/// Single-line text input with a cursor block at the end.
pub(crate) fn input_spans(text: &str, placeholder: &str) -> Vec<Span<'static>> {
    if text.is_empty() {
        vec![
            Span::styled("▏", Style::new().fg(CYAN)),
            Span::styled(placeholder.to_string(), theme::dim()),
        ]
    } else {
        vec![
            Span::raw(text.to_string()),
            Span::styled("▏", Style::new().fg(CYAN)),
        ]
    }
}

/// Applies typing keys to a text buffer: chars append, Backspace deletes, ctrl-u clears. True when handled.
pub(crate) fn edit_text(buf: &mut String, key: &KeyEvent) -> bool {
    use ratatui::crossterm::event::KeyModifiers;
    match key.code {
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => buf.clear(),
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            buf.push(c)
        }
        KeyCode::Backspace => {
            buf.pop();
        }
        _ => return false,
    }
    true
}

pub(crate) fn is_press(key: &KeyEvent) -> bool {
    key.kind != ratatui::crossterm::event::KeyEventKind::Release
}
