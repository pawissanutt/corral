//! Per-run profile choice shared by the dispatch sheet and the review retry sheet: profile pills (`h/l` or a
//! click), a model picker (`M` or a click on the model row) and effort pills (`e` cycles, or a click). Overrides
//! apply to one run only; the sheet then reads "profile (overridden)". Choosing a profile again drops them.

use corral_core::{Config, ConfigSource, FALLBACK_PROFILE, ProfileChoice, Run};
use corral_ops::{Ctx, HarnessReport};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use toml_edit::{Array, Table, value};

use crate::discovery::HarnessLoader;
use crate::hit::{self, Hit, HitMap};
use crate::overlays::{LineInput, centered, pills, popup, truncate};
use crate::{Action, MouseCtx, Toast, Tone, theme};

#[cfg(test)]
pub(crate) mod tests;

/// Mouse zones ([`Hit::Item`]): profile pills, effort pills, model picker rows.
pub(crate) const Z_PROFILE: u8 = 10;
pub(crate) const Z_EFFORT: u8 = 11;
pub(crate) const Z_MODEL: u8 = 12;

/// Lines [`RunChoice::draw`] uses: profile, model, effort.
pub(crate) const ROWS: u16 = 3;

const PICKER_ROWS: usize = 10;

/// "harness·model·effort" of a run, absent parts left out (the profile key stands in for a missing harness).
pub(crate) fn run_label(r: &Run) -> String {
    let mut parts = vec![r.harness.clone().unwrap_or_else(|| r.agent.clone())];
    parts.extend(r.model.clone());
    parts.extend(r.effort.clone());
    parts.join("·")
}

/// The profile key, plus "(harness·model·effort)" when that says more than the key alone.
pub(crate) fn run_title(r: &Run) -> String {
    let label = run_label(r);
    if label == r.agent {
        label
    } else {
        format!("{} ({label})", r.agent)
    }
}

/// What a profile gives a run before overrides, and what its harness accepts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProfileDefaults {
    harness: String,
    model: Option<String>,
    effort: Option<String>,
    takes_model: bool,
    takes_effort: bool,
    /// Effort levels of the harness (empty with `takes_effort` = any value).
    efforts: Vec<String>,
    /// Why the profile does not resolve; dispatch would fail with the same error.
    error: Option<String>,
}

fn normalize(effort: &str) -> String {
    effort.trim().to_ascii_lowercase()
}

fn profile_defaults(cfg: &Config, profile: &str) -> ProfileDefaults {
    let choice = ProfileChoice {
        profile: Some(profile.to_string()),
        ..ProfileChoice::default()
    };
    let error = cfg
        .resolve_profile(&choice, None, None)
        .err()
        .map(|e| format!("{e:#}"));
    let (harness, model, effort) = match cfg.profiles.get(profile) {
        Some(p) => (
            p.harness.clone(),
            p.model.clone(),
            p.effort.as_deref().map(normalize),
        ),
        None => (profile.to_string(), None, None),
    };
    let flags = cfg.harness(&harness).ok().map(|(_, f)| f);
    ProfileDefaults {
        takes_model: flags.as_ref().is_some_and(|f| f.model_flag.is_some()),
        takes_effort: flags.as_ref().is_some_and(|f| f.effort_flag.is_some()),
        efforts: flags.map(|f| f.efforts).unwrap_or_default(),
        harness,
        model,
        effort,
        error,
    }
}

fn valid_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PickRow {
    Default,
    Typed(String),
    Model(String),
}

/// The `M` popup: a filter line over the harness's discovered models; ⏎ picks the selected row.
#[derive(Debug, Clone, Default)]
pub(crate) struct ModelPicker {
    pub(crate) filter: LineInput,
    sel: usize,
    offset: usize,
}

