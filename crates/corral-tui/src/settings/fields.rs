//! Field editors for the settings form: text, number, toggle, single pick (pills) and string-list rows. Each
//! row renders to lines plus click targets (column offset, width, target) so mouse and keyboard share one model.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::model::{Kind, Row};
use crate::theme;

pub(crate) const LABEL_W: usize = 20;
const LIST_W: usize = 28;
const INDENT: &str = "    ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Section(usize),
    SectionList,
    Form,
    Row(usize),
    Pill(usize, usize),
    Up(usize),
    Down(usize),
    Remove(usize),
    /// Selects the row and activates it like `⏎` (the model row's `▾ choose`).
    Activate(usize),
    /// Model picker popup: one choice (index into its items), and the list area (wheel).
    PickerRow(usize),
    PickerList,
}

pub(crate) struct FormLine {
    pub line: Line<'static>,
    pub row: Option<usize>,
    pub targets: Vec<(u16, u16, Target)>,
}

impl FormLine {
    pub fn plain(line: Line<'static>) -> FormLine {
        FormLine {
            line,
            row: None,
            targets: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Builder {
    spans: Vec<Span<'static>>,
    x: u16,
    targets: Vec<(u16, u16, Target)>,
}

impl Builder {
    fn text(&mut self, s: impl Into<String>, style: Style) {
        let s = s.into();
        self.x = self.x.saturating_add(s.width() as u16);
        self.spans.push(Span::styled(s, style));
    }

    fn button(&mut self, s: impl Into<String>, style: Style, target: Target) {
        let x = self.x;
        self.text(s, style);
        self.targets.push((x, self.x - x, target));
    }

    fn finish(self, row: usize) -> FormLine {
        FormLine {
            line: Line::from(self.spans),
            row: Some(row),
            targets: self.targets,
        }
    }
}

pub(crate) struct Editing<'a> {
    pub text: &'a str,
    pub error: Option<&'a str>,
}

pub(crate) fn pill_labels(kind: &Kind) -> Vec<String> {
    match kind {
        Kind::Pick(opts) => opts
            .iter()
            .map(|o| o.clone().unwrap_or_else(|| "inherit".into()))
            .collect(),
        Kind::Toggle => vec!["on".into(), "off".into()],
        _ => Vec::new(),
    }
}

/// The value a pill stands for, as `set_value` text.
pub(crate) fn pill_value(kind: &Kind, j: usize) -> Option<String> {
    match kind {
        Kind::Pick(opts) => opts.get(j).map(|o| o.clone().unwrap_or_default()),
        Kind::Toggle => Some(if j == 0 { "on" } else { "off" }.into()),
        _ => None,
    }
}

pub(crate) fn pill_index(row: &Row) -> usize {
    match &row.kind {
        Kind::Pick(opts) => opts
            .iter()
            .position(|o| o.as_deref().unwrap_or("") == row.value)
            .unwrap_or(0),
        Kind::Toggle => usize::from(row.value != "on"),
        _ => 0,
    }
}

fn pill_style(on: bool, missing: bool) -> Style {
    match (on, missing) {
        (true, _) => Style::new()
            .fg(theme::CYAN)
            .bg(theme::SELECTED_BG)
            .add_modifier(Modifier::BOLD),
        (false, true) => theme::dim().add_modifier(Modifier::CROSSED_OUT),
        (false, false) => theme::dim(),
    }
}

fn editor(b: &mut Builder, e: &Editing) {
    b.text("› ", Style::new().fg(theme::CYAN));
    b.text(e.text.to_string(), Style::new().fg(theme::FG));
    b.text("▏", Style::new().fg(theme::CYAN));
}

/// Lines of row `i`: the value line, a help line, then one line per inline error.
pub(crate) fn row_lines(
    i: usize,
    row: &Row,
    selected: bool,
    editing: Option<Editing>,
    errors: &[&str],
    width: usize,
) -> Vec<FormLine> {
    let mut out = Vec::new();
    let mut b = Builder::default();
    let marker = if selected { "▸ " } else { "  " };
    b.text(marker, Style::new().fg(theme::CYAN));
    let label_style = if selected {
        theme::bold()
    } else {
        Style::new().fg(theme::FG)
    };
    match &row.kind {
        Kind::Group => {
            b.text(
                row.label.clone(),
                Style::new()
                    .fg(theme::project_color(&row.label))
                    .add_modifier(Modifier::BOLD),
            );
            b.text("   ", Style::new());
            if usize::from(b.x) + "× remove".width() > width {
                out.push(b.finish(i));
                b = Builder::default();
                b.text("  ", Style::new());
            }
            b.button("× remove", Style::new().fg(theme::RED), Target::Remove(i));
        }
        Kind::Entry => {
            b.text(row.label.clone(), label_style);
            b.text(format!("  {}", row.value), Style::new().fg(theme::FG));
        }
        Kind::Header => {
            b.text(
                row.label.clone(),
                Style::new().fg(theme::CYAN).add_modifier(Modifier::BOLD),
            );
            let tone = if row.value.starts_with('✗') {
                Style::new().fg(theme::RED)
            } else if row.value.starts_with('✓') || row.value.starts_with('★') {
                Style::new().fg(theme::GREEN)
            } else {
                theme::dim()
            };
            b.text(format!("   {}", row.value), tone);
        }
        Kind::Add => {
            b.text(row.label.clone(), Style::new().fg(theme::CYAN));
            if let Some(e) = &editing {
                b.text("  ", Style::new());
                editor(&mut b, e);
            }
        }
        kind => {
            b.text(format!("{:<LABEL_W$}", row.label), label_style);
            match (&editing, kind) {
                (Some(e), _) => editor(&mut b, e),
                (None, Kind::Pick(_) | Kind::Toggle) => {
                    let on = pill_index(row);
                    for (j, label) in pill_labels(kind).into_iter().enumerate() {
                        let text = if j == on {
                            format!("[{label}]")
                        } else {
                            format!(" {label} ")
                        };
                        if usize::from(b.x) + text.width() > width {
                            out.push(b.finish(i));
                            b = Builder::default();
                            b.text("  ", Style::new());
                        }
                        let missing = row.muted.contains(&label);
                        b.button(text, pill_style(j == on, missing), Target::Pill(i, j));
                        b.text(" ", Style::new());
                    }
                    if row.value.is_empty()
                        && let Some(inh) = &row.inherited
                    {
                        b.text(format!(" → {inh}"), Style::new().fg(theme::BLUE));
                    }
                }
                (None, _) if row.value.is_empty() => match &row.inherited {
                    Some(inh) => b.text(format!(" unset → {inh}"), Style::new().fg(theme::BLUE)),
                    None => b.text(" unset", theme::dim()),
                },
                (None, Kind::ListItem) => b.text(
                    format!(" {:<LIST_W$}", row.value),
                    Style::new().fg(theme::FG),
                ),
                (None, Kind::Info) => b.text(format!(" {}", row.value), theme::dim()),
                (None, _) => b.text(format!(" {}", row.value), Style::new().fg(theme::FG)),
            }
            if *kind == Kind::Picker && editing.is_none() {
                if usize::from(b.x) + "  ▾ choose".width() > width {
                    out.push(b.finish(i));
                    b = Builder::default();
                    b.text("  ", Style::new());
                }
                b.button(
                    "  ▾ choose",
                    Style::new().fg(theme::CYAN),
                    Target::Activate(i),
                );
            }
            if *kind == Kind::ListItem && editing.is_none() {
                if usize::from(b.x) + 12 > width {
                    out.push(b.finish(i));
                    b = Builder::default();
                    b.text("  ", Style::new());
                }
                b.text("   ", Style::new());
                b.button(" ↑ ", theme::dim(), Target::Up(i));
                b.button(" ↓ ", theme::dim(), Target::Down(i));
                b.button(" × ", Style::new().fg(theme::RED), Target::Remove(i));
            }
        }
    }
    out.push(b.finish(i));
    if !row.help.is_empty() {
        let mut h = Builder::default();
        h.text(format!("{INDENT}{}", row.help), theme::dim());
        out.push(h.finish(i));
    }
    let typed = editing.and_then(|e| e.error);
    for err in typed.into_iter().chain(errors.iter().copied()) {
        let mut l = Builder::default();
        l.text(format!("{INDENT}✗ {err}"), Style::new().fg(theme::RED));
        out.push(l.finish(i));
    }
    out
}
