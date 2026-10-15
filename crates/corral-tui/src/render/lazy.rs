//! A whole artifact kept as text with a line index, rendered one viewport at a time so multi-MB logs stay fast.

use std::cell::Cell;
use std::ops::Range;

use ratatui::style::Style;
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use super::{Format, ansi, formats};

/// Markdown, JSON and CSV lay out the whole text; above this size they read as plain lines instead.
const WHOLE_LIMIT: usize = 1 << 20;

/// The complete text of an artifact plus what a viewport needs: line offsets, the SGR state at every log line,
/// the first error line and (on demand) the widest line.
pub struct Document {
    format: Format,
    text: String,
    starts: Vec<usize>,
    sgr: Vec<Style>,
    first_error: Option<usize>,
    widest: Cell<Option<usize>>,
}

impl Document {
    pub fn new(format: Format, text: String) -> Document {
        let format = match format {
            Format::Markdown | Format::Csv if text.len() > WHOLE_LIMIT => Format::Plain,
            f => f,
        };
        let mut starts: Vec<usize> = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        if starts.last() == Some(&text.len()) {
            starts.pop();
        }
        let log = format == Format::Log;
        Document {
            sgr: if log {
                ansi::style_starts(&text)
            } else {
                Vec::new()
            },
            first_error: log.then(|| ansi::first_error(&text)).flatten(),
            format,
            text,
            starts,
            widest: Cell::new(None),
        }
    }

    pub fn format(&self) -> &Format {
        &self.format
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn line_count(&self) -> usize {
        if self.text.is_empty() {
            0
        } else {
            self.starts.len()
        }
    }

    /// Index of the first error line of a log (found once, when the document was built).
    pub fn first_error(&self) -> Option<usize> {
        self.first_error
    }

    /// True when rendering needs the whole text at once (Markdown, JSON, CSV) rather than a line window.
    pub fn needs_whole(&self) -> bool {
        matches!(self.format, Format::Markdown | Format::Json | Format::Csv)
            && self.text.len() <= WHOLE_LIMIT
    }

    fn line(&self, i: usize) -> &str {
        let end = self.starts.get(i + 1).copied().unwrap_or(self.text.len());
        self.text[self.starts[i]..end].trim_end_matches(['\n', '\r'])
    }

    /// Display width of the widest line (computed once, over every line).
    pub fn widest(&self) -> usize {
        if let Some(w) = self.widest.get() {
            return w;
        }
        let w = (0..self.line_count())
            .map(|i| {
                let l = self.line(i);
                if l.contains('\x1b') {
                    ansi::visible_width(l)
                } else {
                    formats::expand_tabs(l).width()
                }
            })
            .max()
            .unwrap_or(0);
        self.widest.set(Some(w));
        w
    }

    /// Lines `range` (clamped) highlighted for the document's format, or as stored (escapes removed) when `raw`.
    pub fn lines(&self, raw: bool, range: Range<usize>) -> Vec<Line<'static>> {
        let end = range.end.min(self.line_count());
        (range.start.min(end)..end)
            .map(|i| {
                let l = self.line(i);
                if raw {
                    return Line::from(
                        ansi::parse_ansi(l)
                            .0
                            .into_iter()
                            .map(|s| s.content.to_string())
                            .collect::<String>(),
                    );
                }
                match &self.format {
                    Format::Log => ansi::log_line(l, self.sgr[i]).0,
                    Format::Diff => formats::diff_line(l),
                    Format::Toml => formats::toml_line(l),
                    Format::Yaml => formats::yaml_line(l),
                    Format::Json => formats::json_line(l),
                    Format::Code(lang) => formats::code_line(lang, l),
                    Format::Markdown | Format::Csv | Format::Plain => {
                        Line::from(formats::expand_tabs(l))
                    }
                }
            })
            .collect()
    }
}
