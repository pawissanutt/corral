//! Log rendering (ANSI colours kept, error/warning lines highlighted) and styled lines -> ANSI text.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::formats::expand_tabs;
use crate::theme::{FG, GREEN, RED, YELLOW};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineClass {
    Error,
    Warn,
    Pass,
    Plain,
}

/// Identifier-sized words of a line: `rejects_error` is one word, `error[E0308]` yields `error`.
fn words(text: &str) -> Vec<&str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect()
}

/// A Rust or pytest record of a test that passed: its name may contain `error` without being one.
fn passing_record(low: &str) -> bool {
    let t = low.trim();
    (t.starts_with("test ") && (t.ends_with("... ok") || t.contains("... ignored")))
        || (t.contains("::") && words(t).contains(&"passed"))
}

/// Classifies a log line by its text (ANSI already stripped). Counts are whole numeric words: `0 failed` is
/// clean, `10 failed` and `20 failed` are failures.
pub(crate) fn classify(plain: &str) -> LineClass {
    let low = plain.to_ascii_lowercase();
    if low.trim_start().starts_with("test result") {
        return if low.contains("test result: ok") {
            LineClass::Pass
        } else {
            LineClass::Error
        };
    }
    if passing_record(&low) {
        return LineClass::Plain;
    }
    let ws = words(&low);
    let counted_zero =
        |i: usize| i.checked_sub(1).and_then(|p| ws[p].parse::<u64>().ok()) == Some(0);
    let mut warn = false;
    for (i, w) in ws.iter().enumerate() {
        match *w {
            "failed" | "failure" | "failures" | "error" | "errors" if !counted_zero(i) => {
                return LineClass::Error;
            }
            "panicked" | "fatal" | "traceback" | "exception" => return LineClass::Error,
            "warning" | "warn" | "warnings" if !counted_zero(i) => warn = true,
            _ => {}
        }
    }
    if warn {
        LineClass::Warn
    } else {
        LineClass::Plain
    }
}

fn sgr_color(n: u16) -> Color {
    Color::Indexed(n as u8)
}

fn apply_sgr(style: Style, params: &str) -> Style {
    let codes: Vec<u16> = params
        .split([';', ':'])
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    let mut s = style;
    let mut i = 0;
    while i < codes.len() {
        let c = codes[i];
        s = match c {
            0 => Style::new(),
            1 => s.add_modifier(Modifier::BOLD),
            2 => s.add_modifier(Modifier::DIM),
            3 => s.add_modifier(Modifier::ITALIC),
            4 => s.add_modifier(Modifier::UNDERLINED),
            7 => s.add_modifier(Modifier::REVERSED),
            22 => s.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => s.remove_modifier(Modifier::ITALIC),
            24 => s.remove_modifier(Modifier::UNDERLINED),
            27 => s.remove_modifier(Modifier::REVERSED),
            30..=37 => s.fg(sgr_color(c - 30)),
            90..=97 => s.fg(sgr_color(c - 90 + 8)),
            39 => Style { fg: None, ..s },
            40..=47 => s.bg(sgr_color(c - 40)),
            100..=107 => s.bg(sgr_color(c - 100 + 8)),
            49 => Style { bg: None, ..s },
            38 | 48 => {
                let color = match (codes.get(i + 1), codes.get(i + 2)) {
                    (Some(5), Some(&n)) => {
                        i += 2;
                        Some(Color::Indexed(n as u8))
                    }
                    (Some(2), Some(&r)) => {
                        let g = codes.get(i + 3).copied().unwrap_or(0);
                        let b = codes.get(i + 4).copied().unwrap_or(0);
                        i += 4;
                        Some(Color::Rgb(r as u8, g as u8, b as u8))
                    }
                    _ => None,
                };
                match color {
                    Some(col) if c == 38 => s.fg(col),
                    Some(col) => s.bg(col),
                    None => s,
                }
            }
            _ => s,
        };
        i += 1;
    }
    s
}

