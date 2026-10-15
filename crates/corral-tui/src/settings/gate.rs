//! Gate suggestions and previews for the Review section. A project whose effective gate is empty lists the
//! presets `detect_gate_presets` finds in its repo; `⏎` or `⏎ use this gate` stages one as the project's own gate
//! list (edit it, then `s` saves through the usual path). The repo-owned script or justfile a project's gate runs
//! is shown read-only, expanded while one of the project's rows is selected; the form scroll (pgdn, wheel) reads
//! long files. Repos are read when the view opens, a project path changes or its gate changes; never per frame.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use corral_core::{Config, GatePreset, detect_gate_presets};
use corral_ops::Ctx;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::SettingsView;
use super::automation::{Key, List, Scope};
use super::fields::FormLine;
use super::model::{Field, Kind, Row};
use crate::{Action, Toast, Tone, theme};

/// Bytes of a gate script read for its preview (the preset scan uses the same cap).
const READ_LIMIT: u64 = 128 * 1024;
const TAB: &str = "    ";

pub(crate) struct ProjectGate {
    path: PathBuf,
    presets: Result<Vec<GatePreset>, String>,
    /// The effective gate the preview was read for.
    shown_for: Option<Vec<String>>,
    /// (title, text) of the repo file the effective gate runs.
    preview: Option<(String, String)>,
}

pub(crate) type Gates = BTreeMap<String, ProjectGate>;

fn repo_path(root: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(p)
    }
}

/// Rescans projects whose path changed and rereads previews whose effective gate changed.
pub(crate) fn sync(gates: &mut Gates, cfg: &Config, root: &Path) {
    gates.retain(|k, _| cfg.projects.contains_key(k));
    for (k, p) in &cfg.projects {
        let path = repo_path(root, &p.path);
        if gates.get(k).is_none_or(|g| g.path != path) {
            let presets = detect_gate_presets(&path).map_err(|e| format!("{e:#}"));
            gates.insert(
                k.clone(),
                ProjectGate {
                    path,
                    presets,
                    shown_for: None,
                    preview: None,
                },
            );
        }
        let Some(g) = gates.get_mut(k) else {
            continue;
        };
        let gate = cfg.review_policy(Some(k), None).gate;
        if g.shown_for.as_ref() != Some(&gate) {
            g.preview = script_preview(&g.path, &gate, g.presets.as_ref().ok());
            g.shown_for = Some(gate);
        }
    }
}

/// A regular file's text (symlinks and devices are not followed), capped at `READ_LIMIT`; a read failure is
/// reported in place of the content.
fn read_file(path: &Path) -> Option<String> {
    if !path.symlink_metadata().is_ok_and(|m| m.is_file()) {
        return None;
    }
    let mut bytes = Vec::new();
    let read = File::open(path).and_then(|f| f.take(READ_LIMIT + 1).read_to_end(&mut bytes));
    if let Err(e) = read {
        return Some(format!("✗ could not read {}: {e}", path.display()));
    }
    let cut = bytes.len() as u64 > READ_LIMIT;
    bytes.truncate(READ_LIMIT as usize);
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(if cut {
        format!("{text}\n[preview truncated]")
    } else {
        text
    })
}

/// The repo file a configured gate runs: a `just` recipe's justfile, or a script named by a relative path
/// argument (`bash scripts/gate.sh`, `./check.sh`); else the matching detected preset's preview.
fn script_preview(
    repo: &Path,
    gate: &[String],
    presets: Option<&Vec<GatePreset>>,
) -> Option<(String, String)> {
    for cmd in gate {
        let words: Vec<&str> = cmd.split_whitespace().collect();
        if words.first() == Some(&"just") {
            for name in ["justfile", "Justfile", ".justfile"] {
                if let Some(text) = read_file(&repo.join(name)) {
                    return Some((format!("{name} · {cmd}"), text));
                }
            }
        }
        let paths = words.iter().filter(|w| {
            !w.starts_with('-')
                && !w.contains("..")
                && !Path::new(w).is_absolute()
                && (w.contains('/') || w.contains('.'))
        });
        for w in paths {
            if let Some(text) = read_file(&repo.join(w)) {
                let title = if *w == cmd {
                    w.to_string()
                } else {
                    format!("{w} · {cmd}")
                };
                return Some((title, text));
            }
        }
    }
    let preset = presets?.iter().find(|p| p.commands == gate)?;
    Some((preset.name.clone(), preset.preview.clone()?))
}

