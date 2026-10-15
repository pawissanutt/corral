//! Settings screen (`,` on the board, `corral settings`): a section list on the left and the section's form on
//! the right. Edits are staged in a draft `Config`; `s` saves through `Store::edit_config`, which validates and
//! keeps the file's comments. Clicks resolve against the regions the last draw recorded (`fields::Target`).

mod fields;
mod model;
#[cfg(test)]
mod mouse_tests;
#[cfg(test)]
mod tests;

use corral_core::{Config, ConfigErrors};
use corral_ops::Ctx;
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
use crate::hit::{self, Hit, HitMap, MouseCtx};
use crate::overlays::{self, LineInput, centered};
use crate::theme;
use crate::{Action, Toast, Tone, View};

const NAV_W: u16 = 18;
const WHEEL_LINES: usize = 3;
const BACK: &str = " ‹ back ";

struct Edit {
    field: Field,
    input: LineInput,
    error: Option<String>,
}

pub struct SettingsView {
    base: Config,
    draft: Config,
    section: usize,
    sel: usize,
    scroll: usize,
    follow: bool,
    edit: Option<Edit>,
    confirm: bool,
    errors: Vec<String>,
    herdr_env: Option<String>,
    regions: Vec<(Rect, Target)>,
}

fn esc() -> KeyEvent {
    KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
}

impl SettingsView {
    pub fn new(ctx: &Ctx) -> SettingsView {
        let mut v = SettingsView {
            base: ctx.store.config.clone(),
            draft: ctx.store.config.clone(),
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
        };
        v.revalidate(ctx);
        v
    }

    fn rows_of(&self, s: Section) -> Vec<Row> {
        model::rows(s, &self.draft, self.herdr_env.as_deref())
    }

    fn rows(&self) -> Vec<Row> {
        self.rows_of(SECTIONS[self.section])
    }

    fn current(&self) -> Option<Row> {
        self.rows().into_iter().nth(self.sel)
    }

    pub(crate) fn dirty(&self) -> bool {
        self.base != self.draft
    }

    fn revalidate(&mut self, ctx: &Ctx) {
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
        self.section = i % SECTIONS.len();
        self.sel = 0;
        self.scroll = 0;
        self.follow = true;
    }

