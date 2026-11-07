//! Sheet layout: target and blockers first, original versus proposed checks next, then effect, remaining hard
//! blockers and owner fields. The button row stays fixed below a scrolling body, so it is reachable at 80x24.

use corral_core::{BlockerClass, RecoveryPreview};
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::labels::{self, wire};
use super::{Control, KINDS, Kind, RecoverySheet, Step, Z_KIND, Z_PARAM, Z_REVIEWER, Z_ROW};
use crate::hit::{self, Hit};
use crate::overlays;
use crate::theme::{self, CYAN, FG, RED, YELLOW};
use crate::{Action, HitMap, MouseCtx, Tone, View};

const MAX_W: u16 = 100;
const LABEL_W: usize = 10;

/// Wraps `text` to `width` display columns, continuing lines with `indent`.
pub(crate) fn wrap(text: &str, width: usize, indent: &str) -> Vec<String> {
    let width = width.max(indent.width() + 8);
    let mut out = Vec::new();
    let mut line = String::new();
    for c in text.chars() {
        let cw = c.width().unwrap_or(0);
        if line.width() + cw > width {
            out.push(std::mem::take(&mut line));
            line.push_str(indent);
        }
        line.push(c);
    }
    out.push(line);
    out
}

struct Body {
    lines: Vec<Line<'static>>,
    controls: Vec<Option<Control>>,
    /// Pill rows: (line, zone, options, x offset).
    pills: Vec<(usize, u8, Vec<String>, u16)>,
}

impl Body {
    fn push(&mut self, line: Line<'static>, control: Option<Control>) {
        self.lines.push(line);
        self.controls.push(control);
    }

    fn text(&mut self, text: &str, width: usize, style: Style) {
        for l in wrap(text, width, "    ") {
            self.push(Line::styled(l, style), None);
        }
    }
}

fn marker(focused: bool) -> Span<'static> {
    if focused {
        Span::styled("▸ ", Style::new().fg(CYAN).add_modifier(Modifier::BOLD))
    } else {
        Span::raw("  ")
    }
}

impl RecoverySheet {
    fn pill_row(
        &self,
        body: &mut Body,
        control: Control,
        zone: u8,
        name: &str,
        options: Vec<String>,
        sel: usize,
    ) {
        let focused = self.focused() == control;
        let prefix = format!("{name:<LABEL_W$}");
        let refs: Vec<&str> = options.iter().map(String::as_str).collect();
        let mut spans = vec![marker(focused), Span::styled(prefix.clone(), theme::dim())];
        spans.extend(overlays::pills(&refs, sel, if focused { "←→" } else { "" }).spans);
        body.pills
            .push((body.lines.len(), zone, options, (2 + prefix.width()) as u16));
        body.push(Line::from(spans), Some(control));
    }

    fn field(
        &self,
        body: &mut Body,
        control: Control,
        name: &str,
        text: &str,
        placeholder: &str,
        width: usize,
    ) {
        let focused = self.focused() == control;
        let room = width.saturating_sub(LABEL_W + 4);
        let shown: String = if text.width() > room {
            let mut tail: Vec<char> = Vec::new();
            let mut w = 0;
            for c in text.chars().rev() {
                w += c.width().unwrap_or(0);
                if w > room.saturating_sub(1) {
                    break;
                }
                tail.push(c);
            }
            format!("…{}", tail.into_iter().rev().collect::<String>())
        } else {
            text.to_string()
        };
        let value = if shown.is_empty() {
            Span::styled(placeholder.to_string(), theme::dim())
        } else {
            Span::styled(shown, Style::new().fg(FG))
        };
        let mut spans = vec![
            marker(focused),
            Span::styled(format!("{name:<LABEL_W$}"), Style::new().fg(CYAN)),
            value,
        ];
        if focused {
            spans.push(Span::styled("▏", Style::new().fg(CYAN)));
        }
        body.push(Line::from(spans), Some(control));
    }

    fn target(&self, body: &mut Body, p: &RecoveryPreview, width: usize) {
        let t = &p.request.target;
        body.text(
            &format!(
                "Target {} · {} · task rev {}",
                t.task,
                labels::identity(&t.identity),
                t.task_revision.chars().take(7).collect::<String>()
            ),
            width,
            theme::bold(),
        );
    }

