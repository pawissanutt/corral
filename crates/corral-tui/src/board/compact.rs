//! The one-line compact card: id, a Ready card's admission state, title and the configured fields, reserving the
//! highest-priority field first.

use corral_core::{CardCfg, CardField, Task, TaskCost};
use jiff::Timestamp;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::cards::wait_color;
use crate::card_fields;
use crate::overlays::truncate;
use crate::theme;

pub(super) struct Compact<'a> {
    pub plan_progress: Option<&'a crate::plan::tracker::Progress>,
    pub route: Option<&'a crate::routes::CardRoute>,
    pub route_warning: bool,
    pub t: &'a Task,
    pub all: &'a [Task],
    pub cfg: &'a CardCfg,
    pub now: Timestamp,
    pub pinned: bool,
    pub stage: Option<&'a str>,
    /// Ready cards: the dispatch-queue admission reason.
    pub wait: Option<&'a str>,
}

/// The admission state's glyph, distinct per class so it tells the states apart alone, and its word.
fn wait_parts(reason: &str) -> (&'static str, &str) {
    if reason == "eligible" {
        return ("▸", "eligible");
    }
    if let Some(rest) = reason.strip_prefix("waiting on ") {
        return ("⛓", rest.split(' ').next().unwrap_or(rest));
    }
    if reason.starts_with("waiting: machine busy") {
        return ("⚠", "machine");
    }
    if reason == "dispatch paused" {
        return ("⏸", "paused");
    }
    let word = [
        ("waiting for a slot", "slot"),
        ("waiting for automatic coding capacity", "coding"),
        ("waiting for finishing", "finishing"),
        ("launch reservation held", "reserved"),
        ("dequeued", "dequeued"),
        ("project is not admitted", "not admitted"),
        ("waiting", "waiting"),
    ]
    .into_iter()
    .find_map(|(prefix, word)| reason.starts_with(prefix).then_some(word))
    .unwrap_or("blocked");
    ("⧗", word)
}

impl Compact<'_> {
    fn first_field_min_width(&self) -> usize {
        self.cfg
            .fields_for(self.t.status)
            .into_iter()
            .find_map(|f| {
                let text = match f {
                    CardField::Cost => Some(TaskCost::of(self.t).short()),
                    CardField::Profile => self
                        .route
                        .map(|r| r.label.clone())
                        .or_else(|| f.text(self.t, self.all, self.now, self.pinned)),
                    other => other.text(self.t, self.all, self.now, self.pinned),
                };
                text.map(|s| s.width())
            })
            .unwrap_or(0)
    }

    /// At a width too narrow for the id and the first field, the id shrinks to its numeric tail (`004`) so the
    /// field keeps its room; the title only gets what is left. The admission state follows the full id, as a word
    /// where it fits; when the full id leaves no room for its glyph, the glyph takes the gutter's blank cell.
    pub(super) fn line(
        &self,
        bar: Span<'static>,
        width: usize,
        title_style: Style,
    ) -> Line<'static> {
        let t = self.t;
        if t.tracker {
            let progress = self.plan_progress.map_or_else(
                || super::cards::tracker_progress(t, self.all),
                crate::plan::tracker::badge,
            );
            let progress = truncate(&progress, width);
            let room = width.saturating_sub(progress.width() + 1);
            let star = if self.pinned { "★ " } else { "" };
            let title = truncate(&format!("{star}{} {}", t.id, t.title), room);
            let pad = width.saturating_sub(title.width() + progress.width());
            return Line::from(vec![
                bar,
                Span::styled(title, title_style),
                Span::raw(" ".repeat(pad)),
                Span::styled(progress, Style::new().fg(theme::CYAN)),
            ]);
        }
        let star = if self.pinned { "★ " } else { "" };
        let first_min = if self.cfg.fields_for(t.status).first() == Some(&CardField::Profile) {
            self.first_field_min_width().min(width.saturating_sub(4))
        } else {
            self.first_field_min_width()
        };
        let full_id = t.id.width() + star.width() + 1 + first_min;
        let wait = self
            .wait
            .map(|reason| (wait_parts(reason), Style::new().fg(wait_color(reason))));
        let inline = wait.filter(|((glyph, _), _)| full_id + glyph.width() < width);
        let mut spans = if self.route_warning {
            vec![
                Span::styled(
                    bar.content.chars().next().unwrap_or(' ').to_string(),
                    bar.style,
                ),
                Span::styled("⚠", Style::new().fg(theme::YELLOW)),
            ]
        } else {
            match wait.filter(|_| inline.is_none()) {
                Some(((glyph, _), style)) => match bar.content.chars().next() {
                    Some(mark) if mark != ' ' => vec![
                        Span::styled(mark.to_string(), bar.style),
                        Span::styled(glyph, style),
                    ],
                    _ => vec![Span::styled(glyph, style), Span::raw(" ")],
                },
                None => vec![bar],
            }
        };
        let inline_min = inline.map_or(0, |((glyph, _), _)| glyph.width() + 1);
        let (id, star) = if full_id <= width {
            (t.id.clone(), star)
        } else {
            let room = width.saturating_sub(first_min + 1);
            let tail = t.id.rsplit('-').next().unwrap_or(&t.id);
            let id = if t.id.width() <= room {
                t.id.clone()
            } else if tail.width() <= room {
                tail.to_string()
            } else {
                let skip = tail.chars().count().saturating_sub(room);
                tail.chars().skip(skip).collect()
            };
            (id, "")
        };
        let mut used = id.width() + star.width() + 1;
        let fields = card_fields::field_spans(
            t,
            self.all,
            self.cfg,
            width.saturating_sub(used + inline_min),
            self.now,
            self.pinned,
            self.route.map(|r| r.label.as_str()),
        )
        .unwrap_or_default();
        let fields_w: usize = fields.iter().map(|s| s.width()).sum();
        let (wait, wait_style) =
            inline.map_or((String::new(), Style::new()), |((glyph, word), style)| {
                let full = format!("{glyph} {word}");
                if full.width() < width.saturating_sub(used + fields_w) {
                    (full, style)
                } else {
                    (glyph.to_string(), style)
                }
            });
        let wait_gap = if wait.is_empty() { "" } else { " " };
        used += wait.width() + wait_gap.len();
        let title_room = width.saturating_sub(used + fields_w + 1);
        let title = if title_room >= 4 {
            truncate(self.stage.unwrap_or(&t.title), title_room)
        } else {
            String::new()
        };
        let pad = width.saturating_sub(used + title.width() + fields_w);
        spans.extend([
            Span::styled(id, theme::bold()),
            Span::raw(" "),
            Span::styled(star, Style::new().fg(theme::YELLOW)),
            Span::styled(wait, wait_style),
            Span::raw(wait_gap),
            Span::styled(title, title_style),
            Span::raw(" ".repeat(pad)),
        ]);
        spans.extend(fields);
        Line::from(spans)
    }
}
