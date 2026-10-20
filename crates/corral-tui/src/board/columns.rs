//! Column and card layout.
use super::*;

impl BoardView {
    /// The `○ ideas` / `○ done` switch drawn on a column's top border after its title; a click presses its key.
    fn draw_toggle(&self, f: &mut Frame, area: Rect, col: Status, right_w: u16, hits: &mut HitMap) {
        let (key, on, label) = match col {
            Status::Backlog => ('i', self.ideas_only, "ideas"),
            _ if self.columns().last() == Some(&col) => ('D', self.show_done, "done"),
            _ => return,
        };
        let text = format!(" {} {label} ", if on { "◉" } else { "○" });
        let x = area.x + col.label().width() as u16 + 4;
        let w = text.width() as u16;
        if x + w + right_w + 1 >= area.right() {
            return;
        }
        let style = if on {
            Style::new().fg(theme::CYAN).add_modifier(Modifier::BOLD)
        } else {
            theme::dim()
        };
        let rect = Rect::new(x, area.y, w, 1);
        f.render_widget(Span::styled(text, style), rect);
        hits.push(
            rect,
            Hit::Key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE)),
        );
    }

    pub(super) fn draw_column(
        &mut self,
        f: &mut Frame,
        area: Rect,
        col: Status,
        config: &Config,
        now: Timestamp,
        hits: &mut HitMap,
    ) {
        let focused = self.columns().get(self.col) == Some(&col);
        let title = col.label().to_string();
        let tasks = self.column_tasks(col);
        let right = match col {
            Status::Running if config.ui.show_wip => {
                let usage = SlotUsage::from_tasks(&self.tasks, &config.limits);
                let avail = (area.width as usize).saturating_sub(title.width() + 6);
                let style = if usage.total() >= config.limits.running {
                    Style::new().fg(theme::ORANGE)
                } else {
                    theme::dim()
                };
                Span::styled(slot_badge(&usage, &config.limits, avail), style)
            }
            Status::NeedsMe if !tasks.is_empty() => {
                Span::styled(" ● ", Style::new().fg(theme::YELLOW))
            }
            _ => Span::styled(format!(" {} ", tasks.len()), theme::dim()),
        };
        let right = if config.board.card.column_cost {
            let sum = corral_core::cost_sum_label(tasks.iter().copied());
            let text = format!(" {sum} {}", right.content.trim_start());
            if area.width as usize >= title.len() + text.len() + 4 {
                Span::styled(text, right.style)
            } else {
                right
            }
        } else {
            right
        };
        let block = if focused {
            overlays::focused_block(title)
        } else {
            theme::titled(title)
        };
        let drop_ok = self.drop_target(col);
        let block = match drop_ok {
            Some(true) => block.border_style(Style::new().fg(theme::GREEN)),
            _ => block,
        };
        let right_w = right.width() as u16;
        let block = block.title(Line::from(right).right_aligned());
        let inner = block.inner(area);
        f.render_widget(block, area);
        hits.push(area, Hit::Column(col));
        self.draw_toggle(f, area, col, right_w, hits);
        if inner.width < 4 || inner.height == 0 {
            return;
        }
        let older = if col == Status::Done {
            self.done_cards().1
        } else {
            0
        };
        let inner = if older > 0 && inner.height > 1 {
            let foot = Rect {
                y: inner.bottom() - 1,
                height: 1,
                ..inner
            };
            draw_older(f, foot, older, hits);
            Rect {
                height: inner.height - 1,
                ..inner
            }
        } else {
            inner
        };
        if tasks.is_empty() {
            f.render_widget(Paragraph::new(Line::styled(" —", theme::dim())), inner);
            return;
        }
        let cw = inner.width as usize - 2;
        let sel_row = if focused {
            Some(self.row.min(tasks.len() - 1))
        } else {
            None
        };
        // Cards are pure functions of the task; build only those the scroll and frame reach.
        let mut built: Vec<Option<Vec<Line<'static>>>> = vec![None; tasks.len()];
        let mut card = |i: usize| -> Vec<Line<'static>> {
            built[i]
                .get_or_insert_with(|| {
                    let t = tasks[i];
                    let extra = CardExtra {
                        plan_progress: self.plan_progress.get(&t.id),
                        finish: self.finishes.get(&t.id),
                        pinned: self.pinned.contains(&t.id),
                        unreviewed: self.unreviewed.get(&t.id).copied().unwrap_or(0),
                        stage: self.stages.get(&t.id).map(String::as_str),
                        wait: self.waits.get(&t.id).map(String::as_str),
                        automation_block: self.automation_blocks.get(&t.id).map(String::as_str),
                        route: self.routes.get(&t.id).map(|r| &r.card),
                    };
                    let mut lines =
                        card_lines(t, &self.tasks, config, cw, sel_row == Some(i), now, extra);
                    if !t.tracker && extra.wait.is_none() {
                        crate::resources::append_hold(
                            &mut lines,
                            t,
                            config,
                            self.resources.reason.as_deref(),
                            cw,
                        );
                    }
                    lines
                })
                .clone()
        };
        let ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();

        let slot = Status::COLUMNS.iter().position(|s| *s == col).unwrap_or(0);
        let mut offset = self.offsets[slot].min(tasks.len() - 1);
        if let (Some(sel), true) = (sel_row, self.follow) {
            offset = offset.min(sel);
            while offset < sel
                && (offset..=sel).map(|i| card(i).len() + 1).sum::<usize>()
                    > inner.height as usize + 1
            {
                offset += 1;
            }
        }
        let mut rects = Vec::new();
        let mut y = inner.y;
        for (i, id) in ids.iter().enumerate().skip(offset) {
            if y >= inner.bottom() {
                break;
            }
            let lines = card(i);
            let h = (lines.len() as u16).min(inner.bottom() - y);
            let mut p = Paragraph::new(lines);
            if sel_row == Some(i) {
                p = p.style(Style::new().bg(theme::SELECTED_BG));
            }
            let rect = Rect {
                y,
                height: h,
                ..inner
            };
            f.render_widget(p, rect);
            hits.push(rect, Hit::Card(id.clone()));
            if !config.ui.compact_cards
                && config.board.card.density != corral_core::Density::Compact
            {
                hits.push(
                    Rect::new(inner.right() - 2, y, 2, 1),
                    Hit::CardMenu(id.clone()),
                );
            }
            rects.push((id.clone(), rect));
            y += h + 1;
        }
        self.offsets[slot] = offset;
        self.card_rects.extend(rects);
    }
}

/// The Done column's `+N older ›` foot; a click presses `Y` (history).
fn draw_older(f: &mut Frame, foot: Rect, older: usize, hits: &mut HitMap) {
    let text = format!(" +{older} older ›");
    let w = (text.width() as u16).min(foot.width);
    f.render_widget(
        Paragraph::new(Line::styled(text, Style::new().fg(theme::CYAN))),
        foot,
    );
    hits.push(
        Rect { width: w, ..foot },
        Hit::Key(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::NONE)),
    );
}