/// Parses one line with SGR escapes into spans; other escape sequences are dropped. Returns the spans and
/// whether any colour was set. Text before a carriage return (progress redraws) is discarded.
pub(crate) fn parse_ansi_from(line: &str, start: Style) -> (Vec<Span<'static>>, bool, Style) {
    let line = expand_tabs(line.rsplit('\r').next().unwrap_or(line));
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut style = start;
    let mut colored = start.fg != Some(FG) || start.bg.is_some();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            if !c.is_control() {
                buf.push(c);
            }
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                let mut params = String::new();
                let mut fin = ' ';
                for p in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&p) {
                        fin = p;
                        break;
                    }
                    params.push(p);
                }
                if fin == 'm' {
                    if !buf.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut buf), style));
                    }
                    style = apply_sgr(style, &params);
                    if style.fg.is_none() {
                        style = style.fg(FG);
                    }
                    colored |= style.fg != Some(FG) || style.bg.is_some();
                }
            }
            Some(']') => {
                while let Some(p) = chars.next() {
                    if p == '\u{7}' || (p == '\x1b' && chars.next() == Some('\\')) {
                        break;
                    }
                }
            }
            _ => {
                chars.next();
            }
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, style));
    }
    (spans, colored, style)
}

pub(crate) fn base_style() -> Style {
    Style::new().fg(FG)
}

pub(crate) fn parse_ansi(line: &str) -> (Vec<Span<'static>>, bool) {
    let (spans, colored, _) = parse_ansi_from(line, base_style());
    (spans, colored)
}

fn plain_text(spans: &[Span<'static>]) -> String {
    spans.iter().map(|s| s.content.as_ref()).collect()
}

/// One styled line plus the SGR state it leaves behind: ANSI colours persist across line breaks, error,
/// warning and `test result` lines without their own colours are highlighted.
pub(crate) fn log_line(text: &str, start: Style) -> (Line<'static>, Style) {
    let (spans, colored, end) = parse_ansi_from(text, start);
    if colored {
        return (Line::from(spans), end);
    }
    let color = match classify(&plain_text(&spans)) {
        LineClass::Error => RED,
        LineClass::Warn => YELLOW,
        LineClass::Pass => GREEN,
        LineClass::Plain => return (Line::from(spans), end),
    };
    (
        Line::from(vec![Span::styled(
            plain_text(&spans),
            Style::new().fg(color),
        )]),
        end,
    )
}

/// Every log line, carrying ANSI colour state across lines.
pub(crate) fn log_lines(text: &str) -> Vec<Line<'static>> {
    let mut style = base_style();
    text.lines()
        .map(|l| {
            let (line, end) = log_line(l, style);
            style = end;
            line
        })
        .collect()
}

/// SGR state at the start of every line (for rendering any line window of a log without its predecessors).
pub(crate) fn style_starts(text: &str) -> Vec<Style> {
    let mut style = base_style();
    text.lines()
        .map(|l| {
            let start = style;
            style = parse_ansi_from(l, style).2;
            start
        })
        .collect()
}

/// Display width of a log line once escapes are removed.
pub(crate) fn visible_width(line: &str) -> usize {
    use unicode_width::UnicodeWidthStr;
    plain_text(&parse_ansi_from(line, base_style()).0).width()
}

/// Index of the first error line of a log (the jump-to-error target).
pub(crate) fn first_error(text: &str) -> Option<usize> {
    text.lines()
        .position(|l| classify(&plain_text(&parse_ansi(l).0)) == LineClass::Error)
}

fn sgr_for(style: Style) -> String {
    let mut codes: Vec<String> = vec!["0".into()];
    let mods = [
        (Modifier::BOLD, "1"),
        (Modifier::DIM, "2"),
        (Modifier::ITALIC, "3"),
        (Modifier::UNDERLINED, "4"),
        (Modifier::REVERSED, "7"),
        (Modifier::CROSSED_OUT, "9"),
    ];
    for (m, c) in mods {
        if style.add_modifier.contains(m) {
            codes.push(c.into());
        }
    }
    for (color, base) in [(style.fg, 38), (style.bg, 48)] {
        match color {
            Some(Color::Rgb(r, g, b)) => codes.push(format!("{base};2;{r};{g};{b}")),
            Some(Color::Indexed(n)) => codes.push(format!("{base};5;{n}")),
            _ => {}
        }
    }
    format!("\x1b[{}m", codes.join(";"))
}

/// Styled lines as text without escapes (piped output).
pub fn to_plain(lines: &[Line<'_>]) -> String {
    lines
        .iter()
        .map(|l| {
            format!(
                "{}\n",
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            )
        })
        .collect()
}

/// Styled lines as ANSI text for the terminal (`corral artifact show`).
pub fn to_ansi(lines: &[Line<'_>]) -> String {
    let mut out = String::new();
    for line in lines {
        for span in &line.spans {
            if span.style == Style::default() {
                out.push_str(&span.content);
            } else {
                out.push_str(&sgr_for(span.style));
                out.push_str(&span.content);
                out.push_str("\x1b[0m");
            }
        }
        out.push('\n');
    }
    out
}
