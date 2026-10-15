//! Markdown -> styled lines, wrapped to the given width (pulldown-cmark events, no I/O).

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::formats;
use super::wrap::{self, Seg};
use crate::theme::{BG2, BLUE, CYAN, DIM, FG, GREEN, YELLOW};

struct ListCtx {
    next: Option<u64>,
    marker_w: usize,
    pending: Option<String>,
}

struct Md {
    width: usize,
    out: Vec<Line<'static>>,
    segs: Vec<Seg>,
    styles: Vec<Style>,
    quote: usize,
    lists: Vec<ListCtx>,
    code: Option<(String, String)>,
    links: Vec<String>,
    rows: Vec<Vec<Vec<Seg>>>,
    row: Vec<Vec<Seg>>,
    heading: Option<HeadingLevel>,
}

/// Renders Markdown for reading at `width` columns; code blocks stay unwrapped for horizontal scrolling.
pub(crate) fn render(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut md = Md {
        width: width.max(8),
        out: Vec::new(),
        segs: Vec::new(),
        styles: vec![Style::new().fg(FG)],
        quote: 0,
        lists: Vec::new(),
        code: None,
        links: Vec::new(),
        rows: Vec::new(),
        row: Vec::new(),
        heading: None,
    };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for ev in Parser::new_ext(text, opts) {
        md.event(ev);
    }
    md.flush();
    while md.out.last().is_some_and(|l| l.width() == 0) {
        md.out.pop();
    }
    md.out
}

impl Md {
    fn style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_style(&mut self, f: impl FnOnce(Style) -> Style) {
        let s = f(self.style());
        self.styles.push(s);
    }

    fn gap(&mut self) {
        if self.lists.is_empty() && self.out.last().is_some_and(|l| l.width() > 0) {
            self.out.push(Line::default());
        }
    }

