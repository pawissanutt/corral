//! Configuration provenance and the explicit write-layer controls.

use corral_core::{Config, ConfigErrors, ConfigLayer, ConfigLayers, ConfigSource, status_key};
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::SettingsView;
use super::automation::{Key, List, Scope};
use super::fields::{FormLine, Target};
use super::model::{self, Field, Row, SECTIONS, Section};
use crate::{Toast, Tone, theme};

/// Quote dynamic table names, including names containing dots or quotes.
fn quoted(name: &str) -> String {
    toml_edit::Key::new(name).to_string()
}

/// Validation prefixes are not always TOML paths (notably Review), so resolve
/// provenance from field identity rather than parsing validation messages.
pub(super) fn key(row: &Row, cfg: &Config) -> Option<String> {
    let path = match &row.field {
        Field::Project(n) => format!("projects.{}", quoted(n)),
        Field::ProjectPath(n) => format!("projects.{}.path", quoted(n)),
        Field::ProjectBase(n) => format!("projects.{}.base", quoted(n)),
        Field::ProjectProfile(n) => format!("projects.{}.default_profile", quoted(n)),
        Field::ProjectCap(n) => format!("limits.per_project.{}", quoted(n)),
        Field::Profile(n) => format!("profiles.{}", quoted(n)),
        Field::ProfileHarness(n) => format!("profiles.{}.harness", quoted(n)),
        Field::ProfileModel(n) => format!("profiles.{}.model", quoted(n)),
        Field::ProfileEffort(n) => format!("profiles.{}.effort", quoted(n)),
        Field::ProfileArgs(n) => format!("profiles.{}.args", quoted(n)),
        Field::Harness(n) | Field::HarnessCommand(n) | Field::HarnessFlags(n) => {
            if row.label == "Notes" {
                return None;
            }
            let table = if cfg.harnesses.contains_key(n) {
                "harnesses"
            } else {
                "agents"
            };
            let suffix = match row.field {
                Field::HarnessCommand(_) => ".command",
                Field::HarnessFlags(_) => ".flags",
                _ => "",
            };
            format!("{table}.{}{suffix}", quoted(n))
        }
        Field::Autopilot => "review.autopilot".into(),
        Field::Review(scope, key) => {
            let prefix = match scope {
                Scope::Global => "review".into(),
                Scope::Project(n) => format!("projects.{}.review", quoted(n)),
            };
            let suffix = match key {
                Key::Heading => return Some(prefix),
                Key::Mode(l) | Key::Item(l, _) | Key::Add(l) => match l {
                    List::Gate => "gate",
                    List::OwnerPaths => "owner_paths",
                },
                Key::Reviewer => "reviewer",
                Key::Accept => "accept",
                Key::MaxRounds => "max_rounds",
                Key::Merge => "merge",
                Key::MaxDiff => "max_diff_lines",
                Key::Env => "env",
                Key::SizeLimits => "size_limits",
                Key::Detected | Key::Preset(_) => return None,
            };
            format!("{prefix}.{suffix}")
        }
        Field::CardField(_) => "board.card.fields".into(),
        Field::CardColumnCost => "board.card.column_cost".into(),
        Field::CardColumn(s) => format!("board.card.columns.{}", quoted(status_key(*s))),
        Field::SlotLimit
        | Field::DefaultProfile
        | Field::PmAgent
        | Field::PmPushEvents
        | Field::PmPushKinds
        | Field::HerdrSession
        | Field::Mouse
        | Field::Preference(_)
        | Field::KeyBinding(_)
        | Field::Glob(_)
        | Field::MaxBytes
        | Field::CardDensity
        | Field::CardFields
        | Field::CardCostWarn => row.key.clone(),
        Field::Operating(_)
        | Field::AddProject
        | Field::AddGlob
        | Field::AddProfile
        | Field::CardPreview(_) => {
            return None;
        }
    };
    Some(path)
}

pub(super) fn annotate(
    lines: &mut Vec<FormLine>,
    row: &Row,
    cfg: &Config,
    layers: Result<&ConfigLayers, &anyhow::Error>,
    staged: bool,
    target: ConfigLayer,
) {
    let Some(key) = key(row, cfg) else {
        return;
    };
    let source = layers
        .map_err(|_| ())
        .and_then(|l| l.source(&key).map_err(|_| ()));
    let mut label = match source {
        Ok(source) => format!("    source: {source}"),
        Err(()) => "    source: unavailable (cannot read configuration layers)".into(),
    };
    if staged {
        label.push_str(&format!(" · staged → {target}"));
    }
    if row.help.is_empty() {
        let mut line = FormLine::plain(Line::styled(label, theme::dim()));
        line.row = lines.first().and_then(|line| line.row);
        lines.push(line);
    } else if let Some(help) = lines.iter_mut().find(|line| {
        line.line
            .spans
            .iter()
            .any(|span| span.content.contains(row.help))
    }) {
        help.line
            .spans
            .insert(0, Span::styled(format!("{label} · "), theme::dim()));
    }
}

