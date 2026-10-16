//! Pure renderer: artifact text -> styled ratatui lines for reading (Markdown, JSON, TOML, YAML, diffs,
//! logs, CSV, code), plus the rendered/raw preference remembered per artifact kind.

use std::cell::RefCell;
use std::collections::HashSet;

use corral_core::ArtifactKind;
use ratatui::text::Line;

mod ansi;
mod formats;
mod lazy;
mod markdown;
pub(crate) mod wrap;

#[cfg(test)]
mod tests;

pub use ansi::{to_ansi, to_plain};
pub(crate) use formats::{diff_lines, expand_tabs};
pub use lazy::Document;

/// How an artifact's text is formatted for reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Json,
    Toml,
    Yaml,
    Diff,
    Log,
    Csv,
    Code(String),
    Plain,
}

impl Format {
    pub fn label(&self) -> &str {
        match self {
            Format::Markdown => "markdown",
            Format::Json => "json",
            Format::Toml => "toml",
            Format::Yaml => "yaml",
            Format::Diff => "diff",
            Format::Log => "log",
            Format::Csv => "csv",
            Format::Code(l) => l,
            Format::Plain => "text",
        }
    }
}

/// Picks the format from the file name's extension, falling back to the artifact kind (reports, summaries
/// and plans without a known extension read as Markdown).
pub fn detect(name: &str, kind: ArtifactKind) -> Format {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" => Format::Markdown,
        "json" => Format::Json,
        "toml" => Format::Toml,
        "yaml" | "yml" => Format::Yaml,
        "diff" | "patch" => Format::Diff,
        "log" => Format::Log,
        "csv" => Format::Csv,
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "sh" | "bash" | "rb" | "c" | "h"
        | "cpp" | "java" | "sql" | "lua" | "css" => Format::Code(ext),
        _ => match kind {
            ArtifactKind::Diff => Format::Diff,
            ArtifactKind::Log => Format::Log,
            ArtifactKind::Report | ArtifactKind::Summary | ArtifactKind::Plan => Format::Markdown,
            _ => Format::Plain,
        },
    }
}

/// Renders `text` for reading at `width` columns. Prose is wrapped to the width; code blocks, logs and
/// data formats keep their lines intact so the view scrolls them horizontally.
pub fn render(format: &Format, text: &str, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width);
    match format {
        Format::Markdown => markdown::render(text, width),
        Format::Json => formats::json_lines(text),
        Format::Toml => formats::render_lang("toml", text),
        Format::Yaml => formats::render_lang("yaml", text),
        Format::Diff => formats::diff_lines(text),
        Format::Log => ansi::log_lines(text),
        Format::Csv => formats::csv_table(text, width),
        Format::Code(lang) => formats::code_lines(lang, text),
        Format::Plain => formats::plain_lines(text),
    }
}

/// The text exactly as stored (tabs expanded, ANSI escapes removed so the terminal is not driven by it).
pub fn raw(text: &str) -> Vec<Line<'static>> {
    text.lines()
        .map(|l| {
            Line::from(
                ansi::parse_ansi(l)
                    .0
                    .into_iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>(),
            )
        })
        .collect()
}

/// Index of the first error/`FAILED`/panic line of a log, for the jump-to-error key.
pub fn first_error_line(text: &str) -> Option<usize> {
    ansi::first_error(text)
}

thread_local! {
    static RAW_KINDS: RefCell<HashSet<&'static str>> = RefCell::new(HashSet::new());
}

/// Whether previews of `kind` currently show raw text.
pub fn is_raw(kind: ArtifactKind) -> bool {
    RAW_KINDS.with(|r| r.borrow().contains(kind.as_str()))
}

/// Flips rendered/raw for every preview of `kind`; returns true when it is now raw.
pub fn toggle_raw(kind: ArtifactKind) -> bool {
    RAW_KINDS.with(|r| {
        let mut set = r.borrow_mut();
        let key = kind.as_str();
        !set.remove(key) && set.insert(key)
    })
}
