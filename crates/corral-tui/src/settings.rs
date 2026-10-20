//! Settings screen (`,` on the board, `corral settings`): a section list on the left and the section's form on
//! the right. Edits are staged in a draft `Config`; `s` saves to the selected layer, which validates and
//! keeps the file's comments. Clicks resolve against the regions the last draw recorded (`fields::Target`).

mod automation;
#[cfg(test)]
mod automation_tests;
#[cfg(test)]
mod cache_tests;
mod card_preview;
mod cards;
#[cfg(test)]
mod cards_tests;
mod catalog;
#[cfg(test)]
mod catalog_tests;
mod confirm;
mod fields;
mod gate;
#[cfg(test)]
mod gate_tests;
mod layers;
#[cfg(test)]
mod layers_tests;
mod model;
mod mouse;
#[cfg(test)]
mod mouse_tests;
mod operating;
mod operating_state;
#[cfg(test)]
mod operating_tests;
mod picker;
mod preferences;
#[cfg(test)]
mod preferences_tests;
mod profiles;
#[cfg(test)]
mod tests;

use corral_core::{Config, ConfigLayer, ConfigLayers, Store};
use corral_ops::{Ctx, HarnessReport};
use ratatui::Frame;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use self::fields::{Editing, FormLine, Target};
use self::model::{Field, Kind, Row, SECTIONS, Section};
use self::picker::Picker;
use crate::discovery::HarnessLoader;
use crate::hit::{Hit, HitMap, MouseCtx};
use crate::overlays::LineInput;
use crate::theme;
use crate::{Action, Toast, Tone, View};

const NAV_W: u16 = 18;
const BACK: &str = " ‹ back ";

struct Edit {
    field: Field,
    input: LineInput,
    error: Option<String>,
}

pub struct SettingsView {
    operating: operating::Controls,
    operating_only: bool,
    base: Config,
    draft: Config,
    layers: layers::Cache,
    config_hash: String,
    layer_loader: fn(&Store) -> anyhow::Result<ConfigLayers>,
    save_target: ConfigLayer,
    section: usize,
    sel: usize,
    scroll: usize,
    follow: bool,
    edit: Option<Edit>,
    confirm: bool,
    errors: Vec<String>,
    herdr_env: Option<String>,
    regions: Vec<(Rect, Target)>,
    /// Profile whose editor replaces the Profiles list.
    open: Option<String>,
    picker: Option<Picker>,
    /// Source of a `y` duplicate while its name is typed.
    dup_from: Option<String>,
    report: Option<HarnessReport>,
    report_error: Option<String>,
    loader: Option<HarnessLoader>,
    gates: gate::Gates,
    #[cfg(test)]
    discovery_report: Option<HarnessReport>,
    #[cfg(test)]
    validations: usize,
}

fn esc() -> KeyEvent {
    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
}

impl SettingsView {
    pub fn new(ctx: &Ctx) -> SettingsView {
        Self::with_layer_loader(ctx, Store::config_layers)
    }

    fn with_layer_loader(
        ctx: &Ctx,
        layer_loader: fn(&Store) -> anyhow::Result<ConfigLayers>,
    ) -> Self {
        let mut v = SettingsView {
            operating: operating::Controls::new(ctx),
            operating_only: false,
            base: ctx.store.config.clone(),
            draft: ctx.store.config.clone(),
            layers: layers::Cache::load(&ctx.store, layer_loader),
            config_hash: ctx.store.config_hash().into(),
            layer_loader,
            save_target: ConfigLayer::Workspace,
            section: 0,
            sel: 0,
            scroll: 0,
            follow: true,
            edit: None,
            confirm: false,
            errors: Vec::new(),
            herdr_env: std::env::var("CORRAL_HERDR_SESSION")
                .ok()
                .filter(|s| !s.is_empty()),
            regions: Vec::new(),
            open: None,
            picker: None,
            dup_from: None,
            report: corral_ops::cached_harness_report(ctx),
            report_error: None,
            loader: None,
            gates: gate::Gates::new(),
            #[cfg(test)]
            discovery_report: None,
            #[cfg(test)]
            validations: 0,
        };
        v.revalidate(ctx);
        v
    }

