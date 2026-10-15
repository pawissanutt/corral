//! Persistent owner/PM thread and the owner comment composer.
use corral_core::{CommentAuthor, Status, Task};
use corral_ops::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::DetailView;
use crate::{
    Action, HitMap, Toast, Tone, hit,
    overlays::{self, LineInput},
    theme,
};

pub(super) fn lines(task: &Task) -> Vec<Line<'static>> {
    if task.comments.is_empty() {
        return vec![Line::from("No comments yet. Press c to add one.")];
    }
    let mut lines = Vec::new();
    for comment in &task.comments {
        lines.push(Line::from(vec![
            Span::styled(comment.author.as_str().to_string(), theme::bold()),
            Span::raw(format!("  {} UTC", comment.at.strftime("%Y-%m-%d %H:%M"))),
        ]));
        lines.extend(crate::artifacts::ui::text_lines(&comment.text));
        lines.push(Line::default());
    }
    lines
}

pub(super) fn toolbar(f: &mut Frame, area: Rect, hits: &mut HitMap) -> Rect {
    let [buttons, _, thread] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    hit::buttons(
        f,
        hits,
        buttons.x,
        buttons.y,
        buttons.right(),
        &[("Add comment", hit::press(KeyCode::Char('c')))],
    );
    thread
}

#[derive(Default)]
pub(super) struct Composer {
    pub(super) input: LineInput,
    send: bool,
    live: bool,
}

impl Composer {
    pub(super) fn new(task: &Task) -> Self {
        Self {
            live: matches!(task.status, Status::Running | Status::NeedsMe)
                && task
                    .current_run()
                    .is_some_and(|r| r.outcome.is_none() && r.herdr.is_some()),
            ..Self::default()
        }
    }

    pub(super) fn draw(&self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        overlays::dim_backdrop(f.buffer_mut(), area);
        let popup = overlays::centered(area, 76, 9);
        let inner = overlays::popup(f, popup, "Add comment", hits);
        let [author, input, _, send, _, buttons] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(inner);
        f.render_widget(Paragraph::new("Author: owner"), author);
        self.input.draw(f, input, "Comment:", "Type a comment");
        if self.live {
            let label = if self.send {
                "[x] Send to live agent"
            } else {
                "[ ] Send to live agent"
            };
            hit::buttons(
                f,
                hits,
                send.x,
                send.y,
                send.right(),
                &[(label, hit::press(KeyCode::Tab))],
            );
        } else {
            f.render_widget(Paragraph::new("Saved to the task; no live agent."), send);
        }
        hit::buttons(
            f,
            hits,
            buttons.x,
            buttons.y,
            buttons.right(),
            &[
                ("Save comment", hit::press(KeyCode::Enter)),
                ("Cancel", hit::press(KeyCode::Esc)),
            ],
        );
    }
}

impl DetailView {
    pub(super) fn on_comment_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        let Some(composer) = &mut self.comment else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => self.comment = None,
            KeyCode::Tab | KeyCode::BackTab => {
                if composer.live {
                    composer.send = !composer.send;
                }
            }
            KeyCode::Enter => {
                if composer.input.text.trim().is_empty() {
                    return Action::Toast(Toast::new(
                        Tone::Warn,
                        "Empty comment",
                        "Type a comment first",
                    ));
                }
                match corral_ops::comment(
                    ctx,
                    &self.task.id,
                    &composer.input.text,
                    CommentAuthor::Owner,
                    composer.send,
                ) {
                    Ok(task) => {
                        self.task = task;
                        self.comment = None;
                        self.scroll.offset = usize::MAX;
                        return Action::Toast(Toast::new(
                            Tone::Good,
                            "Comment saved",
                            &self.task.id,
                        ));
                    }
                    Err(error) => {
                        // Delivery can fail after the append. Read back that saved note so
                        // the owner sees it and cannot accidentally submit it a second time.
                        let mut saved = false;
                        if let Ok(task) = ctx.store.get(&self.task.id) {
                            saved = task.comments.len() > self.task.comments.len()
                                && task.comments.last().is_some_and(|c| {
                                    c.author == CommentAuthor::Owner
                                        && c.text == composer.input.text
                                });
                            self.task = task;
                            if saved {
                                self.comment = None;
                                self.scroll.offset = usize::MAX;
                            }
                        }
                        return Action::Toast(Toast::new(
                            Tone::Bad,
                            if saved {
                                "Comment saved; delivery failed"
                            } else {
                                "Comment failed"
                            },
                            if saved {
                                format!(
                                    "{error:#}. Retry delivery with corral reply {} --resend",
                                    self.task.id
                                )
                            } else {
                                format!("{error:#}")
                            },
                        ));
                    }
                }
            }
            _ => {
                composer.input.edit(&key);
            }
        }
        Action::None
    }
}
