use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Focus, Mode, ReviewView, Z_ART, Z_ARTS, Z_FILE, Z_FILES, Z_STRATEGY, Z_SUMMARY, ui};
use crate::artifacts::list::Row;
use crate::hit::{self, Hit};
use crate::theme::{self, FG, RED};
use crate::{Action, HitMap, MouseCtx, Toast, Tone};

impl ReviewView {
    /// Mouse on the review screen. Buttons, `‹ back` and popup buttons are key hits the app resolves; this
    /// handles the rest: wheel scrolls the box under the pointer, a click focuses a box, a file row jumps the
    /// diff to that file, an artifact row selects (a click on the selected one, or a double-click, opens it),
    /// and a merge-sheet row picks that strategy.
    pub(super) fn mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        match &mut self.mode {
            Mode::Preview(p) => {
                if let Some(dir) = hit::wheel(&ev) {
                    p.wheel(dir);
                }
                return Action::None;
            }
            Mode::Merge(sheet) => {
                if let (true, Some(Hit::Item(Z_STRATEGY, i))) = (hit::left(&ev), &m.hit) {
                    sheet.opt = *i;
                }
                return Action::None;
            }
            Mode::ConfirmDiscard => return Action::None,
            Mode::Retry(c) => return c.on_mouse(&ev, m).unwrap_or(Action::None),
            Mode::Changes(_) | Mode::Normal => {}
        }
        if let Some(dir) = hit::wheel(&ev) {
            match m.hit {
                Some(Hit::Item(Z_SUMMARY, _)) => self.summary_scroll.wheel(dir),
                Some(Hit::Item(Z_FILES | Z_FILE, _)) => self.files_scroll.wheel(dir),
                Some(Hit::Item(Z_ARTS | Z_ART, _)) => self.move_art(dir),
                _ => {}
            }
            return Action::None;
        }
        if !hit::left(&ev) || !matches!(self.mode, Mode::Normal) {
            return Action::None;
        }
        match m.hit {
            Some(Hit::Item(Z_SUMMARY, _)) => self.focus = Focus::Summary,
            Some(Hit::Item(Z_FILES, _)) => self.focus = Focus::Files,
            Some(Hit::Item(Z_FILE, i)) => {
                self.focus = Focus::Files;
                self.jump_to_file(i);
            }
            Some(Hit::Item(Z_ARTS, _)) => self.focus = Focus::Artifacts,
            Some(Hit::Item(Z_ART, r)) => {
                let was_focused = self.focus == Focus::Artifacts;
                self.focus = Focus::Artifacts;
                match self.list.rows(&self.info.artifacts).get(r) {
                    Some(Row::Fold) => self
                        .list
                        .toggle_fold(&self.info.artifacts, &mut self.art_sel),
                    Some(Row::Item(i)) => {
                        let (i, open) = (*i, was_focused && *i == self.art_sel || m.double);
                        self.art_sel = i;
                        if open {
                            return self.on_normal_key(&hit::press(KeyCode::Enter), m.ctx);
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
        Action::None
    }

    fn move_art(&mut self, dir: isize) {
        self.list.step(&self.info.artifacts, &mut self.art_sel, dir);
    }

    /// Scrolls the Files box so the diff of file `i` starts at the top.
    pub(super) fn jump_to_file(&mut self, i: usize) {
        let Some(file) = self.info.stat.files.get(i) else {
            return;
        };
        let suffix = format!(" b/{}", file.path);
        let at = self
            .info
            .diff
            .lines()
            .position(|l| l.starts_with("diff --git") && l.ends_with(&suffix));
        if let Some(at) = at {
            self.file_sel = Some(i);
            self.files_scroll.offset = self.info.stat.files.len() + 1 + at;
        }
    }

    pub(super) fn on_confirm_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') => {
                self.mode = Mode::Normal;
                Action::None
            }
            KeyCode::Enter | KeyCode::Char('y') => {
                self.mode = Mode::Normal;
                let id = self.id();
                match corral_ops::discard(ctx, &id) {
                    Ok(_) => Action::BackWithToast(Toast::new(
                        Tone::Warn,
                        format!("✗ {id} discarded"),
                        "branch, worktree and artifacts kept; card moved to dropped",
                    )),
                    Err(e) => Action::Toast(Toast::new(
                        Tone::Bad,
                        format!("discard {id} failed"),
                        format!("{e:#}"),
                    )),
                }
            }
            _ => Action::None,
        }
    }

    pub(super) fn draw_confirm(&self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        let lines = vec![
            Line::from(Span::styled(
                format!(" Discard run {}?", self.info.run.id),
                Style::new().fg(RED),
            )),
            Line::from(Span::styled(
                " Branch, worktree and artifacts are kept; the card moves to dropped.",
                theme::dim(),
            )),
            Line::default(),
            Line::default(),
        ];
        let rect = ui::centered(area, 74, lines.len() as u16 + 2);
        let inner = ui::popup(f, rect, &format!("discard {}", self.info.task.id), None);
        hit::modal_popup(f, rect, hits);
        f.render_widget(Paragraph::new(lines).style(Style::new().fg(FG)), inner);
        if inner.height >= 4 {
            let buttons = [
                ("Discard", hit::press(KeyCode::Enter)),
                ("Cancel", hit::press(KeyCode::Esc)),
            ];
            hit::buttons(f, hits, inner.x + 1, inner.y + 3, inner.right(), &buttons);
        }
    }
}
