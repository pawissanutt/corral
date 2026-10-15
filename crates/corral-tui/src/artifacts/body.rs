//! A loaded text preview: rendered for reading or raw, drawn one viewport at a time from the complete document.

use corral_core::{Artifact, ArtifactKind};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::ui::{Scroll, artifact_file_name};
use crate::render::{self, Document};
use crate::theme::{self, CYAN, FG};
use crate::{Hit, HitMap};

pub(crate) struct TextBody {
    kind: ArtifactKind,
    doc: Document,
    /// Whole-document layout (Markdown, JSON, CSV) at the width it was laid out for, with its widest line.
    whole: Option<(u16, Vec<Line<'static>>, usize)>,
}

fn key(code: KeyCode, ctrl: bool) -> KeyEvent {
    let mods = if ctrl {
        KeyModifiers::CONTROL
    } else {
        KeyModifiers::NONE
    };
    KeyEvent::new(code, mods)
}

impl TextBody {
    pub fn new(a: &Artifact, text: String) -> TextBody {
        let format = render::detect(&artifact_file_name(a), a.kind);
        TextBody {
            kind: a.kind,
            doc: Document::new(format, text),
            whole: None,
        }
    }

    /// Flips rendered/raw for every preview of this artifact kind.
    pub fn toggle_raw(&mut self, scroll: &mut Scroll) {
        render::toggle_raw(self.kind);
        *scroll = Scroll::default();
    }

    /// Scrolls to the first error line of a log; false when there is none.
    pub fn jump_error(&mut self, scroll: &mut Scroll) -> bool {
        let Some(idx) = self.doc.first_error() else {
            return false;
        };
        scroll.offset = idx;
        scroll.x = 0;
        true
    }

    /// The popup key set: r toggles, e jumps to the first log error, h/l and arrows scroll sideways.
    pub fn on_key(&mut self, k: &KeyEvent, scroll: &mut Scroll) {
        match k.code {
            KeyCode::Char('r') => self.toggle_raw(scroll),
            KeyCode::Char('e') => {
                self.jump_error(scroll);
            }
            KeyCode::Left | KeyCode::Char('h') => scroll.scroll_x(-1),
            KeyCode::Right | KeyCode::Char('l') => scroll.scroll_x(1),
            _ => {}
        }
    }

    /// Draws a toolbar row (format, mode, clickable toggle and error jump) and the visible window below it.
    pub fn draw(
        &mut self,
        f: &mut Frame,
        area: Rect,
        scroll: &mut Scroll,
        ctrl: bool,
        hits: &mut HitMap,
    ) {
        let raw = render::is_raw(self.kind);
        let body = if area.height >= 4 {
            let bar = Rect::new(area.x, area.y, area.width, 1);
            self.draw_toolbar(f, bar, raw, ctrl, hits);
            Rect::new(area.x, area.y + 1, area.width, area.height - 1)
        } else {
            area
        };
        let (total, widest) = if !raw && self.doc.needs_whole() {
            if self.whole.as_ref().is_none_or(|(w, _, _)| *w != body.width) {
                let lines = render::render(self.doc.format(), self.doc.text(), body.width);
                let widest = lines.iter().map(Line::width).max().unwrap_or(0);
                self.whole = Some((body.width, lines, widest));
            }
            self.whole
                .as_ref()
                .map_or((0, 0), |(_, l, w)| (l.len(), *w))
        } else {
            (self.doc.line_count(), self.doc.widest())
        };
        // Content wider than the pane gets a clickable scroll bar row, so sideways reading has a pointer path.
        let wide = widest > usize::from(body.width) && body.height >= 5;
        let text = if wide {
            Rect::new(body.x, body.y, body.width, body.height - 1)
        } else {
            body
        };
        let height = usize::from(text.height);
        let y = scroll.clamp(total, height);
        scroll.x = scroll.x.min(widest.saturating_sub(usize::from(text.width)));
        let range = y..(y + height).min(total);
        let window = match &self.whole {
            Some((_, lines, _)) if !raw && self.doc.needs_whole() => lines[range].to_vec(),
            _ => self.doc.lines(raw, range),
        };
        let p = Paragraph::new(window)
            .style(Style::new().fg(FG))
            .scroll((0, u16::try_from(scroll.x).unwrap_or(u16::MAX)));
        f.render_widget(p, text);
        if wide {
            let bar = Rect::new(body.x, body.bottom() - 1, body.width, 1);
            Self::draw_hbar(f, bar, scroll.x, usize::from(text.width), widest, hits);
        }
    }

    /// `[◂] [▸]  columns a–b of w`: the buttons press Left and Right, the keys that scroll sideways.
    fn draw_hbar(
        f: &mut Frame,
        bar: Rect,
        x: usize,
        width: usize,
        widest: usize,
        hits: &mut HitMap,
    ) {
        let clip = |from: u16, w: u16| {
            Rect::new(
                bar.x + from,
                bar.y,
                w.min(bar.width.saturating_sub(from)),
                1,
            )
        };
        hits.push(clip(0, 3), Hit::Key(key(KeyCode::Left, false)));
        hits.push(clip(4, 3), Hit::Key(key(KeyCode::Right, false)));
        let shown = format!(
            "  columns {}–{} of {widest}",
            x + 1,
            (x + width).min(widest)
        );
        let line = Line::from(vec![
            Span::styled("[◂]", Style::new().fg(CYAN)),
            Span::raw(" "),
            Span::styled("[▸]", Style::new().fg(CYAN)),
            Span::styled(shown, theme::dim()),
        ]);
        f.render_widget(Paragraph::new(line), bar);
    }

    fn draw_toolbar(&self, f: &mut Frame, bar: Rect, raw: bool, ctrl: bool, hits: &mut HitMap) {
        let p = if ctrl { "^" } else { "" };
        let mode = if raw { "raw" } else { "rendered" };
        let mut segs: Vec<(String, Style, Option<KeyEvent>)> = vec![
            (
                format!("{} · {mode}  ", self.doc.format().label()),
                theme::dim(),
                None,
            ),
            (
                format!("[{p}r] {}", if raw { "rendered" } else { "raw" }),
                Style::new().fg(CYAN),
                Some(key(KeyCode::Char('r'), ctrl)),
            ),
        ];
        if self.doc.first_error().is_some() {
            segs.push((
                format!("  [{p}e] error"),
                Style::new().fg(CYAN),
                Some(key(KeyCode::Char('e'), ctrl)),
            ));
        }
        let mut x = bar.x;
        let mut spans = Vec::new();
        for (text, style, k) in segs {
            let w = text.width() as u16;
            if let Some(k) = k {
                let room = bar.right().saturating_sub(x).min(w);
                hits.push(Rect::new(x, bar.y, room, 1), Hit::Key(k));
            }
            x = x.saturating_add(w);
            spans.push(Span::styled(text, style));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), bar);
    }
}