    fn prefixes(&mut self) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
        let (mut first, mut rest) = (Vec::new(), Vec::new());
        for _ in 0..self.quote {
            let bar = Span::styled("│ ", Style::new().fg(DIM));
            first.push(bar.clone());
            rest.push(bar);
        }
        for l in &mut self.lists {
            let pad = Span::raw(" ".repeat(l.marker_w));
            rest.push(pad.clone());
            match l.pending.take() {
                Some(m) => first.push(Span::styled(m, Style::new().fg(CYAN))),
                None => first.push(pad),
            }
        }
        (first, rest)
    }

    fn flush(&mut self) {
        if self.segs.is_empty() {
            return;
        }
        let segs = std::mem::take(&mut self.segs);
        let (first, rest) = self.prefixes();
        let pw: usize = first.iter().map(|s| s.content.width()).sum();
        let lines = wrap::wrap(&wrap::chunks(&segs), self.width.saturating_sub(pw));
        for (i, body) in lines.into_iter().enumerate() {
            let mut spans = if i == 0 { first.clone() } else { rest.clone() };
            spans.extend(body);
            self.out.push(Line::from(spans));
        }
    }

    fn heading_style(level: HeadingLevel) -> Style {
        let bold = Modifier::BOLD;
        match level {
            HeadingLevel::H1 => Style::new()
                .fg(CYAN)
                .add_modifier(bold | Modifier::UNDERLINED),
            HeadingLevel::H2 => Style::new().fg(CYAN).add_modifier(bold),
            HeadingLevel::H3 => Style::new().fg(GREEN).add_modifier(bold),
            _ => Style::new().fg(YELLOW).add_modifier(bold),
        }
    }

    fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => match &mut self.code {
                Some((_, buf)) => buf.push_str(&t),
                None => {
                    let s = self.style();
                    self.segs.push((t.into_string(), s));
                }
            },
            Event::Code(t) => self
                .segs
                .push((t.into_string(), Style::new().fg(YELLOW).bg(BG2))),
            Event::SoftBreak | Event::HardBreak => self.segs.push((" ".into(), self.style())),
            Event::Html(t) | Event::InlineHtml(t) => {
                self.segs.push((t.into_string(), Style::new().fg(DIM)));
            }
            Event::TaskListMarker(done) => {
                let (m, c) = if done { ("[x] ", GREEN) } else { ("[ ] ", DIM) };
                self.segs.push((m.into(), Style::new().fg(c)));
            }
            Event::Rule => {
                self.flush();
                self.gap();
                let w = self.width.saturating_sub(self.quote * 2);
                self.out.push(Line::from(Span::styled(
                    "─".repeat(w),
                    Style::new().fg(DIM),
                )));
                self.gap();
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.flush();
                self.gap();
            }
            Tag::Heading { level, .. } => {
                self.flush();
                self.gap();
                self.heading = Some(level);
                self.push_style(|_| Self::heading_style(level));
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.gap();
                self.quote += 1;
                self.push_style(|s| s.add_modifier(Modifier::ITALIC));
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.gap();
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => {
                        l.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(start) => {
                self.flush();
                if self.lists.is_empty() {
                    self.gap();
                }
                self.lists.push(ListCtx {
                    next: start,
                    marker_w: 2,
                    pending: None,
                });
            }
            Tag::Item => {
                self.flush();
                let depth = self.lists.len();
                if let Some(l) = self.lists.last_mut() {
                    let marker = match l.next {
                        Some(n) => {
                            l.next = Some(n + 1);
                            format!("{n}. ")
                        }
                        None if depth > 1 => "◦ ".to_string(),
                        None => "• ".to_string(),
                    };
                    l.marker_w = marker.width();
                    l.pending = Some(marker);
                }
            }
            Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.links.push(dest_url.into_string());
                self.push_style(|s| s.fg(BLUE).add_modifier(Modifier::UNDERLINED));
            }
            Tag::Image { dest_url, .. } => {
                self.links.push(dest_url.into_string());
                self.segs.push(("[image] ".into(), Style::new().fg(DIM)));
            }
            Tag::Table(_) => {
                self.flush();
                self.gap();
                self.rows.clear();
            }
            Tag::TableHead | Tag::TableRow => self.row.clear(),
            Tag::TableCell => self.segs.clear(),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush(),
            TagEnd::Heading(level) => {
                self.flush();
                self.styles.pop();
                self.heading = None;
                if level == HeadingLevel::H1 {
                    let w = self.width.saturating_sub(self.quote * 2);
                    self.out.push(Line::from(Span::styled(
                        "═".repeat(w),
                        Style::new().fg(DIM),
                    )));
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
                self.styles.pop();
            }
            TagEnd::CodeBlock => self.end_code(),
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
            }
            TagEnd::Item => {
                self.flush();
                if let Some(l) = self.lists.last_mut() {
                    l.pending = None;
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                if let Some(url) = self.links.pop() {
                    self.segs.push((format!(" ({url})"), Style::new().fg(DIM)));
                }
            }
            TagEnd::Image => {
                if let Some(url) = self.links.pop() {
                    self.segs.push((format!(" ({url})"), Style::new().fg(DIM)));
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.segs);
                self.row.push(cell);
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                let row = std::mem::take(&mut self.row);
                self.rows.push(row);
            }
            TagEnd::Table => {
                let rows = std::mem::take(&mut self.rows);
                let (_, rest) = self.prefixes();
                let pw: usize = rest.iter().map(|s| s.content.width()).sum();
                for line in wrap::table(&rows, self.width.saturating_sub(pw), true) {
                    let mut spans = rest.clone();
                    spans.extend(line.spans);
                    self.out.push(Line::from(spans));
                }
                self.gap();
            }
            _ => {}
        }
    }

    fn end_code(&mut self) {
        let Some((lang, text)) = self.code.take() else {
            return;
        };
        let (first, rest) = self.prefixes();
        if !lang.is_empty() {
            let mut spans = first;
            spans.push(Span::styled(format!("─ {lang}"), Style::new().fg(DIM)));
            self.out.push(Line::from(spans));
        }
        for line in formats::render_lang(&lang, text.trim_end_matches('\n')) {
            let mut spans = rest.clone();
            spans.push(Span::styled("  ", Style::new().bg(BG2)));
            spans.extend(
                line.spans
                    .into_iter()
                    .map(|s| Span::styled(s.content, s.style.bg(BG2))),
            );
            self.out.push(Line::from(spans));
        }
        self.gap();
    }
}
