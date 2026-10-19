//! Frame composition and toast rendering.
use super::*;

impl App {
    pub(crate) fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        self.hits.clear();
        f.render_widget(
            Block::new().style(Style::new().bg(theme::BG).fg(theme::FG)),
            area,
        );
        let with_header = self.mode == Mode::Board;
        let mut hints = self.stack.last().map(|v| v.hints()).unwrap_or_default();
        if self.mode == Mode::Board
            && self.stack.len() == 1
            && !self.stack.last().is_some_and(|v| v.captures_text())
        {
            hints.insert(1, ("M", "messages"));
        }
        let back = (self.mode == Mode::Board && self.stack.len() == 1)
            .then(|| footer::back_label(&self.nav));
        let keys = self
            .stack
            .last()
            .is_some_and(|v| v.board_shortcuts())
            .then_some(&self.ctx.store.config.ui);
        let bar = Footer::new(&hints, area.width, back).with_keys(keys);
        let automation_height = if with_header {
            self.automation_header_height(area.width)
        } else {
            0
        };
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(if with_header {
                2 + automation_height
            } else {
                0
            }),
            Constraint::Fill(1),
            Constraint::Length(bar.height()),
        ])
        .areas(area);
        if with_header {
            let [summary, resources] = Layout::vertical([
                Constraint::Length(1 + automation_height),
                Constraint::Length(1),
            ])
            .areas(header);
            let counters = self.draw_header(f, summary);
            f.render_widget(
                Paragraph::new(self.resources.line()).style(Style::new().bg(theme::BG2)),
                resources,
            );
            if self.stack.len() == 1 {
                for (rect, hit) in counters {
                    self.hits.push(rect, hit);
                }
            }
        }
        let base = self
            .stack
            .iter()
            .rposition(|v| !v.is_overlay())
            .unwrap_or(0);
        for (i, view) in self.stack.iter_mut().enumerate().skip(base) {
            if i > base || view.is_overlay() {
                overlays::dim_backdrop(f.buffer_mut(), body);
            }
            view.draw(f, body, &self.ctx, &mut self.hits);
        }
        bar.draw(f, footer, &mut self.hits);
        draw_toasts(&self.toasts, f, area, &mut self.hits);
        theme::apply_accent(f.buffer_mut(), self.ctx.store.config.ui.accent);
    }
}

/// Draws the toasts newest first and makes each one clickable ([`Hit::Toast`], on top of everything).
fn draw_toasts(toasts: &[(Toast, Instant)], f: &mut Frame, area: Rect, hits: &mut HitMap) {
    let w = area.width.min(48);
    if w < 12 {
        return;
    }
    let mut y = area.y + 2;
    for (i, (t, _)) in toasts.iter().enumerate().rev() {
        if y + 4 > area.bottom() {
            break;
        }
        let rect = Rect::new(area.right().saturating_sub(w + 1), y, w, 4);
        hits.push(rect, Hit::Toast(i));
        let color = tone_color(t.tone);
        let block = Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(color))
            .style(Style::new().bg(theme::BG2))
            .title(Line::from(format!(" F{} ", 8 + toasts.len() - 1 - i)).right_aligned());
        let inner = block.inner(rect);
        let width = inner.width as usize;
        f.render_widget(Clear, rect);
        f.render_widget(block, rect);
        let lines = vec![
            Line::styled(
                truncate(&t.title, width),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            ),
            Line::styled(truncate(&t.body, width), theme::dim()),
        ];
        f.render_widget(Paragraph::new(lines), inner);
        y += 4;
    }
}

fn tone_color(tone: Tone) -> ratatui::style::Color {
    match tone {
        Tone::Info => theme::CYAN,
        Tone::Good => theme::GREEN,
        Tone::Warn => theme::ORANGE,
        Tone::Bad => theme::RED,
        Tone::Attention => theme::YELLOW,
    }
}
