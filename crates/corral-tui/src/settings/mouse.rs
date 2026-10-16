//! Settings hit targets and click handling, plus form scrolling shared with PageUp/PageDown.

use corral_ops::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::SettingsView;
use super::fields::Target;
use super::model::SECTIONS;
use crate::{Action, View};

const WHEEL_LINES: usize = 3;

impl SettingsView {
    pub(super) fn target_at(&self, x: u16, y: u16) -> Option<Target> {
        self.regions
            .iter()
            .rev()
            .find(|(r, _)| r.contains((x, y).into()))
            .map(|(_, t)| *t)
    }

    pub(super) fn click(&mut self, target: Target, ctx: &Ctx) -> Action {
        let row_of = |t: Target| match t {
            Target::Row(i)
            | Target::Pill(i, _)
            | Target::Up(i)
            | Target::Down(i)
            | Target::Remove(i)
            | Target::Activate(i) => Some(i),
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
            Target::SaveLayer(layer) => self.save_target = layer,
            Target::Save => {
                return match self.save(ctx) {
                    Ok(t) | Err(t) => Action::Toast(t),
                };
            }
            Target::Section(i) => self.set_section(i),
            Target::Row(i) if i == self.sel => {
                return self.on_key(KeyEvent::from(KeyCode::Enter), ctx);
            }
            Target::Row(i) => self.select(i),
            Target::Activate(i) => {
                self.select(i);
                return self.on_key(KeyEvent::from(KeyCode::Enter), ctx);
            }
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
            Target::Form | Target::SectionList | Target::PickerRow(_) | Target::PickerList => {}
        }
        Action::None
    }

    pub(super) fn wheel(&mut self, target: Target, down: bool) {
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
}
