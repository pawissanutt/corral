//! Profiles and Harnesses behavior of the settings screen: opening a profile's editor, new / duplicate / delete /
//! set-default, the model picker, harness re-detection, and the live dispatch preview.

use corral_ops::{Ctx, ModelSource, PREVIEW_TASK};
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::fields::{FormLine, LABEL_W, Target};
use super::model::{Field, SECTIONS, Section};
use super::picker::{Outcome, Picker};
use super::{Edit, SettingsView, catalog};
use crate::discovery::HarnessLoader;
use crate::{Action, Toast, Tone, theme};

impl SettingsView {
    pub(super) fn in_section(&self, s: Section) -> bool {
        SECTIONS[self.section] == s
    }

    /// Applies a finished background detection, and starts the first one when a section that needs it shows.
    pub(super) fn poll_report(&mut self, ctx: &Ctx) {
        if let Some(msg) = self.loader.as_mut().and_then(HarnessLoader::poll) {
            self.loader = None;
            match msg {
                Ok(r) => {
                    self.report = Some(r);
                    self.report_error = None;
                    if let Some(h) = self.picker.as_ref().map(|p| p.harness.clone()) {
                        let (models, note) = self.model_source(&h);
                        if let Some(p) = &mut self.picker {
                            p.update(models, note);
                        }
                    }
                }
                Err(e) => self.report_error = Some(e),
            }
        }
        let wants = self.in_section(Section::Profiles) || self.in_section(Section::Harnesses);
        if wants && self.report.is_none() && self.loader.is_none() && self.report_error.is_none() {
            self.loader = Some(HarnessLoader::start(ctx, false));
        }
    }

    fn redetect(&mut self, ctx: &Ctx) {
        self.report_error = None;
        self.loader = Some(HarnessLoader::start(ctx, true));
    }

    pub(super) fn open_profile(&mut self, name: String) {
        self.open = Some(name);
        self.sel = 1;
        self.scroll = 0;
        self.follow = true;
    }

    fn close_profile(&mut self) {
        if let Some(name) = self.open.take() {
            self.select_field(&Field::Profile(name));
        }
    }

    fn current_profile(&self) -> Option<String> {
        match self.current().map(|r| r.field) {
            Some(Field::Profile(n)) => Some(n),
            _ => self.open.clone(),
        }
    }

    fn start_new_profile(&mut self, from: Option<String>) {
        self.close_profile();
        let text = from
            .as_ref()
            .map(|f| format!("{f}-copy"))
            .unwrap_or_default();
        self.dup_from = from;
        self.select_field(&Field::AddProfile);
        self.start_edit(Field::AddProfile, text);
    }

    /// Creates the profile the "+ new profile" edit names and opens its editor. False keeps the edit open.
    pub(super) fn commit_new_profile(&mut self, ctx: &Ctx, mut e: Edit) -> bool {
        let template = self
            .dup_from
            .as_ref()
            .and_then(|f| self.draft.profiles.get(f).cloned())
            .unwrap_or_else(|| catalog::new_profile(&self.draft, self.report.as_ref()));
        let name = e.input.text.trim().to_string();
        match catalog::add_profile(&mut self.draft, &name, Some(template)) {
            Ok(()) => {
                self.dup_from = None;
                self.revalidate(ctx);
                self.open_profile(name);
                true
            }
            Err(err) => {
                e.error = Some(err);
                self.edit = Some(e);
                false
            }
        }
    }

    fn delete_profile(&mut self, ctx: &Ctx) -> Action {
        let Some(Field::Profile(name)) = self.current().map(|r| r.field) else {
            return Action::None;
        };
        let tasks = match ctx.store.list() {
            Ok(t) => t,
            Err(e) => return Action::Toast(Toast::new(Tone::Bad, "Not deleted", format!("{e:#}"))),
        };
        let refs = catalog::profile_refs(&self.draft, &tasks, &name);
        if !refs.is_empty() {
            return Action::Toast(Toast::new(
                Tone::Warn,
                format!("Profile {name} is in use"),
                format!("referenced by {}; change those first", refs.join(", ")),
            ));
        }
        self.draft.profiles.remove(&name);
        self.revalidate(ctx);
        self.select(self.sel);
        Action::None
    }

    fn set_default_profile(&mut self, ctx: &Ctx) -> Action {
        let Some(name) = self.current_profile() else {
            return Action::None;
        };
        self.draft.defaults.profile = Some(name);
        self.revalidate(ctx);
        Action::None
    }

    pub(super) fn open_picker(&mut self) {
        let Some(name) = self.open.clone() else {
            return;
        };
        let Some(p) = self.draft.profiles.get(&name) else {
            return;
        };
        let (models, note) = self.model_source(&p.harness);
        self.picker = Some(Picker::new(
            name,
            p.harness.clone(),
            models,
            note,
            p.model.clone(),
        ));
    }

    /// The picker's model list for `harness` and the note explaining where it came from.
    fn model_source(&self, harness: &str) -> (Vec<String>, String) {
        let info = self.report.as_ref().and_then(|r| r.get(harness));
        let models = info.map(|i| i.models.clone()).unwrap_or_default();
        let note = match info {
            None if self.loader.is_some() => "detecting models… type any model name".to_string(),
            None => "not detected; type any model name".to_string(),
            Some(i) if models.is_empty() => match i.notes.first() {
                Some(n) => format!("no model list ({n}); type any model name"),
                None => "no model list; type any model name".to_string(),
            },
            Some(i) => format!(
                "{} model{} ({})",
                models.len(),
                if models.len() == 1 { "" } else { "s" },
                if i.model_source == ModelSource::Builtin {
                    "built-in aliases"
                } else {
                    "listed by the harness"
                }
            ),
        };
        (models, note)
    }

