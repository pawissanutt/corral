//! SGR-only readback of a pane's visible screen; never execute terminal controls.
use std::collections::VecDeque;

use anyhow::Result;
use corral_protocol::detail::session::{TerminalColor, TerminalRun, TerminalSnapshot};

use super::{now, paging};

const MAX_ROWS: usize = corral_protocol::detail::TERMINAL_ROWS_MAX;
const ROW_BYTES: usize = 2048;
const TEXT_BUDGET: usize = 40 * 1024;
/// CONTROL and APP_CONTROL_MAX are both 60 KiB. Detail paging reserves 4 KiB
/// (56 KiB bodies), and reply validation reserves 2 KiB. Cap the combined JSON
/// arrays at 52 KiB: styled rows get 52 KiB minus the serialized plain lines
/// (at most 40 KiB), leaving another 4 KiB for body metadata/paging overhead.
/// This keeps both parallel arrays in one page. If rows do not fit in full,
/// omit all styling for this capture rather than separating rows from lines.
const ARRAYS_BUDGET: usize = corral_protocol::detail::TERMINAL_ARRAYS_MAX;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Style {
    fg: Option<TerminalColor>,
    bg: Option<TerminalColor>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    inverse: bool,
}

impl Style {
    fn run(self) -> TerminalRun {
        TerminalRun {
            text: String::new(),
            fg: self.fg,
            bg: self.bg,
            bold: self.bold,
            dim: self.dim,
            italic: self.italic,
            underline: self.underline,
            strike: self.strike,
            inverse: self.inverse,
        }
    }

    fn color(&mut self, foreground: bool, color: TerminalColor) {
        if foreground {
            self.fg = Some(color);
        } else {
            self.bg = Some(color);
        }
    }

    fn sgr(&mut self, text: &str) {
        let mut parameters = text.split(';').peekable();
        while let Some(parameter) = parameters.next() {
            if parameter.contains(':') {
                self.colon(parameter);
                continue;
            }
            let code = if parameter.is_empty() {
                Some(0)
            } else {
                parameter.parse::<u16>().ok()
            };
            match code {
                Some(0) => *self = Self::default(),
                Some(1) => self.bold = true,
                Some(2) => self.dim = true,
                Some(3) => self.italic = true,
                Some(4 | 21) => self.underline = true,
                Some(7) => self.inverse = true,
                Some(9) => self.strike = true,
                Some(22) => {
                    self.bold = false;
                    self.dim = false;
                }
                Some(23) => self.italic = false,
                Some(24) => self.underline = false,
                Some(27) => self.inverse = false,
                Some(29) => self.strike = false,
                Some(n @ 30..=37) => self.fg = Some(TerminalColor::Indexed((n - 30) as u8)),
                Some(n @ 90..=97) => self.fg = Some(TerminalColor::Indexed((n - 90 + 8) as u8)),
                Some(n @ 40..=47) => self.bg = Some(TerminalColor::Indexed((n - 40) as u8)),
                Some(n @ 100..=107) => self.bg = Some(TerminalColor::Indexed((n - 100 + 8) as u8)),
                Some(code @ (38 | 48)) => {
                    let color = match parameters.peek().copied() {
                        Some("5") => {
                            parameters.next();
                            byte(parameters.next()).map(TerminalColor::Indexed)
                        }
                        Some("2") => {
                            parameters.next();
                            rgb(&mut parameters)
                        }
                        _ => None,
                    };
                    if let Some(color) = color {
                        self.color(code == 38, color);
                    }
                }
                Some(39) => self.fg = None,
                Some(49) => self.bg = None,
                _ => {}
            }
        }
    }

    fn colon(&mut self, parameter: &str) {
        let mut parts = parameter.split(':');
        match parts.next() {
            Some("4") => {
                if let Some(kind) = parts.next().and_then(|value| value.parse::<u16>().ok()) {
                    self.underline = kind != 0;
                }
            }
            Some(code @ ("38" | "48")) => {
                let color = match parts.next() {
                    Some("5") => byte(parts.next()).map(TerminalColor::Indexed),
                    Some("2") => {
                        // ISO colon notation may include an empty colour-space ID.
                        let mut values = parts.peekable();
                        if values.peek() == Some(&"") {
                            values.next();
                        }
                        rgb(&mut values)
                    }
                    _ => None,
                };
                if let Some(color) = color {
                    self.color(code == "38", color);
                }
            }
            _ => {}
        }
    }
}

fn byte(value: Option<&str>) -> Option<u8> {
    value.and_then(|value| value.parse().ok())
}

fn rgb<'a>(values: &mut impl Iterator<Item = &'a str>) -> Option<TerminalColor> {
    // Consume the whole colour even when a component is invalid, so a later
    // component cannot accidentally become a bold/reset/etc. SGR parameter.
    let red = byte(values.next());
    let green = byte(values.next());
    let blue = byte(values.next());
    Some(TerminalColor::Rgb([red?, green?, blue?]))
}

