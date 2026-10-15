//! Session message history. Repeats are keyed by operation and task or file, not by toast lifetime.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use corral_ops::{Ctx, Message};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::artifacts::ui::{self, Scroll};
use crate::{Action, HitMap, View, overlays, theme};

const MAX_MESSAGES: usize = 200;

#[derive(Default)]
pub(super) struct MessageLog {
    latest: HashMap<(Option<String>, Option<PathBuf>, &'static str), String>,
    entries: VecDeque<Message>,
}

impl MessageLog {
    pub(super) fn record(&mut self, message: Message) -> bool {
        let key = (message.task.clone(), message.file.clone(), message.source);
        if self.latest.get(&key) == Some(&message.text) {
            return false;
        }
        self.latest.insert(key, message.text.clone());
        self.entries.push_back(message);
        if self.entries.len() > MAX_MESSAGES {
            self.entries.pop_front();
        }
        true
    }

    pub(super) fn view(&self) -> MessageView {
        MessageView {
            entries: self.entries.iter().rev().cloned().collect(),
            scroll: Scroll::default(),
        }
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

pub(super) struct MessageView {
    entries: Vec<Message>,
    scroll: Scroll,
}

impl View for MessageView {
    fn draw(&mut self, f: &mut Frame, area: Rect, _: &Ctx, hits: &mut HitMap) {
        let area = overlays::centered(area, 100, area.height.saturating_sub(2));
        let inner = overlays::popup(f, area, "Board messages - newest first", hits);
        let mut lines = Vec::new();
        if self.entries.is_empty() {
            lines.push(Line::styled("No messages this session.", theme::dim()));
        }
        for entry in &self.entries {
            lines.push(Line::styled(
                format!(
                    "{} / {}",
                    entry.task.as_deref().unwrap_or("Board"),
                    entry.source
                ),
                theme::dim(),
            ));
            lines.extend(entry.text.lines().map(|line| Line::raw(line.to_owned())));
            lines.push(Line::raw(""));
        }
        ui::scrolled(f, inner, lines, &mut self.scroll, true);
    }

    fn on_key(&mut self, key: KeyEvent, _: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('M' | 'q') => return Action::Back,
            KeyCode::End => self.scroll.offset = usize::MAX,
            _ => {
                self.scroll.on_key(&key, 10);
            }
        }
        Action::None
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![("j/k", "scroll"), ("esc", "back")]
    }

    fn is_overlay(&self) -> bool {
        true
    }
}