    fn finish_picker(&mut self, ctx: &Ctx, out: Outcome) -> Action {
        match out {
            Outcome::Stay => Action::None,
            Outcome::Cancel => {
                self.picker = None;
                Action::None
            }
            Outcome::Pick(model) => {
                let Some(p) = self.picker.take() else {
                    return Action::None;
                };
                let field = Field::ProfileModel(p.profile);
                match self.set(ctx, &field, &model.unwrap_or_default()) {
                    Ok(()) => Action::None,
                    Err(e) => Action::Toast(Toast::new(Tone::Bad, "Model not changed", e)),
                }
            }
        }
    }

    pub(super) fn picker_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        let out = match &mut self.picker {
            Some(p) => p.key(&key),
            None => return Action::None,
        };
        self.finish_picker(ctx, out)
    }

    pub(super) fn picker_mouse(&mut self, ev: MouseEvent, ctx: &Ctx) -> Action {
        let target = self.target_at(ev.column, ev.row);
        let Some(p) = &mut self.picker else {
            return Action::None;
        };
        let on_list = matches!(target, Some(Target::PickerRow(_) | Target::PickerList));
        let out = match (ev.kind, target) {
            (MouseEventKind::Down(MouseButton::Left), Some(Target::PickerRow(i))) => p.click(i),
            (MouseEventKind::ScrollDown, _) if on_list => {
                p.wheel(true);
                Outcome::Stay
            }
            (MouseEventKind::ScrollUp, _) if on_list => {
                p.wheel(false);
                Outcome::Stay
            }
            _ => Outcome::Stay,
        };
        self.finish_picker(ctx, out)
    }

    /// Keys of the Profiles and Harnesses sections; `None` when the generic form handling applies.
    pub(super) fn catalog_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Option<Action> {
        let profiles = self.in_section(Section::Profiles);
        if !profiles && !self.in_section(Section::Harnesses) {
            return None;
        }
        match key.code {
            KeyCode::Char('r') => self.redetect(ctx),
            KeyCode::Esc if self.open.is_some() => self.close_profile(),
            KeyCode::Char('n' | 'a') if profiles => self.start_new_profile(None),
            KeyCode::Char('y') if profiles => {
                let from = self.current_profile()?;
                self.start_new_profile(Some(from));
            }
            KeyCode::Char('x') | KeyCode::Delete if profiles && self.open.is_none() => {
                return Some(self.delete_profile(ctx));
            }
            KeyCode::Char('*') if profiles => return Some(self.set_default_profile(ctx)),
            _ => return None,
        }
        Some(Action::None)
    }

    pub(super) fn catalog_hints(&self) -> Vec<(&'static str, &'static str)> {
        let mut h = Vec::new();
        if self.in_section(Section::Profiles) {
            if self.open.is_none() {
                h.extend([("n", "new"), ("y", "duplicate"), ("x", "delete")]);
            }
            h.push(("*", "set default"));
        }
        h.push(("r", "re-detect"));
        h
    }

    /// Lines above the rows: the Profiles list's column titles, or the Harnesses detection state.
    pub(super) fn catalog_head(&self) -> Vec<FormLine> {
        let line = |text: String, style: Style| FormLine::plain(Line::styled(text, style));
        if self.in_section(Section::Profiles) && self.open.is_none() {
            return vec![line(
                format!("  {}", catalog::profile_header(&self.draft)),
                theme::dim().add_modifier(Modifier::UNDERLINED),
            )];
        }
        if !self.in_section(Section::Harnesses) {
            return Vec::new();
        }
        let state = match (&self.report_error, &self.report, &self.loader) {
            (Some(e), _, _) => line(
                format!("  ✗ detection failed: {e}"),
                Style::new().fg(theme::RED),
            ),
            (None, _, Some(_)) => line("  … detecting harnesses".into(), theme::dim()),
            (None, Some(r), None) => line(
                format!(
                    "  detected {} · r re-detects",
                    r.detected_at.strftime("%Y-%m-%d %H:%M UTC")
                ),
                theme::dim(),
            ),
            (None, None, None) => line("  not detected yet · r detects".into(), theme::dim()),
        };
        vec![state, FormLine::plain(Line::from(""))]
    }

    /// The command a dispatch of the open profile would send, for the sample task, wrapped to `width`.
    pub(super) fn preview_lines(&self, ctx: &Ctx, width: usize) -> Vec<FormLine> {
        let Some(name) = self
            .open
            .as_ref()
            .filter(|n| self.draft.profiles.contains_key(*n))
        else {
            return Vec::new();
        };
        let mut out = vec![
            FormLine::plain(Line::from("")),
            FormLine::plain(Line::from(vec![
                Span::styled(format!("  {:<LABEL_W$}", "Preview"), theme::bold()),
                Span::styled(
                    format!(" what dispatch sends to the pane, for sample task {PREVIEW_TASK}"),
                    theme::dim(),
                ),
            ])),
        ];
        match corral_ops::preview_command(ctx, &self.draft, name) {
            Ok(cmd) => {
                let chars: Vec<char> = format!("$ {cmd}").chars().collect();
                for chunk in chars.chunks(width.saturating_sub(6).max(20)) {
                    out.push(FormLine::plain(Line::styled(
                        format!("    {}", chunk.iter().collect::<String>()),
                        Style::new().fg(theme::GREEN),
                    )));
                }
            }
            Err(e) => out.push(FormLine::plain(Line::styled(
                format!("    ✗ cannot render: {e:#}"),
                Style::new().fg(theme::RED),
            ))),
        }
        out
    }
}
