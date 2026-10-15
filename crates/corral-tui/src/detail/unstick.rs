use corral_core::StuckProcess;
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{DetailView, load_live};
use crate::artifacts::ui;
use crate::hit;
use crate::theme::{self, FG, RED};
use crate::{Action, HitMap, Toast, Tone};

const WIDTH: u16 = 74;
const ARGV_LINES: usize = 4;

impl DetailView {
    /// ⏎/y kills the process captured when the popup opened (ops re-checks it is still that exact process),
    /// esc/n cancels.
    pub(super) fn on_unstick_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') => {
                self.confirm = None;
                Action::None
            }
            KeyCode::Enter | KeyCode::Char('y') => {
                let Some(expected) = self.confirm.take() else {
                    return Action::None;
                };
                let id = self.task.id.clone();
                match corral_ops::unstick(ctx, &id, &expected) {
                    Ok(task) => {
                        self.task = task;
                        self.live = load_live(ctx, &self.task);
                        Action::Toast(Toast::new(
                            Tone::Good,
                            format!("⚡ {id} unstuck"),
                            format!("killed pid {}: {}", expected.pid, expected.argv.join(" ")),
                        ))
                    }
                    Err(e) => Action::Toast(Toast::new(
                        Tone::Bad,
                        format!("unstick {id} failed"),
                        format!("{e:#}"),
                    )),
                }
            }
            _ => Action::None,
        }
    }

    pub(super) fn draw_unstick(
        &self,
        f: &mut Frame,
        area: Rect,
        s: &StuckProcess,
        hits: &mut HitMap,
    ) {
        let run = self.task.current_run().map_or("", |r| r.id.as_str());
        let width = WIDTH.min(area.width);
        let mut lines = vec![Line::from(Span::styled(
            format!(" Unstick run {run}? Kill pid {}:", s.pid),
            Style::new().fg(RED),
        ))];
        lines.extend(
            argv_lines(&s.argv.join(" "), usize::from(width.saturating_sub(5)))
                .into_iter()
                .map(|l| Line::from(Span::styled(format!("   {l}"), Style::new().fg(FG)))),
        );
        lines.push(Line::from(Span::styled(
            " SIGTERM now; SIGKILL after 3s if still running.",
            theme::dim(),
        )));
        lines.push(Line::from(Span::styled(
            format!(
                " started {}; refused if the process changed since.",
                s.started
            ),
            theme::dim(),
        )));
        let buttons_at = lines.len() as u16 + 1;
        lines.extend([Line::default(), Line::default()]);
        let rect = ui::centered(area, width, lines.len() as u16 + 2);
        let inner = ui::popup(f, rect, &format!("unstick {}", self.task.id), None);
        hit::modal_popup(f, rect, hits);
        f.render_widget(Paragraph::new(lines).style(Style::new().fg(FG)), inner);
        if inner.height > buttons_at {
            let buttons = [
                ("Unstick", hit::press(KeyCode::Enter)),
                ("Cancel", hit::press(KeyCode::Esc)),
            ];
            hit::buttons(
                f,
                hits,
                inner.x + 1,
                inner.y + buttons_at,
                inner.right(),
                &buttons,
            );
        }
    }
}

fn argv_lines(text: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = chars
        .chunks(width.max(1))
        .map(|c| c.iter().collect())
        .collect();
    if out.len() > ARGV_LINES {
        out.truncate(ARGV_LINES);
        if let Some(last) = out.last_mut() {
            last.pop();
            last.push('…');
        }
    }
    out
}
