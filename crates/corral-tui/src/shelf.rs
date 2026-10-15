//! Pinned shelf: the one-line strip of pin chips under the board header, the shelf panel (`G`), the `*` pin
//! toggle every list shares, and opening a pin through the viewer layer (HTML in a terminal-browser split,
//! images and text in a preview popup, tasks in their detail or review screen).

use std::collections::HashSet;

use anyhow::{Result, bail};
use corral_core::{ArtifactKind, Pin, PinRef, PinTarget, Task, resolve_pin};
use corral_ops::{Ctx, Preview};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::artifacts::ui::{self, PreviewPopup};
use crate::hit::{self, Hit};
use crate::overlays::truncate;
use crate::theme;
use crate::{Action, HitMap, MouseCtx, Toast, Tone, View};

pub(crate) mod panel;
#[cfg(test)]
mod tests;

/// Mouse zones ([`Hit::Item`]) of the strip: the whole strip (wheel), a chip, and the overflow arrows.
pub(crate) const Z_STRIP: u8 = 20;
pub(crate) const Z_CHIP: u8 = 21;
pub(crate) const Z_PREV: u8 = 22;
pub(crate) const Z_NEXT: u8 = 23;

/// Widest label a chip shows before truncating.
const CHIP_LABEL: usize = 28;
/// The strip's leading `★ ` mark.
const LEAD: &str = " ★ ";

/// A pin and what it resolves to right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub pin: Pin,
    pub target: PinTarget,
}

/// The store's pins resolved against `tasks` (an unreadable pins.toml shows as no pins).
pub(crate) fn load(ctx: &Ctx, tasks: &[Task]) -> Vec<Entry> {
    ctx.store
        .pins()
        .unwrap_or_default()
        .into_iter()
        .map(|pin| {
            let target = resolve_pin(&pin, tasks, ctx.store.dir());
            Entry { pin, target }
        })
        .collect()
}

/// Refs of every pin ("T-042", "T-042:R1:<file>") for ★ markers.
pub(crate) fn pinned_refs(ctx: &Ctx) -> HashSet<String> {
    ctx.store
        .pins()
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.target)
        .collect()
}

/// The pin ref of an artifact of `task`.
pub(crate) fn artifact_ref(task: &str, a: &corral_core::Artifact) -> String {
    PinRef::of_artifact(task, a).to_string()
}

/// The ★ marker span for a pinned row (two columns wide either way, so rows stay aligned).
pub(crate) fn star(pinned: bool) -> Span<'static> {
    if pinned {
        Span::styled("★ ", Style::new().fg(theme::YELLOW))
    } else {
        Span::raw("  ")
    }
}

/// Pins `target` (a ref) or, when it is already pinned, unpins it; the toast says which.
pub(crate) fn toggle(ctx: &Ctx, target: &str) -> Action {
    let pinned = ctx
        .store
        .pins()
        .unwrap_or_default()
        .iter()
        .any(|p| p.target == target);
    let result = if pinned {
        ctx.store.unpin(target).map(|p| {
            Toast::new(
                Tone::Info,
                "Unpinned",
                format!("{} left the shelf", p.target),
            )
        })
    } else {
        ctx.store.pin(target, None, false).map(|(n, p)| {
            let open = if n <= 9 {
                format!("; {n} opens it from the board")
            } else {
                String::new()
            };
            Toast::new(
                Tone::Good,
                format!("★ Pinned #{n}"),
                format!("{}{open}", p.target),
            )
        })
    };
    Action::Toast(result.unwrap_or_else(|e| {
        Toast::new(
            Tone::Bad,
            format!(
                "Could not {} {target}",
                if pinned { "unpin" } else { "pin" }
            ),
            format!("{e:#}"),
        )
    }))
}

/// Kind icon of a pin target.
pub(crate) fn icon(target: &PinTarget) -> &'static str {
    match target {
        PinTarget::Task { .. } => "▣",
        PinTarget::Broken(_) => "⚠",
        PinTarget::Artifact { artifact, path, .. } => {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            match (artifact.kind, ext.as_str()) {
                (_, "html" | "htm") => "◧",
                (ArtifactKind::Screenshot, _)
                | (_, "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg") => "▨",
                (ArtifactKind::Log, _) => "≡",
                (ArtifactKind::Diff, _) => "±",
                (ArtifactKind::Plan, _) => "☰",
                _ => "◆",
            }
        }
    }
}

/// What a chip or panel row calls the pin: its label, else the target's title, else the ref.
pub(crate) fn label(e: &Entry) -> String {
    if let Some(l) = &e.pin.label {
        return l.clone();
    }
    match &e.target {
        PinTarget::Task { title, .. } => title.clone(),
        PinTarget::Artifact { artifact, .. } => artifact.title.clone(),
        PinTarget::Broken(_) => e.pin.target.clone(),
    }
}

/// Chip text for the pin at 0-based `i`: ` 1 ◆ label ` (pins past the ninth carry no number key).
pub(crate) fn chip_text(i: usize, e: &Entry) -> String {
    let n = if i < 9 {
        format!("{} ", i + 1)
    } else {
        String::new()
    };
    format!(
        " {n}{} {} ",
        icon(&e.target),
        truncate(&label(e), CHIP_LABEL)
    )
}

/// Index one past the last chip that fits in `avail` columns when the strip starts at chip `offset`, leaving
/// room for the `›` overflow mark whenever chips remain.
fn visible_end(widths: &[u16], offset: usize, avail: u16) -> usize {
    let mut x = 0u16;
    for (i, w) in widths.iter().enumerate().skip(offset) {
        let reserve = if i + 1 < widths.len() { 6 } else { 0 };
        if x + w + reserve > avail {
            return i.max(offset + 1).min(widths.len());
        }
        x += w + 1;
    }
    widths.len()
}