    fn edit_lines(&self, width: usize) -> Body {
        let mut b = Body {
            lines: vec![],
            controls: vec![],
            pills: vec![],
        };
        let preview = match &self.preview {
            Some(Ok(p)) => Some(p),
            Some(Err(e)) => {
                b.text(
                    &format!("Target {} · preview unavailable", self.task),
                    width,
                    theme::bold(),
                );
                b.text(&format!("  {e}"), width, Style::new().fg(RED));
                None
            }
            None => None,
        };
        if let Some(p) = preview {
            self.target(&mut b, p, width);
            b.push(Line::styled("Current blockers", theme::bold()), None);
            if p.blockers.is_empty() {
                b.push(
                    Line::styled("  none observed for this action", theme::dim()),
                    None,
                );
            }
            for blocker in &p.blockers {
                let class = match blocker.class {
                    BlockerClass::Recoverable => "recoverable",
                    BlockerClass::Hard => "hard",
                    BlockerClass::Prerequisite => "prerequisite",
                };
                let style = if blocker.class == BlockerClass::Recoverable {
                    Style::new().fg(YELLOW)
                } else {
                    Style::new().fg(RED)
                };
                b.text(
                    &format!(
                        "  [{class}] {}: {}",
                        blocker.code,
                        labels::observation(&blocker.observed)
                    ),
                    width,
                    style,
                );
            }
        }
        b.push(Line::default(), None);
        let kinds: Vec<String> = KINDS.iter().map(|k| k.label().to_string()).collect();
        self.pill_row(&mut b, Control::Kind, Z_KIND, "Action", kinds, self.kind);
        let kind = self.kind();
        if !kind.params().is_empty() {
            let params = kind.params().iter().map(|s| s.to_string()).collect();
            self.pill_row(
                &mut b,
                Control::Param,
                Z_PARAM,
                kind.param_name(),
                params,
                self.param,
            );
        }
        if kind == Kind::UseReviewer {
            self.pill_row(
                &mut b,
                Control::Reviewer,
                Z_REVIEWER,
                "reviewer",
                self.reviewers.clone(),
                self.reviewer,
            );
        }
        if let Some(p) = preview {
            b.push(
                Line::styled("Checks  original → proposed", theme::bold()),
                None,
            );
            for c in &p.checks {
                if let Some(i) = self.offered.iter().position(|o| *o == c.check) {
                    let on = self.acknowledged.contains(&c.check);
                    let focused = self.focused() == Control::Check(i);
                    b.push(
                        Line::from(vec![
                            marker(focused),
                            Span::styled(if on { "[x] " } else { "[ ] " }, Style::new().fg(CYAN)),
                            Span::styled(labels::check_line(c), Style::new().fg(YELLOW)),
                        ]),
                        Some(Control::Check(i)),
                    );
                } else {
                    b.text(
                        &format!("    {}", labels::check_line(c)),
                        width,
                        theme::dim(),
                    );
                }
            }
            if let Some(r) = &p.policy.expected_reviewer {
                b.text(
                    &format!(
                        "  expected reviewer {} ({} · {})",
                        r.profile, r.model, r.vendor
                    ),
                    width,
                    theme::dim(),
                );
            }
            if let Some(failure) = &p.policy.resolution_failure {
                b.text(
                    &format!(
                        "  policy resolution failed ({}): {}",
                        failure.code, failure.message
                    ),
                    width,
                    theme::dim(),
                );
            }
            for (key, source) in &p.policy.sources {
                b.text(
                    &format!("  policy {key} from {source}"),
                    width,
                    theme::dim(),
                );
            }
            b.push(Line::styled("Proposed effect", theme::bold()), None);
            b.text(
                &format!("  {}", labels::effect(&p.proposed_effect)),
                width,
                Style::new().fg(FG),
            );
            b.push(Line::styled("Remaining hard blockers", theme::bold()), None);
            if p.remaining_blockers.is_empty() {
                b.push(Line::styled("  none", theme::dim()), None);
            }
            for blocker in &p.remaining_blockers {
                b.text(
                    &format!(
                        "  {}: {}",
                        blocker.code,
                        labels::observation(&blocker.observed)
                    ),
                    width,
                    Style::new().fg(RED),
                );
            }
        }
        b.push(Line::default(), None);
        if kind == Kind::AcceptPlan {
            self.field(
                &mut b,
                Control::Notes,
                "decisions",
                &self.notes,
                "resolve every owner decision",
                width,
            );
        }
        self.field(
            &mut b,
            Control::Reason,
            "reason",
            &self.reason,
            "owner-written reason (required)",
            width,
        );
        b
    }

    fn confirm_lines(&self, width: usize) -> Body {
        let mut b = Body {
            lines: vec![],
            controls: vec![],
            pills: vec![],
        };
        b.push(
            Line::styled(
                "Confirm owner recovery — not ordinary policy success",
                Style::new().fg(YELLOW).add_modifier(Modifier::BOLD),
            ),
            None,
        );
        if let Some(Ok(p)) = &self.preview {
            self.target(&mut b, p, width);
            b.text(
                &format!("Action {}", self.kind().label()),
                width,
                Style::new().fg(FG),
            );
            b.push(
                Line::styled(
                    "Exceptions authorized for this operation only",
                    theme::bold(),
                ),
                None,
            );
            for c in p
                .checks
                .iter()
                .filter(|c| p.request.exceptions.contains(&c.check))
            {
                b.text(
                    &format!("  {} ({})", labels::check_line(c), wire!(&c.check)),
                    width,
                    Style::new().fg(YELLOW),
                );
            }
            b.push(Line::styled("Effect", theme::bold()), None);
            b.text(
                &format!("  {}", labels::effect(&p.proposed_effect)),
                width,
                Style::new().fg(FG),
            );
            b.text(
                &format!("Reason: {}", self.reason.trim()),
                width,
                Style::new().fg(FG),
            );
            if self.kind() == Kind::AcceptPlan {
                b.text(
                    &format!("Decisions: {}", self.notes.trim()),
                    width,
                    Style::new().fg(FG),
                );
            }
            b.text(
                &format!("Preview {}", p.digest.chars().take(16).collect::<String>()),
                width,
                theme::dim(),
            );
        }
        b.text("Cancel is focused. Confirm recovery applies this exact operation once; a changed proposal is refreshed instead.", width, theme::dim());
        b
    }