/// Profile + per-run model/effort overrides. `model`/`effort` hold overrides only: a value equal to the
/// profile's own is stored as `None`.
pub(crate) struct RunChoice {
    pub(crate) profiles: Vec<String>,
    pub(crate) idx: usize,
    /// New dispatch stays automatic until the user changes or saves a profile.
    /// `choice()` remains explicit for retries.
    pub(crate) profile_selected: bool,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
    base: ProfileDefaults,
    /// Harness discovery result (models per harness); loaded the first time the picker opens.
    pub(crate) report: Option<Result<HarnessReport, String>>,
    loader: Option<HarnessLoader>,
    pub(crate) picker: Option<ModelPicker>,
    /// The `p` popup: name for saving the current choice as a new `[profiles.<name>]`.
    pub(crate) naming: Option<LineInput>,
}

impl RunChoice {
    /// Starts on `profile` (else the first configured one) with `model`/`effort` as the run's values; values
    /// equal to the profile's own are not overrides.
    pub(crate) fn new(
        cfg: &Config,
        profile: &str,
        model: Option<String>,
        effort: Option<String>,
    ) -> RunChoice {
        let mut profiles = cfg.profile_names();
        if profiles.is_empty() {
            profiles.push(FALLBACK_PROFILE.into());
        }
        let idx = profiles.iter().position(|p| p == profile).unwrap_or(0);
        let mut c = RunChoice {
            profiles,
            idx,
            profile_selected: false,
            model: None,
            effort: None,
            base: ProfileDefaults::default(),
            report: None,
            loader: None,
            picker: None,
            naming: None,
        };
        c.base = profile_defaults(cfg, c.profile());
        c.set_model(model);
        c.set_effort(effort);
        c
    }

    pub(crate) fn profile(&self) -> &str {
        self.profiles
            .get(self.idx)
            .map_or(FALLBACK_PROFILE, String::as_str)
    }

    pub(crate) fn overridden(&self) -> bool {
        self.model.is_some() || self.effort.is_some()
    }

    /// "omo", or "omo (overridden)" when the run overrides the profile's model or effort.
    pub(crate) fn display_name(&self) -> String {
        if self.overridden() {
            format!("{} (overridden)", self.profile())
        } else {
            self.profile().to_string()
        }
    }

    pub(crate) fn model_value(&self) -> Option<&str> {
        self.model.as_deref().or(self.base.model.as_deref())
    }

    pub(crate) fn effort_value(&self) -> Option<&str> {
        self.effort.as_deref().or(self.base.effort.as_deref())
    }

    pub(crate) fn choice(&self) -> ProfileChoice {
        ProfileChoice {
            profile: Some(self.profile().to_string()),
            model: self.model.clone(),
            effort: self.effort.clone(),
        }
    }

    fn set_profile(&mut self, cfg: &Config, i: usize) {
        if i >= self.profiles.len() {
            return;
        }
        self.profile_selected |= i != self.idx;
        self.idx = i;
        self.base = profile_defaults(cfg, self.profile());
        self.model = None;
        self.effort = None;
    }