/// Draws the strip in `area`: `★`, then chips from `offset` (adjusted so `cursor` stays visible), with `‹` and
/// `› +N` marks when chips overflow. Registers chip, arrow and strip hits.
pub(crate) fn draw_strip(
    f: &mut Frame,
    area: Rect,
    entries: &[Entry],
    offset: &mut usize,
    cursor: Option<usize>,
    hits: &mut HitMap,
) {
    hits.push(area, Hit::Item(Z_STRIP, 0));
    f.render_widget(
        Paragraph::new(Line::styled(
            LEAD,
            Style::new().fg(theme::YELLOW).add_modifier(Modifier::BOLD),
        )),
        area,
    );
    let texts: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| chip_text(i, e))
        .collect();
    let widths: Vec<u16> = texts.iter().map(|t| t.width() as u16).collect();
    let lead = LEAD.width() as u16;
    let prev_w = 2u16;
    let avail = |off: usize| {
        area.width
            .saturating_sub(lead + if off > 0 { prev_w } else { 0 })
    };
    *offset = (*offset).min(entries.len().saturating_sub(1));
    if let Some(c) = cursor {
        *offset = (*offset).min(c);
        while *offset < c && visible_end(&widths, *offset, avail(*offset)) <= c {
            *offset += 1;
        }
    }
    let mut x = area.x + lead;
    if *offset > 0 {
        let r = Rect::new(x, area.y, prev_w.min(area.right().saturating_sub(x)), 1);
        f.render_widget(Span::styled("‹ ", Style::new().fg(theme::CYAN)), r);
        hits.push(r, Hit::Item(Z_PREV, 0));
        x += prev_w;
    }
    let end = visible_end(&widths, *offset, avail(*offset));
    for i in *offset..end {
        let w = widths[i].min(area.right().saturating_sub(x));
        if w == 0 {
            break;
        }
        let r = Rect::new(x, area.y, w, 1);
        let mut style = match entries[i].target {
            PinTarget::Broken(_) => Style::new().fg(theme::RED).bg(theme::BG2),
            _ => Style::new().fg(theme::FG).bg(theme::SELECTED_BG),
        };
        if cursor == Some(i) {
            style = Style::new()
                .fg(theme::BG)
                .bg(theme::CYAN)
                .add_modifier(Modifier::BOLD);
        }
        f.render_widget(Span::styled(texts[i].clone(), style), r);
        hits.push(r, Hit::Item(Z_CHIP, i));
        x = x.saturating_add(w + 1);
    }
    if end < entries.len() {
        let text = format!("› +{}", entries.len() - end);
        let w = (text.width() as u16).min(area.right().saturating_sub(x));
        let r = Rect::new(x, area.y, w, 1);
        f.render_widget(Span::styled(text, Style::new().fg(theme::CYAN)), r);
        hits.push(r, Hit::Item(Z_NEXT, 0));
    }
}

/// Opens the pin at 0-based `index` with the viewer layer: a task opens its review (when it awaits the owner)
/// or detail screen, HTML and other external files go to `corral_ops::open_path` (terminal-browser split for
/// HTML inside herdr), images and text open in a preview popup.
pub(crate) fn open(ctx: &Ctx, index: usize) -> Result<Opening> {
    let pins = ctx.store.pins()?;
    let Some(pin) = pins.get(index) else {
        bail!("no pin #{} ({} pinned)", index + 1, pins.len());
    };
    let tasks = ctx.store.list()?;
    match resolve_pin(pin, &tasks, ctx.store.dir()) {
        PinTarget::Task { id, .. } => {
            let awaiting = tasks.iter().any(|t| t.id == id && t.awaiting_owner());
            Ok(if awaiting {
                Opening::Action(Action::OpenReview(id))
            } else {
                Opening::Action(Action::OpenDetail(id))
            })
        }
        PinTarget::Artifact { artifact, .. } => {
            match corral_ops::artifact_preview(ctx, &artifact)? {
                Preview::External { .. } => Ok(Opening::Action(Action::Toast(ui::open_toast(
                    ctx, &artifact,
                )))),
                Preview::Text(_) | Preview::Image { .. } => Ok(Opening::Preview(Box::new(
                    PinPreview(PreviewPopup::open(ctx, &artifact)),
                ))),
            }
        }
        PinTarget::Broken(reason) => bail!("pin #{} {} is broken: {reason}", index + 1, pin.target),
    }
}

/// What opening a pin produces.
pub(crate) enum Opening {
    /// An ordinary route (detail, review) or the toast of an external viewer launch.
    Action(Action),
    /// A preview popup to push on top of the current screen.
    Preview(Box<PinPreview>),
}

/// A pinned artifact's preview popup as a stacked overlay view: ⏎ opens it with its viewer, esc closes.
pub(crate) struct PinPreview(pub(crate) PreviewPopup);

impl View for PinPreview {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        self.0.draw(f, area, hits);
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        if !ui::is_press(&key) {
            return Action::None;
        }
        if key.code == KeyCode::Esc {
            return Action::Back;
        }
        self.0.on_key(&key, ctx).map_or(Action::None, Action::Toast)
    }

    fn on_mouse(&mut self, ev: MouseEvent, _m: &MouseCtx) -> Action {
        if let Some(dir) = hit::wheel(&ev) {
            self.0.wheel(dir);
        }
        Action::None
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("j/k", "scroll"),
            ("⏎", "open externally"),
            ("esc", "close"),
        ]
    }

    fn is_overlay(&self) -> bool {
        true
    }
}
