//! The one-line compact card: id, title and the configured fields, reserving the highest-priority field first.

use corral_core::{CardCfg, CardField, Task, TaskCost};
use jiff::Timestamp;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::card_fields;
use crate::overlays::truncate;
use crate::theme;

pub(super) struct Compact<'a> {
    pub t: &'a Task,
    pub all: &'a [Task],
    pub cfg: &'a CardCfg,
    pub now: Timestamp,
    pub pinned: bool,
    pub stage: Option<&'a str>,
}

impl Compact<'_> {
    fn first_field_min_width(&self) -> usize {
        self.cfg
            .fields_for(self.t.status)
            .into_iter()
            .find_map(|f| {
                let text = match f {
                    CardField::Cost => Some(TaskCost::of(self.t).short()),
                    other => other.text(self.t, self.all, self.now, self.pinned),
                };
                text.map(|s| s.width())
            })
            .unwrap_or(0)
    }

    /// At a width too narrow for the id and the first field, the id shrinks to its numeric tail (`004`) so the
    /// field keeps its room; the title only gets what is left.
    pub(super) fn line(
        &self,
        bar: Span<'static>,
        width: usize,
        title_style: Style,
    ) -> Line<'static> {
        let t = self.t;
        let star = if self.pinned { "★ " } else { "" };
        let first_min = self.first_field_min_width();
        let (id, star) = if t.id.width() + star.width() + 1 + first_min <= width {
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
        let used = id.width() + star.width() + 1;
        let fields = card_fields::field_spans(
            t,
            self.all,
            self.cfg,
            width.saturating_sub(used),
            self.now,
            self.pinned,
        )
        .unwrap_or_default();
        let fields_w: usize = fields.iter().map(|s| s.width()).sum();
        let title_room = width.saturating_sub(used + fields_w + 1);
        let title = if title_room >= 4 {
            truncate(self.stage.unwrap_or(&t.title), title_room)
        } else {
            String::new()
        };
        let pad = width.saturating_sub(used + title.width() + fields_w);
        let mut spans = vec![
            bar,
            Span::styled(id, theme::bold()),
            Span::raw(" "),
            Span::styled(star, Style::new().fg(theme::YELLOW)),
            Span::styled(title, title_style),
            Span::raw(" ".repeat(pad)),
        ];
        spans.extend(fields);
        Line::from(spans)
    }
}