    /// Supplies deterministic discovery, including re-detection, for app-level journeys.
    #[cfg(test)]
    pub(crate) fn with_discovery_report(ctx: &Ctx, report: HarnessReport) -> Self {
        let mut v = Self::new(ctx);
        v.report = Some(report.clone());
        v.discovery_report = Some(report);
        v
    }

    fn rows_of(&self, s: Section) -> Vec<Row> {
        if s == Section::Automation && self.operating_only {
            return self.operating.rows();
        }
        let mut rows = if s == Section::Automation {
            vec![Row::new(
                Field::Operating(operating::Key::Open),
                "Operating mode",
                Kind::Entry,
                "scope and authority".into(),
            )]
        } else {
            Vec::new()
        };
        rows.extend(model::rows(
            s,
            &self.draft,
            self.herdr_env.as_deref(),
            self.report.as_ref(),
            &self.gates,
        ));
        rows
    }

    fn rows(&self) -> Vec<Row> {
        match &self.open {
            Some(p) if self.in_section(Section::Profiles) => {
                catalog::editor_rows(&self.draft, p, self.report.as_ref())
            }
            _ => self.rows_of(SECTIONS[self.section]),
        }
    }

    fn current(&self) -> Option<Row> {
        self.rows().into_iter().nth(self.sel)
    }

    pub(crate) fn dirty(&self) -> bool {
        self.base != self.draft
    }

    fn revalidate(&mut self, ctx: &Ctx) {
        #[cfg(test)]
        {
            self.validations += 1;
        }
        gate::sync(&mut self.gates, &self.draft, ctx.store.root());
        self.errors = match self.draft.validate(ctx.store.root()) {
            Ok(()) => Vec::new(),
            Err(e) => e.0,
        };
    }

    fn section_problems(&self, s: Section) -> usize {
        let rows = self.rows_of(s);
        self.errors
            .iter()
            .filter(|e| rows.iter().any(|r| r.owns(e)))
            .count()
    }

    /// Problems no row of any section owns (e.g. a broken harness); they still block saving.
    fn elsewhere(&self) -> Vec<String> {
        let all: Vec<Row> = SECTIONS.iter().flat_map(|s| self.rows_of(*s)).collect();
        self.errors
            .iter()
            .filter(|e| !all.iter().any(|r| r.owns(e)))
            .cloned()
            .collect()
    }

    fn select(&mut self, i: usize) {
        let n = self.rows().len();
        self.sel = i.min(n.saturating_sub(1));
        self.follow = true;
    }

    fn select_field(&mut self, field: &Field) {
        if let Some(i) = self.rows().iter().position(|r| r.field == *field) {
            self.select(i);
        }
    }

    fn set_section(&mut self, i: usize) {
        self.operating_only = false;
        self.section = i % SECTIONS.len();
        self.open = None;
        self.sel = 0;
        self.scroll = 0;
        self.follow = true;
    }

    /// Applies one edit to the draft. Rows are rebuilt from the draft (the Board cards preview changes height),
    /// so the selection is kept by field identity, never by position.
    fn set(&mut self, ctx: &Ctx, field: &Field, text: &str) -> Result<(), String> {
        if let Field::Operating(key) = field {
            return self.operating.set(*key, text);
        }
        let selected = self.current().map(|r| r.field);
        model::set_value(&mut self.draft, field, text)?;
        self.revalidate(ctx);
        if let Some(selected) = selected {
            self.select_field(&selected);
        }
        Ok(())
    }

    fn start_edit(&mut self, field: Field, text: String) {
        self.edit = Some(Edit {
            field,
            input: LineInput { text },
            error: None,
        });
    }

    /// Applies the open edit. False (edit kept open with its error) when the text does not parse.
    fn commit_edit(&mut self, ctx: &Ctx) -> bool {
        let Some(mut e) = self.edit.take() else {
            return true;
        };
        let text = e.input.text.clone();
        let adding = matches!(
            e.field,
            Field::AddProject
                | Field::AddGlob
                | Field::AddProfile
                | Field::Review(_, automation::Key::Add(_))
        );
        if adding && text.trim().is_empty() {
            self.dup_from = None;
            return true;
        }
        if e.field == Field::AddProfile {
            return self.commit_new_profile(ctx, e);
        }
        match self.set(ctx, &e.field, &text) {
            Ok(()) => {
                match e.field {
                    Field::AddProject => self.select_field(&Field::ProjectPath(text.trim().into())),
                    _ => self.select(self.sel),
                }
                true
            }
            Err(err) => {
                e.error = Some(err);
                self.edit = Some(e);
                false
            }
        }
    }

