//! Lazy task timeline and its read-only raw-event overlay.

use corral_ops::{Ctx, EntryClass, TimelineEntry, TimelineFilter};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::artifacts::list::truncate;
use crate::artifacts::ui::{self, Scroll};
use crate::hit;
use crate::theme;
use crate::{Action, HitMap, MouseCtx, Toast, Tone};

pub(super) const Z_TIMELINE: u8 = 6;
const FIRST: usize = 2;

#[derive(Default)]
pub(super) struct TimelineTab {
    entries: Option<Result<Vec<TimelineEntry>, String>>,
    all: bool,
    sel: usize,
    reveal: bool,
    overlay: Option<RawEvent>,
}

struct RawEvent {
    title: String,
    lines: Vec<Line<'static>>,
    scroll: Scroll,
}

impl TimelineTab {
    pub(super) fn load(&mut self, ctx: &Ctx, id: &str) {
        if self.entries.is_none() {
            self.entries = Some(
                corral_ops::timeline(&ctx.store, id, TimelineFilter::All)
                    .map_err(|e| format!("{e:#}")),
            );
            self.sel = self.sel.min(self.visible().len().saturating_sub(1));
        }
        self.reveal = true;
    }

    pub(super) fn reset(&mut self) {
        self.entries = None;
    }

    fn visible(&self) -> Vec<&TimelineEntry> {
        match &self.entries {
            Some(Ok(entries)) => entries
                .iter()
                .filter(|entry| self.all || entry.class != EntryClass::Noise)
                .collect(),
            None | Some(Err(_)) => Vec::new(),
        }
    }

    pub(super) fn lines(&self, width: usize) -> Vec<Line<'static>> {
        let note = match &self.entries {
            None => "Loading timeline...".to_owned(),
            Some(Err(error)) => format!("Timeline unavailable: {error}"),
            Some(Ok(_)) => format!(
                "{} events · a {} · ⏎ raw event",
                if self.all { "All" } else { "Relevant" },
                if self.all { "hide noise" } else { "all events" },
            ),
        };
        let mut lines = vec![Line::styled(truncate(&note, width), theme::dim())];
        let cols = [17, 14, 10, 9, 5];
        let heading = ["time", "kind", "actor", "via", "run"];
        let mut spans: Vec<_> = heading
            .iter()
            .zip(cols)
            .map(|(text, w)| Span::styled(ui::fit(text, w), theme::dim()))
            .collect();
        spans.push(Span::styled("summary", theme::dim()));
        lines.push(Line::from(spans));
        let entries = self.visible();
        if entries.is_empty() && matches!(self.entries, Some(Ok(_))) {
            lines.push(Line::styled(
                truncate("No task events yet. Refresh to check again.", width),
                theme::dim(),
            ));
        }
        for (i, entry) in entries.iter().enumerate() {
            let style = match entry.class {
                EntryClass::Lifecycle => Style::new().fg(theme::FG),
                EntryClass::Review | EntryClass::Run | EntryClass::Noise => theme::dim(),
            };
            let style = if i == self.sel {
                style.bg(theme::SELECTED_BG)
            } else {
                style
            };
            let time = ui::local_date(entry.at, "%Y-%m-%d %H:%M");
            let values = [
                time.as_str(),
                entry.kind.as_str(),
                entry.actor.as_deref().unwrap_or("?"),
                entry.via.as_deref().unwrap_or("?"),
                entry.run.as_deref().unwrap_or("-"),
            ];
            let mut spans: Vec<_> = values
                .iter()
                .zip(cols)
                .map(|(text, w)| {
                    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    Span::styled(ui::fit(&truncate(&text, w - 1), w), style)
                })
                .collect();
            let room = width.saturating_sub(cols.iter().sum());
            if room > 0 {
                spans.push(Span::styled(truncate(&entry.summary, room), style));
            }
            lines.push(Line::from(spans).style(style));
        }
        lines
    }

    pub(super) fn follow_selection(&mut self, scroll: &mut Scroll, height: u16) {
        if !self.reveal {
            return;
        }
        let row = FIRST + self.sel;
        if row < scroll.offset {
            scroll.offset = row.saturating_sub(FIRST);
        } else if row >= scroll.offset.saturating_add(usize::from(height)) {
            scroll.offset = row.saturating_add(1).saturating_sub(usize::from(height));
        }
        self.reveal = false;
    }

    pub(super) fn hits(&self, hits: &mut HitMap, area: Rect, offset: usize) {
        hit::rows(hits, area, Z_TIMELINE, FIRST, self.visible().len(), offset);
    }

    pub(super) fn overlay_open(&self) -> bool {
        self.overlay.is_some()
    }

    pub(super) fn draw_overlay(&mut self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        let Some(raw) = &mut self.overlay else {
            return;
        };
        let rect = ui::centered(
            area,
            area.width.saturating_sub(4),
            area.height.saturating_sub(4),
        );
        let inner = ui::popup(f, rect, &raw.title, None);
        hit::modal_popup(f, rect, hits);
        ui::scrolled(f, inner, raw.lines.clone(), &mut raw.scroll, true);
    }

    pub(super) fn on_key(&mut self, key: &KeyEvent, scroll: &mut Scroll) -> Option<Action> {
        if let Some(raw) = &mut self.overlay {
            if key.code == KeyCode::Esc {
                self.overlay = None;
            } else {
                raw.scroll.on_key(key, 10);
            }
            return Some(Action::None);
        }
        match key.code {
            KeyCode::Char('a') => {
                self.all = !self.all;
                self.sel = 0;
                *scroll = Scroll::default();
            }
            KeyCode::Enter => {
                let entries = self.visible();
                let entry = entries.get(self.sel)?;
                match serde_json::to_string_pretty(&entry.event) {
                    Ok(json) => {
                        self.overlay = Some(RawEvent {
                            title: format!("Raw event · {}", entry.kind),
                            lines: json
                                .lines()
                                .map(|line| Line::raw(line.to_owned()))
                                .collect(),
                            scroll: Scroll::default(),
                        });
                    }
                    Err(error) => {
                        return Some(Action::Toast(Toast::new(
                            Tone::Bad,
                            "Could not display event",
                            error.to_string(),
                        )));
                    }
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.sel = self
                    .sel
                    .saturating_add(1)
                    .min(self.visible().len().saturating_sub(1));
                self.reveal = true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.sel = self.sel.saturating_sub(1);
                self.reveal = true;
            }
            _ => return None,
        }
        Some(Action::None)
    }

    pub(super) fn mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Option<Action> {
        if let Some(raw) = &mut self.overlay {
            if let Some(dir) = hit::wheel(&ev) {
                raw.scroll.wheel(dir);
            }
            return Some(Action::None);
        }
        if hit::left(&ev)
            && let Some(hit::Hit::Item(Z_TIMELINE, i)) = m.hit
        {
            let open = m.double || i == self.sel;
            self.sel = i;
            if open {
                return self.on_key(&hit::press(KeyCode::Enter), &mut Scroll::default());
            }
            return Some(Action::None);
        }
        None
    }
}