    fn buttons(&self) -> Vec<(Control, &'static str)> {
        match self.step {
            Step::Edit => vec![
                (Control::Cancel, "Cancel"),
                (Control::Continue, "Continue…"),
            ],
            Step::Confirm => vec![
                (Control::Edit, "Edit"),
                (Control::Cancel, "Cancel"),
                (Control::Confirm, "Confirm recovery"),
            ],
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        let w = MAX_W.min(area.width);
        let rect = overlays::centered(area, w, area.height);
        let inner = overlays::popup(f, rect, &format!("Recover {}", self.task), hits);
        if inner.height < 3 || inner.width < 10 {
            return;
        }
        let body_area = Rect::new(inner.x, inner.y, inner.width, inner.height - 2);
        let width = usize::from(inner.width);
        let body = match self.step {
            Step::Edit => self.edit_lines(width),
            Step::Confirm => self.confirm_lines(width),
        };
        let height = usize::from(body_area.height);
        self.page = height.max(1);
        let focused = self.focused();
        if self.follow
            && let Some(line) = body.controls.iter().position(|c| *c == Some(focused))
        {
            if line < self.scroll.offset {
                self.scroll.offset = line;
            } else if line >= self.scroll.offset + height {
                self.scroll.offset = line + 1 - height;
            }
        }
        let offset = self.scroll.clamp(body.lines.len(), height);
        f.render_widget(
            Paragraph::new(body.lines.clone())
                .scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0)),
            body_area,
        );
        let controls = self.controls();
        for (line, control) in body.controls.iter().enumerate() {
            let (Some(control), Some(dy)) = (control, line.checked_sub(offset)) else {
                continue;
            };
            if dy >= height {
                continue;
            }
            if let Some(i) = controls.iter().position(|c| c == control) {
                hits.push(
                    Rect::new(body_area.x, body_area.y + dy as u16, body_area.width, 1),
                    Hit::Item(Z_ROW, i),
                );
            }
        }
        for (line, zone, options, x) in &body.pills {
            if let Some(dy) = line.checked_sub(offset).filter(|dy| *dy < height) {
                let refs: Vec<&str> = options.iter().map(String::as_str).collect();
                hit::pill_hits(
                    hits,
                    &refs,
                    *zone,
                    body_area.x + x,
                    body_area.y + dy as u16,
                    body_area.right(),
                );
            }
        }
        let notice_row = Rect::new(inner.x, inner.bottom() - 2, inner.width, 1);
        let more = if offset + height < body.lines.len() {
            "  ↓ more (pgdn)"
        } else {
            ""
        };
        let notice = match &self.notice {
            Some((tone, text)) => Line::styled(
                format!("{text}{more}"),
                Style::new().fg(if *tone == Tone::Warn { YELLOW } else { FG }),
            ),
            None => Line::styled(more.trim_start().to_string(), theme::dim()),
        };
        f.render_widget(Paragraph::new(notice), notice_row);
        let mut x = inner.x + 1;
        let y = inner.bottom() - 1;
        for (control, label) in self.buttons() {
            let text = format!("[{label}]");
            let tw = (text.width() as u16).min(inner.right().saturating_sub(x));
            if tw == 0 {
                break;
            }
            let style = if focused == control {
                Style::new()
                    .fg(theme::BG)
                    .bg(CYAN)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(CYAN).bg(theme::SELECTED_BG)
            };
            let r = Rect::new(x, y, tw, 1);
            f.render_widget(Span::styled(text, style), r);
            if let Some(i) = controls.iter().position(|c| *c == control) {
                hits.push(r, Hit::Item(Z_ROW, i));
            }
            x = x.saturating_add(tw + 1);
        }
    }
}

impl View for RecoverySheet {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        self.render(f, area, hits);
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        RecoverySheet::on_key(self, key, ctx)
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        RecoverySheet::on_mouse(self, ev, m)
    }

    fn has_unsaved_text(&self) -> bool {
        RecoverySheet::has_unsaved_text(self)
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        RecoverySheet::hints(self)
    }

    fn refresh(&mut self, ctx: &Ctx) {
        RecoverySheet::refresh(self, ctx);
    }

    fn is_overlay(&self) -> bool {
        true
    }

    fn captures_text(&self) -> bool {
        RecoverySheet::captures_text(self)
    }
}
