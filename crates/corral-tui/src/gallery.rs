//! Recent and pinned visual outputs. Images are cached at refresh, never decoded during drawing.
use std::collections::HashMap;

use anyhow::Result;
use corral_core::{PinRef, PinTarget};
use corral_ops::{ArtifactRow, Ctx};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui_image::protocol::StatefulProtocol;

use crate::artifacts::{image, ui};
use crate::overlays::truncate;
use crate::{Action, Hit, HitMap, MouseCtx, View, hit, shelf, theme};

const CARD: u8 = 0;
const RECENT: usize = 40;

struct Tile {
    row: ArtifactRow,
    pin: Option<String>,
    image: Result<Box<StatefulProtocol>, String>,
}

pub struct GalleryView {
    tiles: Vec<Tile>,
    matches: Vec<usize>,
    selected: usize,
    columns: usize,
    page: usize,
    query: String,
    pinned_only: bool,
}

impl GalleryView {
    pub fn new(ctx: &Ctx) -> Result<Self> {
        let mut view = Self {
            tiles: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            columns: 1,
            page: 1,
            query: String::new(),
            pinned_only: false,
        };
        view.load(ctx)?;
        Ok(view)
    }

    fn load(&mut self, ctx: &Ctx) -> Result<()> {
        let owners = ctx.store.artifact_owners()?;
        let mut pins = HashMap::new();
        for pin in ctx.store.pins()? {
            match corral_core::resolve_pin(&pin, &owners, ctx.store.dir()) {
                PinTarget::Artifact { artifact, .. } => {
                    pins.insert(artifact.path, pin.target);
                }
                PinTarget::Task { .. } | PinTarget::Broken(_) => {}
            }
        }
        let mut recent = 0;
        let rows = corral_ops::all_artifacts(ctx)?;
        let mut cached: HashMap<_, _> = std::mem::take(&mut self.tiles)
            .into_iter()
            .filter_map(|tile| tile.image.ok().map(|image| (tile.row.artifact.path, image)))
            .collect();
        self.tiles = rows
            .into_iter()
            .filter(|row| corral_ops::is_visual_artifact(&row.artifact))
            .filter_map(|row| {
                let pin = pins.get(&row.artifact.path).cloned();
                if pin.is_none() {
                    recent += 1;
                    if recent > RECENT {
                        return None;
                    }
                }
                let image = if let Some(image) = cached.remove(&row.artifact.path) {
                    Ok(image)
                } else {
                    corral_ops::artifact_thumbnail(ctx, &row.artifact)
                        .map_err(|error| format!("{error:#}"))
                        .and_then(|path| {
                            path.ok_or_else(|| {
                                "HTML preview unavailable; Enter opens original".into()
                            })
                        })
                        .and_then(|path| {
                            image::thumbnail(&path.to_string_lossy())
                                .map(Box::new)
                                .map_err(|error| format!("Preview unavailable: {error:#}"))
                        })
                };
                Some(Tile { row, pin, image })
            })
            .collect();
        // Pins stay at the front without disturbing the newest-first order inside each group.
        self.tiles.sort_by_key(|tile| tile.pin.is_none());
        self.filter();
        Ok(())
    }

    fn filter(&mut self) {
        let query = self.query.to_lowercase();
        self.matches = self
            .tiles
            .iter()
            .enumerate()
            .filter(|(_, tile)| !self.pinned_only || tile.pin.is_some())
            .filter(|(_, tile)| {
                let row = &tile.row;
                let text = format!(
                    "{} {} {} {}",
                    row.task_id,
                    row.task_title,
                    row.artifact.title,
                    row.project.as_deref().unwrap_or("")
                )
                .to_lowercase();
                query.split_whitespace().all(|word| text.contains(word))
            })
            .map(|(index, _)| index)
            .collect();
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
    }

    fn move_by(&mut self, amount: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(amount)
            .min(self.matches.len().saturating_sub(1));
    }

    fn tile(&self) -> Option<&Tile> {
        self.matches
            .get(self.selected)
            .and_then(|i| self.tiles.get(*i))
    }
}