#[derive(Default)]
struct Row {
    runs: Vec<TerminalRun>,
    last_style: Option<Style>,
    bytes: usize,
    full: bool,
}

impl Row {
    fn push(&mut self, ch: char, style: Style) {
        if self.full {
            return;
        }
        if self.bytes + ch.len_utf8() > ROW_BYTES {
            self.full = true;
            return;
        }
        self.bytes += ch.len_utf8();
        if self.last_style != Some(style) {
            self.runs.push(style.run());
            self.last_style = Some(style);
        }
        if let Some(run) = self.runs.last_mut() {
            run.text.push(ch);
        }
    }

    fn finish(mut self) -> Vec<TerminalRun> {
        while let Some(run) = self.runs.last_mut() {
            if run.bg.is_some() || run.inverse || run.underline || run.strike {
                break;
            }
            run.text.truncate(run.text.trim_end().len());
            if !run.text.is_empty() {
                break;
            }
            self.runs.pop();
        }
        self.runs
    }
}

#[derive(Default)]
enum Escape {
    #[default]
    Text,
    Esc,
    Intermediate,
    Csi(usize),
    String,
    StringEsc,
}

#[derive(Default)]
struct Screen {
    rows: VecDeque<Vec<TerminalRun>>,
    row: Row,
    blank: usize,
}

impl Screen {
    fn newline(&mut self) {
        let row = std::mem::take(&mut self.row).finish();
        if row.is_empty() {
            // Delay blank rows so trailing empty rows do not displace screen text.
            self.blank = (self.blank + 1).min(MAX_ROWS);
            return;
        }
        for _ in 0..self.blank {
            self.append(Vec::new());
        }
        self.blank = 0;
        self.append(row);
    }

    fn append(&mut self, row: Vec<TerminalRun>) {
        if self.rows.len() == MAX_ROWS {
            self.rows.pop_front();
        }
        self.rows.push_back(row);
    }
}

pub(super) fn snapshot(text: &str) -> Result<TerminalSnapshot> {
    let mut screen = Screen::default();
    let mut style = Style::default();
    let mut escape = Escape::default();
    for (index, ch) in text.char_indices() {
        escape = match escape {
            Escape::Text => match ch {
                '\n' => {
                    screen.newline();
                    Escape::Text
                }
                '\x1b' => Escape::Esc,
                '\u{9b}' => Escape::Csi(index + ch.len_utf8()),
                '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => Escape::String,
                ch if ch.is_control() => Escape::Text,
                ch => {
                    screen.row.push(ch, style);
                    Escape::Text
                }
            },
            Escape::Esc => match ch {
                '[' => Escape::Csi(index + 1),
                ']' | 'P' | 'X' | '^' | '_' => Escape::String,
                '\x20'..='\x2f' => Escape::Intermediate,
                '\x1b' => Escape::Esc,
                _ => Escape::Text,
            },
            Escape::Intermediate => match ch {
                '\x20'..='\x2f' => Escape::Intermediate,
                '\x1b' => Escape::Esc,
                _ => Escape::Text,
            },
            Escape::Csi(start) => match ch {
                '\x40'..='\x7e' => {
                    if ch == 'm' {
                        style.sgr(&text[start..index]);
                    }
                    Escape::Text
                }
                '\x1b' => Escape::Esc,
                _ => Escape::Csi(start),
            },
            Escape::String => match ch {
                '\x07' | '\u{9c}' => Escape::Text,
                '\x1b' => Escape::StringEsc,
                _ => Escape::String,
            },
            Escape::StringEsc => match ch {
                '\\' | '\x07' | '\u{9c}' => Escape::Text,
                '\x1b' => Escape::StringEsc,
                _ => Escape::String,
            },
        };
    }
    screen.newline();
    let mut budget = TEXT_BUDGET - 2;
    let count = screen
        .rows
        .iter()
        .rev()
        .take_while(|row| {
            let cost = row
                .iter()
                .flat_map(|run| run.text.chars())
                .map(|ch| {
                    if matches!(ch, '"' | '\\') {
                        2
                    } else {
                        ch.len_utf8()
                    }
                })
                .sum::<usize>()
                + 3;
            if cost > budget {
                return false;
            }
            budget -= cost;
            true
        })
        .count();
    screen.rows.drain(..screen.rows.len() - count);
    let mut rows: Vec<_> = screen.rows.into_iter().collect();
    let lines: Vec<String> = rows
        .iter()
        .map(|row| {
            let mut line = String::with_capacity(row.iter().map(|run| run.text.len()).sum());
            for run in row {
                line.push_str(&run.text);
            }
            line
        })
        .collect();
    if paging::json_len(&rows)? > ARRAYS_BUDGET - paging::json_len(&lines)? {
        rows.clear();
    }
    Ok(TerminalSnapshot {
        lines,
        rows,
        captured_at: now(),
        live: true,
    })
}

#[cfg(test)]
#[path = "terminal_screen_tests.rs"]
mod tests;
