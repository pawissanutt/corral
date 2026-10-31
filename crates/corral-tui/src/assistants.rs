//! Workspace assistant sessions and the PM's durable inbox. Read-only: no pane handoffs.
use anyhow::{Context, Result};
use corral_core::pm_inbox::{self, ItemState, PmInboxItem};
use corral_ops::{AssistantStats, Ctx};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind},
    layout::Rect,
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::{Action, HitMap, View};

pub(crate) struct AssistantsView {
    sessions: Vec<AssistantStats>,
    inbox: Vec<PmInboxItem>,
    offset: u16,
    error: Option<String>,
}

impl AssistantsView {
    pub(crate) fn new(ctx: &Ctx) -> Result<Self> {
        let mut view = Self {
            sessions: Vec::new(),
            inbox: Vec::new(),
            offset: 0,
            error: None,
        };
        view.load(ctx)?;
        Ok(view)
    }

    fn load(&mut self, ctx: &Ctx) -> Result<()> {
        let mut sessions = corral_ops::assistant_stats(&ctx.store, None)?;
        sessions.sort_by_key(|s| (!s.live, std::cmp::Reverse(s.created)));
        // All live sessions, plus the 20 most recent resumable/ended sessions.
        let mut recent = 0;
        sessions.retain(|s| {
            if s.live {
                true
            } else {
                recent += 1;
                recent <= 20
            }
        });
        let inbox = ctx
            .store
            .pm_inbox_items()?
            .into_iter()
            .filter(|i| matches!(i.state, ItemState::Open | ItemState::Claimed))
            .collect();
        self.sessions = sessions;
        self.inbox = inbox;
        self.error = None;
        Ok(())
    }

    fn lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from("Live and recent assistants · no coding slots")];
        for s in &self.sessions {
            let source = format!("assistant:{}", s.session);
            let open = self.inbox.iter().filter(|i| i.from == source).count();
            let status = if s.ended.is_some() {
                "ended"
            } else if s.live {
                "live"
            } else {
                "open"
            };
            lines.push(Line::from(format!(
                "asst {} · {} · focus {} · pane {}",
                s.session,
                status,
                s.focus,
                s.pane.as_deref().unwrap_or("-")
            )));
            lines.push(Line::from(format!(
                "  cost {} · scratch {} · open inbox {}",
                s.cost, s.scratch, open
            )));
        }
        if self.sessions.is_empty() {
            lines.push(Line::from(
                "No assistant sessions yet. Use s on a card to open one.",
            ));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "PM inbox · {} open (including claimed)",
            self.inbox.len()
        )));
        for item in &self.inbox {
            let source = item
                .from
                .strip_prefix("assistant:")
                .map(|s| format!("asst {s}"))
                .unwrap_or_else(|| item.from.clone());
            lines.push(Line::from(format!(
                "{} · {} · {} · {} · {}",
                item.id,
                source,
                pm_inbox::name(&item.kind),
                pm_inbox::name(&item.state),
                item.subject
            )));
            lines.push(Line::from(format!("  corral pm inbox show {}", item.id)));
        }
        if self.inbox.is_empty() {
            lines.push(Line::from("No open PM inbox items."));
        }
        if let Some(error) = &self.error {
            lines.push(Line::from(format!("Refresh failed: {error}")));
        }
        lines
    }
}

impl View for AssistantsView {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        let lines = self.lines();
        let height = area.height.saturating_sub(2) as usize;
        self.offset = self.offset.min(lines.len().saturating_sub(height) as u16);
        hits.push(
            Rect::new(area.x, area.y, area.width.min(8), 1),
            crate::Hit::Key(KeyCode::Esc.into()),
        );
        f.render_widget(
            Paragraph::new(lines).scroll((self.offset, 0)).block(
                Block::default().borders(Borders::ALL).title(format!(
                    "‹ back · assistants · PM inbox [{}]",
                    self.inbox.len()
                )),
            ),
            area,
        );
    }

    fn on_key(&mut self, key: KeyEvent, _ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Action::Back,
            KeyCode::Down | KeyCode::Char('j') => {
                self.offset = self.offset.saturating_add(1);
                Action::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.offset = self.offset.saturating_sub(1);
                Action::None
            }
            KeyCode::Char('r') => Action::Reload,
            _ => Action::None,
        }
    }

    fn on_mouse(&mut self, ev: MouseEvent, _m: &crate::MouseCtx) -> Action {
        match ev.kind {
            MouseEventKind::ScrollDown => self.offset = self.offset.saturating_add(3),
            MouseEventKind::ScrollUp => self.offset = self.offset.saturating_sub(3),
            _ => {}
        }
        Action::None
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![("j/k", "scroll"), ("r", "refresh"), ("esc", "back")]
    }

    fn refresh(&mut self, ctx: &Ctx) {
        if let Err(error) = self.load(ctx) {
            self.error = Some(format!("{error:#}"));
        }
    }
}

/// A new pane owns this launch; never send input to an existing assistant or PM pane.
pub(crate) fn open_task(ctx: &Ctx, id: &str) -> Result<String> {
    let executable = std::env::current_exe().context("locating corral for assistant launch")?;
    open_task_with(ctx, id, &executable)
}

fn open_task_with(ctx: &Ctx, id: &str, executable: &std::path::Path) -> Result<String> {
    ctx.store.get(id)?;
    let command = launch_command(executable, id);
    let pane = ctx
        .herdr
        .workspace_create(ctx.store.root(), &format!("asst · {id}"), false)?;
    if let Err(error) = ctx.herdr.pane_run(&pane.pane_id, &command) {
        // Only the workspace just allocated here is ours to close on a failed launch.
        let _ = ctx.herdr.workspace_close(&pane.workspace_id);
        return Err(error);
    }
    ctx.herdr.pane_focus(&pane.pane_id)?;
    Ok(pane.pane_id)
}

fn launch_command(executable: &std::path::Path, id: &str) -> String {
    fn quote(text: &str) -> String {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
    format!(
        "{} pi --task {}",
        quote(&executable.to_string_lossy()),
        quote(id)
    )
}

#[cfg(all(test, unix))]
mod live_tests;
#[cfg(test)]
mod tests;