/// Rows for a project with no effective gate: one stageable row per detected preset, or why there is none.
pub(super) fn preset_rows(scope: &Scope, g: Option<&ProjectGate>) -> Vec<Row> {
    let f = |key: Key| Field::Review(scope.clone(), key);
    let info = |value: String| {
        vec![Row::new(
            f(Key::Detected),
            "Detected gates",
            Kind::Info,
            value,
        )]
    };
    match g.map(|g| &g.presets) {
        None => Vec::new(),
        Some(Err(e)) => info(format!("✗ could not inspect the repo: {e}")),
        Some(Ok(p)) if p.is_empty() => {
            info("none found (scripts/gate.sh, justfile, Cargo.toml, package.json, …)".into())
        }
        Some(Ok(presets)) => presets
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let row = Row::new(
                    f(Key::Preset(i)),
                    p.name.clone(),
                    Kind::Stage(p.commands.join("\n")),
                    p.commands.join(" · "),
                );
                if i == 0 {
                    row.help("Detected in this repo, nothing applied yet. ⏎ stages one as this project's gate; edit it, then s saves.")
                } else {
                    row
                }
            })
            .collect(),
    }
}

fn styled(text: String, style: Style) -> FormLine {
    FormLine::plain(Line::from(Span::styled(text, style)))
}

impl SettingsView {
    /// Stages a preset's commands as the project's own gate list and selects its first command for editing.
    pub(super) fn stage(&mut self, ctx: &Ctx, row: &Row, commands: &str) -> Action {
        if let Err(e) = self.set(ctx, &row.field, commands) {
            return Action::Toast(Toast::new(
                Tone::Bad,
                format!("{} not staged", row.label),
                e,
            ));
        }
        if let Field::Review(scope, _) = &row.field {
            self.select_field(&Field::Review(scope.clone(), Key::Item(List::Gate, 0)));
        }
        Action::Toast(Toast::new(
            Tone::Good,
            format!("{} staged", row.label),
            "⏎ edits a command; s saves",
        ))
    }

    /// The read-only preview drawn above `row` when it is a project's Reviewer row (right below its gate rows):
    /// the file the configured gate runs, or the selected (else first) detected preset's.
    pub(super) fn gate_preview(&self, row: &Row, selected: Option<&Row>) -> Vec<FormLine> {
        let Field::Review(Scope::Project(k), Key::Reviewer) = &row.field else {
            return Vec::new();
        };
        let Some(g) = self.gates.get(k) else {
            return Vec::new();
        };
        let focused = selected.map(|r| &r.field);
        let focused = matches!(focused, Some(Field::Review(Scope::Project(p), _)) if p == k);
        let configured = g.shown_for.as_ref().is_some_and(|c| !c.is_empty());
        let shown = if configured {
            g.preview
                .as_ref()
                .map(|(title, text)| (title.clone(), Some(text.as_str())))
        } else {
            let presets = g.presets.as_ref().map(Vec::as_slice).unwrap_or_default();
            let at = match focused.then(|| selected.map(|r| &r.field)).flatten() {
                Some(Field::Review(_, Key::Preset(i))) => Some(*i),
                _ => presets.iter().position(|p| p.preview.is_some()),
            };
            at.and_then(|i| presets.get(i))
                .map(|p| (format!("{} preview", p.name), p.preview.as_deref()))
        };
        let Some((title, text)) = shown else {
            return Vec::new();
        };
        let Some(text) = text else {
            return vec![styled(
                format!("{TAB}┄ {title}: its commands run directly; no repo file to show"),
                theme::dim(),
            )];
        };
        let n = text.lines().count();
        if !focused {
            return vec![styled(
                format!("{TAB}┄ {title} · {n} lines · select a row of {k} to read it"),
                theme::dim(),
            )];
        }
        let mut out = vec![FormLine::plain(Line::from(vec![
            Span::styled(format!("{TAB}┌ "), theme::dim()),
            Span::styled(
                title,
                Style::new().fg(theme::CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" · read-only · {n} lines"), theme::dim()),
        ]))];
        for line in text.lines() {
            let clean: String = line
                .replace('\t', TAB)
                .chars()
                .map(|c| if c.is_control() { '·' } else { c })
                .collect();
            out.push(FormLine::plain(Line::from(vec![
                Span::styled(format!("{TAB}│ "), theme::dim()),
                Span::styled(clean, Style::new().fg(theme::FG)),
            ])));
        }
        out.push(styled(
            format!("{TAB}└ end of {n} lines · pgup/pgdn or the wheel scroll"),
            theme::dim(),
        ));
        out
    }
}