    fn set_model(&mut self, model: Option<String>) {
        self.model = model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty() && Some(m.as_str()) != self.base.model.as_deref());
    }

    fn set_effort(&mut self, effort: Option<String>) {
        self.effort = effort
            .map(|e| normalize(&e))
            .filter(|e| !e.is_empty() && Some(e.as_str()) != self.base.effort.as_deref());
    }

    /// Next effort level; after the last one an override falls back to the profile's own effort.
    fn cycle_effort(&mut self) -> Action {
        let levels = &self.base.efforts;
        if levels.is_empty() {
            let why = if self.base.takes_effort {
                "accepts any effort; set it with corral dispatch --effort"
            } else {
                "takes no effort flag"
            };
            return Action::Toast(Toast::new(
                Tone::Warn,
                "No effort levels",
                format!("harness '{}' {why}", self.base.harness),
            ));
        }
        let pos = self
            .effort_value()
            .and_then(|e| levels.iter().position(|l| l == e));
        let next = match pos {
            Some(p) if p + 1 < levels.len() => Some(levels[p + 1].clone()),
            _ if self.effort.is_some() => None,
            _ => levels.first().cloned(),
        };
        self.set_effort(next);
        Action::None
    }

    fn poll(&mut self) {
        if let Some(loader) = &mut self.loader
            && let Some(msg) = loader.poll()
        {
            self.report = Some(msg);
            self.loader = None;
        }
    }

    fn open_picker(&mut self, ctx: &Ctx) -> Action {
        if !self.base.takes_model {
            return Action::Toast(Toast::new(
                Tone::Warn,
                "No model choice",
                format!("harness '{}' takes no model flag", self.base.harness),
            ));
        }
        if self.report.is_none() && self.loader.is_none() {
            self.report = corral_ops::cached_harness_report(ctx).map(Ok);
        }
        if self.report.is_none() && self.loader.is_none() {
            self.loader = Some(HarnessLoader::start(ctx, false));
        }
        self.picker = Some(ModelPicker::default());
        let current = self.model.clone();
        let sel = self
            .pick_rows()
            .iter()
            .position(|r| matches!((r, &current), (PickRow::Model(m), Some(c)) if m == c))
            .unwrap_or(0);
        if let Some(p) = &mut self.picker {
            p.sel = sel;
        }
        Action::None
    }

    fn models(&self) -> Vec<String> {
        match &self.report {
            Some(Ok(r)) => r
                .get(&self.base.harness)
                .map(|h| h.models.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn pick_rows(&self) -> Vec<PickRow> {
        let filter = self.picker.as_ref().map_or("", |p| p.filter.text.trim());
        let needle = filter.to_lowercase();
        let models = self.models();
        let mut rows = Vec::new();
        if filter.is_empty() {
            rows.push(PickRow::Default);
        } else if !models.iter().any(|m| m.eq_ignore_ascii_case(filter)) {
            rows.push(PickRow::Typed(filter.to_string()));
        }
        rows.extend(
            models
                .into_iter()
                .filter(|m| m.to_lowercase().contains(&needle))
                .map(PickRow::Model),
        );
        rows
    }

    fn pick(&mut self) {
        let sel = self.picker.as_ref().map_or(0, |p| p.sel);
        match self.pick_rows().get(sel) {
            Some(PickRow::Default) => self.model = None,
            Some(PickRow::Typed(m) | PickRow::Model(m)) => self.set_model(Some(m.clone())),
            None => return,
        }
        self.picker = None;
    }

    fn move_sel(&mut self, delta: isize) {
        let n = self.pick_rows().len();
        if let Some(p) = &mut self.picker {
            p.sel = p.sel.saturating_add_signed(delta).min(n.saturating_sub(1));
        }
    }

    fn picker_key(&mut self, key: &KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => self.picker = None,
            KeyCode::Enter => self.pick(),
            KeyCode::Up => self.move_sel(-1),
            KeyCode::Down => self.move_sel(1),
            _ => {
                if let Some(p) = &mut self.picker
                    && p.filter.edit(key)
                {
                    p.sel = 0;
                    p.offset = 0;
                }
            }
        }
        Action::None
    }

    /// Keys of this control: with the picker open every key (filter text, ↑↓, ⏎ pick, esc close); otherwise
    /// h/l (profile), M (model picker) and e (effort). `None`: not this control's key.
    pub(crate) fn on_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Option<Action> {
        if self.picker.is_some() {
            return Some(self.picker_key(key));
        }
        if self.naming.is_some() {
            return Some(self.naming_key(key, ctx));
        }
        let n = self.profiles.len().max(1);
        let cfg = &ctx.store.config;
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.set_profile(cfg, (self.idx + n - 1) % n),
            KeyCode::Char('l') | KeyCode::Right => self.set_profile(cfg, (self.idx + 1) % n),
            KeyCode::Char('M') => return Some(self.open_picker(ctx)),
            KeyCode::Char('e') => return Some(self.cycle_effort()),
            KeyCode::Char('p') => return Some(self.start_naming()),
            _ => return None,
        }
        Some(Action::None)
    }

    /// Mouse on this control: profile and effort pills select, picker rows select (a click on the selected row
    /// or a double-click picks), the wheel moves the picker selection. While the picker is open it takes every
    /// event (it is modal). `None`: not this control's event.
    pub(crate) fn on_mouse(&mut self, ev: &MouseEvent, m: &MouseCtx) -> Option<Action> {
        if self.picker.is_some() {
            if let Some(d) = hit::wheel(ev) {
                self.move_sel(d);
            } else if hit::left(ev)
                && let Some(Hit::Item(Z_MODEL, i)) = m.hit
            {
                let sel = self.picker.as_ref().map_or(usize::MAX, |p| p.sel);
                if sel == i || m.double {
                    if let Some(p) = &mut self.picker {
                        p.sel = i;
                    }
                    self.pick();
                } else if let Some(p) = &mut self.picker {
                    p.sel = i;
                }
            }
            return Some(Action::None);
        }
        if self.naming.is_some() {
            return Some(Action::None);
        }
        if !hit::left(ev) {
            return None;
        }
        match m.hit {
            Some(Hit::Item(Z_PROFILE, i)) => self.set_profile(&m.ctx.store.config, i),
            Some(Hit::Item(Z_EFFORT, i)) => {
                if let Some(level) = self.base.efforts.get(i).cloned() {
                    self.set_effort(Some(level));
                }
            }
            _ => return None,
        }
        Some(Action::None)
    }

    /// True while the model picker or the save-as-profile popup is open (they take every input).
    pub(crate) fn modal(&self) -> bool {
        self.picker.is_some() || self.naming.is_some()
    }

    pub(crate) fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.picker.is_some() {
            vec![("↑↓", "select"), ("⏎", "pick"), ("esc", "close")]
        } else if self.naming.is_some() {
            vec![("⏎", "save profile"), ("esc", "cancel")]
        } else if self.overridden() {
            vec![
                ("h/l", "profile"),
                ("M", "model"),
                ("e", "effort"),
                ("p", "save as profile"),
            ]
        } else {
            vec![("h/l", "profile"), ("M", "model"), ("e", "effort")]
        }
    }

    pub(crate) fn has_unsaved_text(&self) -> bool {
        let picker = self
            .picker
            .as_ref()
            .is_some_and(|p| !p.filter.text.is_empty());
        picker || self.naming.as_ref().is_some_and(|n| !n.text.is_empty())
    }

    /// Opens the name popup, prefilled with an unused "<profile>-<effort or model>" name.
    fn start_naming(&mut self) -> Action {
        if !self.overridden() {
            return Action::Toast(Toast::new(
                Tone::Warn,
                "Nothing to save",
                "override the model (M) or effort (e) first; p saves that as a new profile",
            ));
        }
        let tail = self
            .effort
            .clone()
            .or_else(|| {
                let m = self.model.as_deref()?;
                m.rsplit('/').next().map(str::to_string)
            })
            .unwrap_or_default();
        let stem: String = format!("{}-{tail}", self.profile())
            .chars()
            .map(|c| if valid_name_char(c) { c } else { '-' })
            .collect();
        let mut name = stem.clone();
        let mut n = 2;
        while self.profiles.contains(&name) {
            name = format!("{stem}-{n}");
            n += 1;
        }
        self.naming = Some(LineInput { text: name });
        Action::None
    }

    /// "harness·model·effort" the run would use (what `p` saves).
    fn preset_label(&self) -> String {
        [
            Some(self.base.harness.as_str()),
            self.model_value(),
            self.effort_value(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("·")
    }

    fn naming_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc => self.naming = None,
            KeyCode::Enter => return self.save_profile(ctx),
            _ => {
                if let Some(n) = &mut self.naming {
                    n.edit(key);
                }
            }
        }
        Action::None
    }

    /// Writes `[profiles.<name>]` (harness, model, effort, the profile's args) through the validated,
    /// comment-preserving config write path, then switches to the new profile with no overrides.
    fn save_profile(&mut self, ctx: &Ctx) -> Action {
        let name = self
            .naming
            .as_ref()
            .map(|n| n.text.trim().to_string())
            .unwrap_or_default();
        if name.is_empty() || !name.chars().all(valid_name_char) {
            return Action::Toast(Toast::new(
                Tone::Bad,
                "Profile not saved",
                format!("'{name}': use letters, digits, - and _"),
            ));
        }
        let mut entry = Table::new();
        entry.insert("harness", value(self.base.harness.as_str()));
        if let Some(m) = self.model_value() {
            entry.insert("model", value(m));
        }
        if let Some(e) = self.effort_value() {
            entry.insert("effort", value(e));
        }
        let args = ctx
            .store
            .config
            .profiles
            .get(self.profile())
            .map(|p| p.args.clone())
            .unwrap_or_default();
        if !args.is_empty() {
            entry.insert("args", value(Array::from_iter(args)));
        }
        let label = self.preset_label();
        match ctx.store.edit_config_from(ConfigSource::SettingsUi, |doc| {
            doc.add_entry("profiles", &name, entry)
        }) {
            Ok(saved) => {
                let (report, loader) = (self.report.take(), self.loader.take());
                *self = RunChoice::new(&saved, &name, None, None);
                self.profile_selected = true;
                self.report = report;
                self.loader = loader;
                Action::Toast(Toast::new(
                    Tone::Good,
                    format!("Saved profile {name}"),
                    format!("{label} · in corral.toml"),
                ))
            }
            Err(e) => Action::Toast(Toast::new(Tone::Bad, "Profile not saved", format!("{e:#}"))),
        }
    }

    /// Draws the profile, model and effort rows in the first [`ROWS`] lines of `area` with `label_w`-wide labels,
    /// and registers their click targets.
    pub(crate) fn draw(&mut self, f: &mut Frame, area: Rect, label_w: u16, hits: &mut HitMap) {
        self.poll();
        let lw = usize::from(label_w);
        let label = |s: &str| Span::styled(format!("{s:<lw$}"), theme::dim());
        let tag = |s: &str, overridden: bool| {
            let style = if overridden {
                Style::new().fg(theme::ORANGE)
            } else {
                theme::dim()
            };
            Span::styled(format!("  {s}"), style)
        };
        let x = area.x + label_w;
        let right = area.right();
        let row = |dy: u16| Rect::new(area.x, area.y + dy, area.width, 1);

        let names: Vec<&str> = self.profiles.iter().map(String::as_str).collect();
        let mut profile = vec![label("profile")];
        profile.extend(pills(&names, self.idx, "").spans);
        if self.overridden() {
            profile.push(tag("(overridden)", true));
        }
        profile.push(Span::styled("  h/l", theme::dim()));
        hit::pill_hits(hits, &names, Z_PROFILE, x, area.y, right);

        let mut model = vec![label("model")];
        if let Some(e) = &self.base.error {
            let room = usize::from(area.width).saturating_sub(lw);
            model.push(Span::styled(truncate(e, room), Style::new().fg(theme::RED)));
        } else if !self.base.takes_model {
            model.push(Span::styled(
                format!("— harness '{}' takes no model flag", self.base.harness),
                theme::dim(),
            ));
        } else {
            let value = self.model_value().unwrap_or("harness default").to_string();
            let shown = if self.model_value().is_some() {
                Span::styled(value, Style::new().fg(theme::FG))
            } else {
                Span::styled(value, theme::dim())
            };
            let source = match (&self.model, &self.base.model) {
                (Some(_), _) => tag("override", true),
                (None, Some(_)) => tag("from profile", false),
                (None, None) => Span::raw(""),
            };
            let field = shown.width() + source.width();
            model.extend([shown, source, Span::styled("  M", theme::dim())]);
            let w = (field as u16 + 3).min(right.saturating_sub(x));
            hits.push(
                Rect::new(x, area.y + 1, w, 1),
                Hit::Key(hit::press(KeyCode::Char('M'))),
            );
        }

        let mut effort = vec![label("effort")];
        if self.base.efforts.is_empty() {
            let text = match (self.effort_value(), self.base.takes_effort) {
                (Some(e), _) => e.to_string(),
                (None, true) => "harness default".to_string(),
                (None, false) => format!("— harness '{}' takes no effort flag", self.base.harness),
            };
            effort.push(Span::styled(text, theme::dim()));
        } else {
            let levels: Vec<&str> = self.base.efforts.iter().map(String::as_str).collect();
            let sel = self
                .effort_value()
                .and_then(|e| levels.iter().position(|l| *l == e))
                .unwrap_or(usize::MAX);
            effort.extend(pills(&levels, sel, "").spans);
            if self.effort.is_some() {
                effort.push(tag("override", true));
            }
            effort.push(Span::styled("  e", theme::dim()));
            hit::pill_hits(hits, &levels, Z_EFFORT, x, area.y + 2, right);
        }

        for (dy, spans) in [profile, model, effort].into_iter().enumerate() {
            let r = row(dy as u16);
            if r.y < area.bottom() {
                f.render_widget(Paragraph::new(Line::from(spans)), r);
            }
        }
    }

    /// Draws the open popup over `area` (model picker or save-as-profile name; both modal); call after the rest
    /// of the sheet.
    pub(crate) fn draw_popups(&mut self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        self.poll();
        if self.naming.is_some() {
            self.draw_naming(f, area, hits);
        }
        if self.picker.is_none() {
            return;
        }
        let rows = self.pick_rows();
        let status = match (&self.report, self.loader.is_some()) {
            (_, true) => "loading models…".to_string(),
            (Some(Err(e)), _) => format!("model list unavailable: {e}"),
            _ if self.models().is_empty() => "no model list; type a model name".to_string(),
            _ => format!("{} models · any typed name works too", self.models().len()),
        };
        let default_text = format!(
            "profile default ({})",
            self.base.model.as_deref().unwrap_or("harness default")
        );
        let current = self.model.clone();
        let title = format!("model · {}", self.base.harness);
        let Some(p) = &mut self.picker else {
            return;
        };
        let list_h = rows.len().clamp(1, PICKER_ROWS) as u16;
        let rect = centered(area, 60, list_h + 4);
        let inner = popup(f, rect, &title, hits);
        if inner.height < 3 {
            return;
        }
        let w = usize::from(inner.width);
        f.render_widget(
            Paragraph::new(Line::styled(truncate(&status, w), theme::dim())),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
        let list = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
        let h = usize::from(list.height);
        p.sel = p.sel.min(rows.len().saturating_sub(1));
        if p.sel < p.offset {
            p.offset = p.sel;
        } else if h > 0 && p.sel >= p.offset + h {
            p.offset = p.sel + 1 - h;
        }
        let lines: Vec<Line> = rows
            .iter()
            .enumerate()
            .skip(p.offset)
            .take(h)
            .map(|(i, r)| {
                let (text, on) = match r {
                    PickRow::Default => (default_text.clone(), current.is_none()),
                    PickRow::Typed(s) => (format!("use \"{s}\""), false),
                    PickRow::Model(s) => (s.clone(), current.as_deref() == Some(s.as_str())),
                };
                let mark = if on { "• " } else { "  " };
                let text = truncate(&format!("{mark}{text}"), w.saturating_sub(2));
                if i == p.sel {
                    Line::from(vec![
                        Span::styled("▌ ", Style::new().fg(theme::CYAN)),
                        Span::styled(
                            text,
                            Style::new()
                                .fg(theme::CYAN)
                                .bg(theme::SELECTED_BG)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ])
                } else {
                    Line::from(vec![Span::raw("  "), Span::styled(text, theme::dim())])
                }
            })
            .collect();
        f.render_widget(Paragraph::new(lines), list);
        hit::rows(hits, list, Z_MODEL, 0, rows.len(), p.offset);
        p.filter.draw(
            f,
            Rect::new(inner.x, inner.y, inner.width, 1),
            "model ›",
            "type to filter, or a model name",
        );
    }

    fn draw_naming(&mut self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        let label = self.preset_label();
        let Some(input) = &self.naming else {
            return;
        };
        let rect = centered(area, 60, 5);
        let inner = popup(f, rect, "save as profile", hits);
        if inner.height < 3 {
            return;
        }
        let w = usize::from(inner.width);
        f.render_widget(
            Paragraph::new(Line::styled(
                truncate(&format!("[profiles.<name>] = {label}"), w),
                theme::dim(),
            )),
            Rect::new(inner.x, inner.y + 2, inner.width, 1),
        );
        input.draw(
            f,
            Rect::new(inner.x, inner.y, inner.width, 1),
            "name ›",
            "letters, digits, - and _",
        );
    }
}