impl SettingsView {
    pub(super) fn saved_rows(&self) -> Vec<Row> {
        match &self.open {
            Some(p) if self.in_section(Section::Profiles) => {
                super::catalog::editor_rows(&self.base, p, self.report.as_ref())
            }
            _ => model::rows(
                SECTIONS[self.section],
                &self.base,
                self.herdr_env.as_deref(),
                self.report.as_ref(),
                &self.gates,
            ),
        }
    }

    pub(super) fn row_staged(&self, row: &Row, saved: &[Row]) -> bool {
        match &row.field {
            Field::Profile(n) => self.base.profiles.get(n) != self.draft.profiles.get(n),
            Field::Project(n) => self.base.projects.get(n) != self.draft.projects.get(n),
            Field::Harness(n) => {
                self.base.harnesses.get(n) != self.draft.harnesses.get(n)
                    || self.base.agents.get(n) != self.draft.agents.get(n)
            }
            _ => saved
                .iter()
                .find(|saved| saved.field == row.field)
                .is_none_or(|saved| saved.value != row.value),
        }
    }

    /// Apply only the semantic delta to the current raw layer document. The
    /// store enforces personal-key restrictions before writing either file.
    pub(super) fn save(&mut self, ctx: &Ctx) -> Result<Toast, Toast> {
        if !self.commit_edit(ctx) {
            return Err(Toast::new(
                Tone::Warn,
                "Not saved",
                "fix the field being edited",
            ));
        }
        if self.save_target == ConfigLayer::User {
            let mut personal = self.base.clone();
            personal.profiles = self.draft.profiles.clone();
            personal.harnesses = self.draft.harnesses.clone();
            personal.ui = self.draft.ui.clone();
            personal.defaults = self.draft.defaults.clone();
            if personal != self.draft {
                return Err(Toast::new(
                    Tone::Bad,
                    "Not saved",
                    "User target allows only profiles, harnesses, ui and defaults. Nonpersonal edits require workspace (w). Draft kept.",
                ));
            }
        }
        self.revalidate(ctx);
        if !self.errors.is_empty() {
            return Err(Toast::new(
                Tone::Warn,
                "Not saved",
                format!("{} problems; see the marked fields", self.errors.len()),
            ));
        }
        let result =
            ctx.store
                .edit_config_layer_from(self.save_target, ConfigSource::SettingsUi, |doc| {
                    doc.apply_config_changes(&self.base, &self.draft)
                });
        match result {
            Ok(cfg) => {
                let masked = cfg != self.draft;
                self.base = cfg.clone();
                self.draft = cfg;
                self.revalidate(ctx);
                Ok(Toast::new(
                    Tone::Good,
                    "Settings saved",
                    format!(
                        "Saved to {}.{} Effective values and source labels are reloaded.",
                        self.save_target,
                        if masked {
                            " Other layers or concurrent edits affect the displayed values."
                        } else {
                            ""
                        },
                    ),
                ))
            }
            Err(e) => {
                if let Some(errs) = e.chain().find_map(|c| c.downcast_ref::<ConfigErrors>()) {
                    self.errors = errs.0.clone();
                }
                Err(Toast::new(
                    Tone::Bad,
                    "Not saved",
                    format!(
                        "{} target: {e:#}. Draft kept; select workspace (w) to save nonpersonal edits.",
                        self.save_target
                    ),
                ))
            }
        }
    }

    pub(super) fn draw_save_target(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let choices = [
            (" w workspace ", ConfigLayer::Workspace),
            (" u user ", ConfigLayer::User),
        ];
        let mut x = area.x;
        for (label, layer) in choices {
            let width = u16::try_from(label.len())
                .unwrap_or(u16::MAX)
                .min(area.right().saturating_sub(x));
            let rect = Rect::new(x, area.y, width, area.height.min(1));
            let text = if self.save_target == layer {
                format!("[{}]", label.trim())
            } else {
                label.into()
            };
            let style = if self.save_target == layer {
                theme::bold().fg(theme::CYAN).bg(theme::SELECTED_BG)
            } else {
                theme::dim()
            };
            f.render_widget(Paragraph::new(text).style(style), rect);
            self.regions.push((rect, Target::SaveLayer(layer)));
            x = x.saturating_add(width);
        }
        let rect = Rect::new(
            x,
            area.y,
            area.right().saturating_sub(x).min(10),
            area.height.min(1),
        );
        f.render_widget(
            Paragraph::new(" s save ").style(Style::new().fg(theme::CYAN)),
            rect,
        );
        self.regions.push((rect, Target::Save));
        if area.height > 1 {
            let target = ctx
                .store
                .config_layers()
                .and_then(|l| l.path(self.save_target));
            let path = match target {
                Ok(path) => path.display().to_string(),
                Err(e) => format!("unavailable: {e:#}"),
            };
            f.render_widget(
                Paragraph::new(format!("Save target: {} · {path}", self.save_target))
                    .style(theme::dim()),
                Rect::new(area.x, area.y + 1, area.width, 1),
            );
        }
        if area.height > 2 {
            f.render_widget(
                Paragraph::new("User: profiles / harnesses / ui / defaults only; other edits require workspace.").style(theme::dim()),
                Rect::new(area.x, area.y + 2, area.width, 1),
            );
        }
    }
}