    fn set(&mut self, ctx: &Ctx, field: &Field, text: &str) -> Result<(), String> {
        model::set_value(&mut self.draft, field, text)?;
        self.revalidate(ctx);
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
        let adding = matches!(e.field, Field::AddProject | Field::AddGlob);
        if adding && text.trim().is_empty() {
            return true;
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
        match row.kind {
            Kind::Text { .. } | Kind::Number { .. } | Kind::ListItem => {
                self.start_edit(row.field, row.value)
            }
            Kind::Add => self.start_edit(row.field, String::new()),
            Kind::Toggle | Kind::Pick(_) => return self.cycle(ctx, true),
            Kind::Group => {}
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
            Ok(()) => Action::None,
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
            _ => return,
        }
        self.revalidate(ctx);
        self.select(self.sel);
    }

    fn move_item(&mut self, up: bool) {
        if let Some(Field::Glob(i)) = self.current().map(|r| r.field)
            && let Some(j) = model::move_glob(&mut self.draft, i, up)
        {
            self.select_field(&Field::Glob(j));
        }
    }

    fn add(&mut self) {
        let field = match SECTIONS[self.section] {
            Section::Projects => Field::AddProject,
            Section::Artifacts => Field::AddGlob,
            Section::General => return,
        };
        self.select_field(&field);
        self.start_edit(field, String::new());
    }

    /// Validates the draft and writes what changed on top of the file as it is now.
    fn save(&mut self, ctx: &Ctx) -> Result<Toast, Toast> {
        if !self.commit_edit(ctx) {
            return Err(Toast::new(
                Tone::Warn,
                "Not saved",
                "fix the field being edited",
            ));
        }
        self.revalidate(ctx);
        if !self.errors.is_empty() {
            let n = self.errors.len();
            return Err(Toast::new(
                Tone::Warn,
                "Not saved",
                format!(
                    "{n} problem{}; see the marked fields",
                    if n == 1 { "" } else { "s" }
                ),
            ));
        }
        let (base, draft) = (self.base.clone(), self.draft.clone());
        let saved = ctx.store.edit_config(|doc| {
            let mut now = doc.config()?;
            model::apply_changes(&base, &draft, &mut now);
            doc.sync_from(&now)
        });
        match saved {
            Ok(cfg) => {
                self.base = cfg.clone();
                self.draft = cfg;
                self.revalidate(ctx);
                Ok(Toast::new(
                    Tone::Good,
                    "Settings saved",
                    ctx.store.config_path().display().to_string(),
                ))
            }
            Err(e) => {
                if let Some(errs) = e.chain().find_map(|c| c.downcast_ref::<ConfigErrors>()) {
                    self.errors = errs.0.clone();
                }
                Err(Toast::new(Tone::Bad, "Not saved", format!("{e:#}")))
            }
        }
    }

    fn confirm_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Char('y') => Action::Back,
            KeyCode::Char('s') => {
                self.confirm = false;
                match self.save(ctx) {
                    Ok(t) => Action::BackWithToast(t),
                    Err(t) => Action::Toast(t),
                }
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.confirm = false;
                Action::None
            }
            _ => Action::None,
        }
    }

    fn edit_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc => self.edit = None,
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

    fn target_at(&self, x: u16, y: u16) -> Option<Target> {
        self.regions
            .iter()
            .rev()
            .find(|(r, _)| r.contains((x, y).into()))
            .map(|(_, t)| *t)
    }

    fn click(&mut self, target: Target, ctx: &Ctx) -> Action {
        let row_of = |t: Target| match t {
            Target::Row(i)
            | Target::Pill(i, _)
            | Target::Up(i)
            | Target::Down(i)
            | Target::Remove(i) => Some(i),
            _ => None,
        };
        if let Some(e) = &self.edit {
            let same = row_of(target)
                .is_some_and(|i| self.rows().get(i).is_some_and(|r| r.field == e.field));
            if same || !self.commit_edit(ctx) {
                return Action::None;
            }
        }
        let key = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        match target {
            Target::Section(i) => self.set_section(i),
            Target::Row(i) if i == self.sel => {
                return self.on_key(KeyEvent::from(KeyCode::Enter), ctx);
            }
            Target::Row(i) => self.select(i),
            Target::Pill(i, j) => {
                self.select(i);
                return self.choose(ctx, i, j);
            }
            Target::Up(i) => {
                self.select(i);
                return self.on_key(key('K'), ctx);
            }
            Target::Down(i) => {
                self.select(i);
                return self.on_key(key('J'), ctx);
            }
            Target::Remove(i) => {
                self.select(i);
                return self.on_key(key('x'), ctx);
            }
            Target::Form | Target::SectionList => {}
        }
        Action::None
    }

    fn wheel(&mut self, target: Target, down: bool) {
        match target {
            Target::Section(_) | Target::SectionList => {
                let n = SECTIONS.len();
                self.set_section(if down {
                    self.section + 1
                } else {
                    self.section + n - 1
                });
            }
            _ => {
                self.follow = false;
                self.scroll = if down {
                    self.scroll + WHEEL_LINES
                } else {
                    self.scroll.saturating_sub(WHEEL_LINES)
                };
            }
        }
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

    fn form_lines(&self) -> Vec<FormLine> {
        let rows = self.rows();
        let mut lines = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            if row.kind == Kind::Group && i > 0 {
                lines.push(FormLine::plain(Line::from("")));
            }
            if row.field == Field::AddProject || row.field == Field::AddGlob {
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
            lines.extend(fields::row_lines(i, row, i == self.sel, editing, &errs));
        }
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

    fn draw_form(&mut self, f: &mut Frame, area: Rect) {
        let block = theme::titled_focused(SECTIONS[self.section].label());
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.regions.push((area, Target::Form));
        let lines = self.form_lines();
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

    fn draw_confirm(&self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        overlays::dim_backdrop(f.buffer_mut(), area);
        let rect = centered(area, 60, 6);
        let inner = overlays::popup(f, rect, "unsaved changes", hits);
        if inner.height < 3 {
            return;
        }
        f.render_widget(
            Paragraph::new(Line::raw("Your settings changes are not saved yet.")),
            Rect { height: 1, ..inner },
        );
        let buttons = [
            ("y", "discard"),
            ("s", "save and close"),
            ("n", "keep editing"),
        ];
        let y = inner.y + 2;
        hit::hint_hits(hits, &buttons, inner.x, y, inner.right());
        f.render_widget(
            Paragraph::new(Line::from(overlays::hint_spans(&buttons))),
            Rect {
                y,
                height: 1,
                ..inner
            },
        );
    }
}

impl View for SettingsView {
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, hits: &mut HitMap) {
        self.regions.clear();
        let [title, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        let [nav, form] =
            Layout::horizontal([Constraint::Length(NAV_W), Constraint::Fill(1)]).areas(body);
        self.draw_title(f, title, ctx, hits);
        self.draw_nav(f, nav);
        self.draw_form(f, form);
        if self.confirm {
            self.draw_confirm(f, area, hits);
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if self.confirm {
            return self.confirm_key(key, ctx);
        }
        if self.edit.is_some() {
            return self.edit_key(key, ctx);
        }
        let n = self.rows().len();
        match key.code {
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
            KeyCode::Tab => self.set_section(self.section + 1),
            KeyCode::BackTab => self.set_section(self.section + SECTIONS.len() - 1),
            KeyCode::Char(c @ '1'..='3') => self.set_section(c as usize - '1' as usize),
            KeyCode::Enter | KeyCode::Char(' ') => return self.activate(ctx),
            KeyCode::Left | KeyCode::Char('h') => return self.cycle(ctx, false),
            KeyCode::Right | KeyCode::Char('l') => return self.cycle(ctx, true),
            KeyCode::Char('a') => self.add(),
            KeyCode::Char('x') | KeyCode::Delete => self.remove(ctx),
            KeyCode::Char('K') => self.move_item(true),
            KeyCode::Char('J') => self.move_item(false),
            _ => {}
        }
        Action::None
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if self.confirm {
            return Action::None;
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
        let mut h = vec![("↑↓", "field"), ("⇥", "section")];
        let row = self.current();
        match row.as_ref().map(|r| &r.kind) {
            Some(Kind::Text { .. } | Kind::Number { .. }) => h.push(("⏎", "edit")),
            Some(Kind::ListItem) => h.extend([("⏎", "edit"), ("x", "remove"), ("K/J", "reorder")]),
            Some(Kind::Pick(_)) => h.push(("←→", "choose")),
            Some(Kind::Toggle) => h.push(("⏎", "toggle")),
            Some(Kind::Add) => h.push(("⏎", "add")),
            Some(Kind::Group) | None => {}
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
            Section::General => {}
        }
        h.push((
            "s",
            if self.dirty() {
                "save ● unsaved"
            } else {
                "save"
            },
        ));
        h.push(("esc", "back"));
        h
    }

    fn refresh(&mut self, ctx: &Ctx) {
        if !self.dirty() && self.edit.is_none() && !self.confirm {
            self.base = ctx.store.config.clone();
            self.draft = ctx.store.config.clone();
            self.select(self.sel);
            self.revalidate(ctx);
        }
    }

    fn captures_text(&self) -> bool {
        self.edit.is_some()
    }
}