impl View for GalleryView {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        ui::paint_bg(f, area);
        let [header, search, body] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Fill(1),
        ])
        .areas(area);
        let label = if self.pinned_only {
            "pinned"
        } else {
            "recent + pinned"
        };
        let title = format!(
            " Gallery / {label}  {} visuals  {}/{} ",
            self.matches.len(),
            if self.matches.is_empty() {
                0
            } else {
                self.selected + 1
            },
            self.matches.len()
        );
        f.render_widget(Paragraph::new(title).style(theme::bold()), header);
        let mut spans = vec![Span::styled(" Filter: ", Style::new().fg(theme::CYAN))];
        spans.extend(ui::input_spans(&self.query, "task, project or title"));
        f.render_widget(Paragraph::new(Line::from(spans)), search);
        if self.matches.is_empty() {
            let message = if !self.query.is_empty() {
                "No matching visuals. Clear the filter with Backspace."
            } else if self.pinned_only {
                "No pinned visuals. Tab shows recent images and HTML reports; * pins one."
            } else {
                "No visual artifacts yet. Attach an image or HTML report with corral artifact add."
            };
            f.render_widget(
                Paragraph::new(message).wrap(ratatui::widgets::Wrap { trim: false }),
                body,
            );
            return;
        }
        self.columns = usize::from((body.width / 36).clamp(1, 4));
        let rows = usize::from((body.height / 8).clamp(1, 3))
            .min(self.matches.len().div_ceil(self.columns));
        self.page = self.columns * rows;
        let start = self.selected / self.page * self.page;
        let row_count = u16::try_from(rows).unwrap_or(3);
        let grid = Rect {
            height: body
                .height
                .min(row_count.saturating_mul(15).saturating_sub(1)),
            ..body
        };
        let row_areas = Layout::vertical(vec![Constraint::Fill(1); rows])
            .spacing(1)
            .split(grid);
        for (row_index, row_area) in row_areas.iter().enumerate() {
            let cells = Layout::horizontal(vec![Constraint::Fill(1); self.columns])
                .spacing(1)
                .split(*row_area);
            for (column, cell) in cells.iter().enumerate() {
                let index = start + row_index * self.columns + column;
                let Some(&tile_index) = self.matches.get(index) else {
                    break;
                };
                let tile = &mut self.tiles[tile_index];
                let selected = index == self.selected;
                let marker = if selected { "> " } else { "" };
                let pin = if tile.pin.is_some() { "* " } else { "" };
                let title = truncate(
                    &format!("{marker}{pin}{}", tile.row.artifact.title),
                    usize::from(cell.width.saturating_sub(4)),
                );
                let inner = ui::boxed(f, *cell, &title, selected, None);
                hits.push(*cell, Hit::Item(CARD, index));
                let [picture, caption] =
                    Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
                match &mut tile.image {
                    Ok(state) => image::draw(f, picture, state),
                    Err(error) => f.render_widget(
                        Paragraph::new(error.as_str())
                            .style(theme::dim())
                            .wrap(ratatui::widgets::Wrap { trim: false }),
                        picture,
                    ),
                }
                let row = &tile.row;
                let text = truncate(
                    &format!(
                        "{} / {} / {}",
                        row.task_id,
                        row.artifact.run,
                        row.project.as_deref().unwrap_or("workspace")
                    ),
                    usize::from(caption.width),
                );
                f.render_widget(Paragraph::new(Span::styled(text, theme::dim())), caption);
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if !ui::is_press(&key) {
            return Action::None;
        }
        match key.code {
            KeyCode::Esc => return Action::Back,
            KeyCode::Left => self.move_by(-1),
            KeyCode::Right => self.move_by(1),
            KeyCode::Up => self.move_by(-(self.columns as isize)),
            KeyCode::Down => self.move_by(self.columns as isize),
            KeyCode::PageUp => self.move_by(-(self.page as isize)),
            KeyCode::PageDown => self.move_by(self.page as isize),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = self.matches.len().saturating_sub(1),
            KeyCode::Tab => {
                self.pinned_only = !self.pinned_only;
                self.selected = 0;
                self.filter();
            }
            KeyCode::Enter => {
                if let Some(tile) = self.tile() {
                    return Action::Toast(ui::open_toast(ctx, &tile.row.artifact));
                }
            }
            KeyCode::Char('*') => {
                if let Some(tile) = self.tile() {
                    let target = tile.pin.clone().unwrap_or_else(|| {
                        PinRef::of_artifact(&tile.row.task_id, &tile.row.artifact).to_string()
                    });
                    return shelf::toggle(ctx, &target);
                }
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Action::OpenArtifacts;
            }
            _ => {
                if ui::edit_text(&mut self.query, &key) {
                    self.selected = 0;
                    self.filter();
                }
            }
        }
        Action::None
    }

    fn on_mouse(&mut self, event: MouseEvent, mouse: &MouseCtx) -> Action {
        if let Some(direction) = hit::wheel(&event) {
            self.move_by(direction * self.columns as isize);
        } else if let Some(Hit::Item(CARD, index)) = mouse.hit
            && hit::left(&event)
        {
            self.selected = index;
            if mouse.double {
                return self.on_key(hit::press(KeyCode::Enter), mouse.ctx);
            }
        }
        Action::None
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("arrows", "select"),
            ("tab", if self.pinned_only { "recent" } else { "pinned" }),
            ("*", "pin"),
            ("enter", "open original"),
            ("pgup/pgdn", "page"),
            ("ctrl+a", "finder"),
            ("esc", "back"),
        ]
    }

    fn refresh(&mut self, ctx: &Ctx) {
        let keep = self.tile().map(|tile| tile.row.artifact.path.clone());
        if let Err(error) = self.load(ctx) {
            corral_ops::report_message(None, "Gallery refresh", format!("{error:#}"));
            return;
        }
        if let Some(path) = keep
            && let Some(index) = self
                .matches
                .iter()
                .position(|i| self.tiles[*i].row.artifact.path == path)
        {
            self.selected = index;
        }
    }

    fn captures_text(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