    fn activate(&mut self, ctx: &Ctx) -> Action {
        let Some(row) = self.current() else {
            return Action::None;
        };
        if let Field::Operating(key) = row.field
            && matches!(row.kind, Kind::Entry)
        {
            return self.operating_action(key, ctx);
        }
        match row.kind {
            Kind::Text { .. } | Kind::Number { .. } | Kind::ListItem => {
                self.start_edit(row.field, row.value)
            }
            Kind::Add => self.start_edit(row.field, String::new()),
            Kind::Toggle | Kind::Pick(_) => return self.cycle(ctx, true),
            Kind::Entry => {
                if let Field::Profile(name) = row.field {
                    self.open_profile(name);
                }
            }
            Kind::Picker => self.open_picker(),
            Kind::Stage(ref commands) => return self.stage(ctx, &row, commands),
            Kind::Group | Kind::Header | Kind::Info => {}
        }
        Action::None
    }

    fn cycle(&mut self, ctx: &Ctx, forward: bool) -> Action {
        let Some(row) = self.current() else {
            return Action::None;
        };
        let n = fields::pill_labels(&row.kind).len();
        if n == 0 {
            return Action::None;
        }
        let at = fields::pill_index(&row);
        let next = if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        };
        self.choose(ctx, self.sel, next)
    }

    fn choose(&mut self, ctx: &Ctx, i: usize, j: usize) -> Action {
        let Some(row) = self.rows().into_iter().nth(i) else {
            return Action::None;
        };
        let Some(value) = fields::pill_value(&row.kind, j) else {
            return Action::None;
        };
        match self.set(ctx, &row.field, &value) {
            Ok(()) => {
                self.select_field(&row.field);
                Action::None
            }
            Err(e) => Action::Toast(Toast::new(
                Tone::Bad,
                format!("{} not changed", row.label),
                e,
            )),
        }
    }

    fn remove(&mut self, ctx: &Ctx) {
        let Some(row) = self.current() else {
            return;
        };
        match &row.field {
            Field::Glob(i) => {
                self.draft.artifacts.globs.remove(*i);
            }
            Field::Project(k)
            | Field::ProjectPath(k)
            | Field::ProjectBase(k)
            | Field::ProjectProfile(k)
            | Field::ProjectCap(k) => model::remove_project(&mut self.draft, k),
            f @ Field::Review(..) => {
                if !automation::remove(&mut self.draft, f) {
                    return;
                }
            }
            _ => return,
        }
        self.revalidate(ctx);
        self.select(self.sel);
    }

    fn move_item(&mut self, ctx: &Ctx, up: bool) {
        match self.current().map(|r| r.field) {
            Some(Field::Glob(i)) => {
                if let Some(j) = model::move_glob(&mut self.draft, i, up) {
                    self.select_field(&Field::Glob(j));
                }
            }
            Some(f @ Field::Review(..)) => {
                if let Some(to) = automation::move_item(&mut self.draft, &f, up) {
                    self.revalidate(ctx);
                    self.select_field(&to);
                }
            }
            _ => {}
        }
    }

    fn add(&mut self) {
        let field = match SECTIONS[self.section] {
            Section::Projects => Field::AddProject,
            Section::Artifacts => Field::AddGlob,
            Section::General
            | Section::Profiles
            | Section::Harnesses
            | Section::Automation
            | Section::Cards
            | Section::Preferences => {
                return;
            }
        };
        self.select_field(&field);
        self.start_edit(field, String::new());
    }

    fn edit_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.edit = None;
                self.dup_from = None;
            }
            KeyCode::Enter => {
                self.commit_edit(ctx);
            }
            _ => {
                if let Some(e) = &mut self.edit {
                    e.input.edit(&key);
                    e.error = None;
                }
            }
        }
        Action::None
    }

    fn draw_title(&self, f: &mut Frame, area: Rect, ctx: &Ctx, hits: &mut HitMap) {
        let path = ctx.store.config_path();
        let shown = path
            .strip_prefix(ctx.store.root())
            .unwrap_or(&path)
            .display()
            .to_string();
        let mut spans = vec![
            Span::styled(BACK, Style::new().fg(theme::CYAN)),
            Span::styled(
                " settings",
                Style::new().fg(theme::CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" · {shown}"), theme::dim()),
        ];
        if self.dirty() {
            spans.push(Span::styled(
                "   ● unsaved changes",
                Style::new().fg(theme::YELLOW),
            ));
        }
        hits.push(
            Rect::new(
                area.x,
                area.y,
                (BACK.chars().count() as u16).min(area.width),
                1,
            ),
            Hit::Key(esc()),
        );
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_nav(&mut self, f: &mut Frame, area: Rect) {
        let block = theme::titled("sections");
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.regions.push((area, Target::SectionList));
        for (i, s) in SECTIONS.iter().enumerate() {
            let y = inner.y + i as u16;
            if y >= inner.bottom() {
                break;
            }
            let on = i == self.section;
            let style = if on {
                Style::new()
                    .fg(theme::CYAN)
                    .bg(theme::SELECTED_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(theme::FG)
            };
            let mut spans = vec![Span::styled(
                format!("{}{}", if on { "▸ " } else { "  " }, s.label()),
                style,
            )];
            let n = self.section_problems(*s);
            if n > 0 {
                spans.push(Span::styled(format!(" ✗{n}"), Style::new().fg(theme::RED)));
            }
            let rect = Rect::new(inner.x, y, inner.width, 1);
            f.render_widget(Paragraph::new(Line::from(spans)), rect);
            self.regions.push((rect, Target::Section(i)));
        }
    }

    fn form_lines(&self, ctx: &Ctx, width: usize) -> Vec<FormLine> {
        let rows = self.rows();
        let mut lines = self.catalog_head();
        let saved_rows = self.saved_rows();
        for (i, row) in rows.iter().enumerate() {
            lines.extend(self.gate_preview(row, rows.get(self.sel)));
            if matches!(row.kind, Kind::Group | Kind::Header) && i > 0 {
                lines.push(FormLine::plain(Line::from("")));
            }
            if matches!(
                row.field,
                Field::AddProject | Field::AddGlob | Field::AddProfile
            ) {
                lines.push(FormLine::plain(Line::from("")));
            }
            let editing = self
                .edit
                .as_ref()
                .filter(|e| e.field == row.field)
                .map(|e| Editing {
                    text: &e.input.text,
                    error: e.error.as_deref(),
                });
            let errs: Vec<&str> = self
                .errors
                .iter()
                .filter(|e| row.owns(e))
                .map(String::as_str)
                .collect();
            let mut rendered = fields::row_lines(i, row, i == self.sel, editing, &errs, width);
            layers::annotate(
                &mut rendered,
                row,
                &self.draft,
                self.layers.sources.as_ref(),
                self.row_staged(row, &saved_rows),
                self.save_target,
            );
            lines.extend(rendered);
        }
        lines.extend(self.preview_lines(ctx, width));
        let elsewhere = self.elsewhere();
        if !elsewhere.is_empty() {
            lines.push(FormLine::plain(Line::from("")));
            lines.push(FormLine::plain(Line::styled(
                "  Elsewhere in corral.toml (edit the file; saving is blocked until fixed):",
                Style::new().fg(theme::RED),
            )));
            for e in elsewhere {
                lines.push(FormLine::plain(Line::styled(
                    format!("    ✗ {e}"),
                    Style::new().fg(theme::RED),
                )));
            }
        }
        lines
    }

    fn draw_form(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let title = match &self.open {
            Some(p) if self.in_section(Section::Profiles) => format!("Profiles › {p}"),
            _ => SECTIONS[self.section].label().to_string(),
        };
        let block = theme::titled_focused(title);
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.regions.push((area, Target::Form));
        let lines = self.form_lines(ctx, inner.width as usize);
        let h = inner.height as usize;
        if self.follow {
            let first = lines.iter().position(|l| l.row == Some(self.sel));
            let last = lines.iter().rposition(|l| l.row == Some(self.sel));
            if let (Some(first), Some(last)) = (first, last) {
                if first < self.scroll {
                    self.scroll = first;
                } else if last >= self.scroll + h {
                    self.scroll = (last + 1).saturating_sub(h);
                }
            }
        }
        self.scroll = self.scroll.min(lines.len().saturating_sub(h));
        for (n, l) in lines.into_iter().skip(self.scroll).take(h).enumerate() {
            let rect = Rect::new(inner.x, inner.y + n as u16, inner.width, 1);
            f.render_widget(Paragraph::new(l.line), rect);
            if let Some(r) = l.row {
                self.regions.push((rect, Target::Row(r)));
            }
            for (x, w, t) in l.targets {
                let x = inner.x.saturating_add(x).min(inner.right());
                let w = w.min(inner.right() - x);
                self.regions.push((Rect::new(x, rect.y, w, 1), t));
            }
        }
    }
}

impl View for SettingsView {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, hits: &mut HitMap) {
        self.regions.clear();
        self.poll_report(ctx);
        let [title, body, targets] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(3),
        ])
        .areas(area);
        let [nav, form] =
            Layout::horizontal([Constraint::Length(NAV_W), Constraint::Fill(1)]).areas(body);
        self.draw_title(f, title, ctx, hits);
        self.draw_nav(f, nav);
        self.draw_form(f, form, ctx);
        self.draw_save_target(f, targets);
        if let Some(p) = &mut self.picker {
            p.draw(f, area, hits, &mut self.regions);
        }
        if self.confirm {
            self.draw_confirm(f, area, hits);
        }
        self.draw_operating(f, area, hits);
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if self.operating.preview.is_some() || self.operating.pick.is_some() {
            return self.operating_key(key, ctx);
        }
        if self.confirm {
            return self.confirm_key(key, ctx);
        }
        if self.edit.is_some() {
            return self.edit_key(key, ctx);
        }
        if self.picker.is_some() {
            return self.picker_key(key, ctx);
        }
        if let Some(action) = self.catalog_key(key, ctx) {
            return action;
        }
        let n = self.rows().len();
        match key.code {
            KeyCode::Char('w') => self.save_target = ConfigLayer::Workspace,
            KeyCode::Char('u') => self.save_target = ConfigLayer::User,
            KeyCode::Esc if self.dirty() => self.confirm = true,
            KeyCode::Esc => return Action::Back,
            KeyCode::Char('s') => {
                return match self.save(ctx) {
                    Ok(t) | Err(t) => Action::Toast(t),
                };
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.select((self.sel + 1).min(n.saturating_sub(1)))
            }
            KeyCode::Up | KeyCode::Char('k') => self.select(self.sel.saturating_sub(1)),
            KeyCode::Home => self.select(0),
            KeyCode::End => self.select(n.saturating_sub(1)),
            KeyCode::Tab => self.set_section(self.section + 1),
            KeyCode::BackTab => self.set_section(self.section + SECTIONS.len() - 1),
            KeyCode::PageDown => self.wheel(Target::Form, true),
            KeyCode::PageUp => self.wheel(Target::Form, false),
            KeyCode::Char(c @ '1'..='8') => {
                self.set_section(c as usize - '1' as usize);
                if self.in_section(Section::Automation) {
                    self.operating.refresh(ctx);
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => return self.activate(ctx),
            KeyCode::Left | KeyCode::Char('h') => return self.cycle(ctx, false),
            KeyCode::Right | KeyCode::Char('l') => return self.cycle(ctx, true),
            KeyCode::Char('a') => self.add(),
            KeyCode::Char('x') | KeyCode::Delete => self.remove(ctx),
            KeyCode::Char('K') => self.move_item(ctx, true),
            KeyCode::Char('J') => self.move_item(ctx, false),
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if self.operating.pick.is_some() {
            return Action::None;
        }
        if self.operating.preview.is_some() {
            match ev.kind {
                MouseEventKind::ScrollDown => {
                    self.operating.scroll = self.operating.scroll.saturating_add(3)
                }
                MouseEventKind::ScrollUp => {
                    self.operating.scroll = self.operating.scroll.saturating_sub(3)
                }
                _ => {}
            }
            return Action::None;
        }
        if self.confirm {
            return Action::None;
        }
        if self.picker.is_some() {
            return self.picker_mouse(ev, m.ctx);
        }
        let Some(target) = self.target_at(ev.column, ev.row) else {
            return Action::None;
        };
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => self.click(target, m.ctx),
            MouseEventKind::ScrollDown => {
                self.wheel(target, true);
                Action::None
            }
            MouseEventKind::ScrollUp => {
                self.wheel(target, false);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.operating.preview.is_some() {
            return vec![
                ("y", "grant"),
                ("esc", "cancel"),
                ("pgup/pgdn", "review scope"),
            ];
        }
        if self.operating.pick.is_some() {
            return vec![("space", "select"), ("⏎", "apply"), ("esc", "cancel")];
        }
        if self.confirm {
            return vec![
                ("y", "discard"),
                ("s", "save and close"),
                ("n", "keep editing"),
            ];
        }
        if self.edit.is_some() {
            return vec![("⏎", "apply"), ("esc", "cancel")];
        }
        if self.picker.is_some() {
            return vec![("↑↓", "move"), ("⏎", "choose"), ("esc", "cancel")];
        }
        let mut h = vec![("↑↓", "field"), ("⇥", "section"), ("pgup/pgdn", "scroll")];
        let row = self.current();
        match row.as_ref().map(|r| &r.kind) {
            Some(Kind::Text { .. } | Kind::Number { .. }) => h.push(("⏎", "edit")),
            Some(Kind::ListItem) => h.extend([("⏎", "edit"), ("x", "remove"), ("K/J", "reorder")]),
            Some(Kind::Pick(_)) => h.push(("←→", "choose")),
            Some(Kind::Toggle) => h.push(("⏎", "toggle")),
            Some(Kind::Add) => h.push(("⏎", "add")),
            Some(Kind::Entry) => h.push(("⏎", "edit")),
            Some(Kind::Picker) => h.push(("⏎", "choose model")),
            Some(Kind::Stage(_)) => h.push(("⏎", "use this gate")),
            Some(Kind::Group | Kind::Header | Kind::Info) | None => {}
        }
        let in_project = row.as_ref().is_some_and(|r| {
            matches!(
                r.field,
                Field::Project(_)
                    | Field::ProjectPath(_)
                    | Field::ProjectBase(_)
                    | Field::ProjectProfile(_)
                    | Field::ProjectCap(_)
            )
        });
        if in_project {
            h.push(("x", "remove project"));
        }
        match SECTIONS[self.section] {
            Section::Projects => h.push(("a", "add project")),
            Section::Artifacts => h.push(("a", "add glob")),
            Section::Profiles | Section::Harnesses => h.extend(self.catalog_hints()),
            Section::General | Section::Automation | Section::Cards | Section::Preferences => {}
        }
        h.push((
            "s",
            if self.dirty() {
                "save ● unsaved"
            } else {
                "save"
            },
        ));
        h.push((
            "esc",
            if self.open.is_some() {
                "profiles"
            } else {
                "back"
            },
        ));
        h
    }

    fn refresh(&mut self, ctx: &Ctx) {
        if self.config_hash != ctx.store.config_hash() || self.in_section(Section::Automation) {
            self.operating.refresh(ctx);
        }
        if self.config_hash != ctx.store.config_hash() {
            self.layers = layers::Cache::load(&ctx.store, self.layer_loader);
            self.config_hash = ctx.store.config_hash().into();
        }
        if self.base != ctx.store.config && !self.dirty() && self.edit.is_none() && !self.confirm {
            self.base = ctx.store.config.clone();
            self.draft = ctx.store.config.clone();
            self.select(self.sel);
            self.revalidate(ctx);
        }
    }

    fn captures_text(&self) -> bool {
        self.edit.is_some()
            || self.picker.is_some()
            || self.operating.preview.is_some()
            || self.operating.pick.is_some()
    }

    fn has_unsaved_text(&self) -> bool {
        self.picker
            .as_ref()
            .is_some_and(|p| !p.input.text.is_empty())
    }
}
