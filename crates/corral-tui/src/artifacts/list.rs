//! Importance-ordered artifact rows shared by task detail, review and the finder: keep-tier rows are bold with
//! a status and an excerpt, transient rows are dimmed under a collapsed "raw (N)" fold. Rows adapt to the
//! width: the status and tier badge always stay, the title is cut with an ellipsis, and the excerpt moves to
//! the selected item's summary line when it does not fit.

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Result;
use corral_core::{Artifact, ArtifactTier};
use corral_ops::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::ui::{KIND_COL, TIER_COL, human_bytes, tier_badge};
use crate::shelf;
use crate::theme::{self, FG, GREEN, MAGENTA, SELECTED_BG, YELLOW};
use crate::{Toast, Tone};

/// Rows at least this wide show the kind column, run and size.
const WIDE: usize = 40;

/// One display row: an artifact (index into the task's artifact vec) or the raw fold header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    Item(usize),
    Fold,
}

pub(crate) struct ArtList {
    pub fold_open: bool,
    reviewed: BTreeSet<String>,
    excerpts: HashMap<String, Option<String>>,
    order: Vec<usize>,
}

/// What a row needs besides the artifact itself.
pub(crate) struct RowInfo<'a> {
    pub reviewed: bool,
    pub excerpt: Option<&'a str>,
    pub pinned: bool,
    /// Columns available for the whole row.
    pub width: usize,
    /// Dim trailing text after the size (the finder's task id).
    pub extra: Option<String>,
}

/// `s` cut to at most `w` columns with a trailing ellipsis.
pub(crate) fn truncate(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    out
}

/// Review state of an artifact: keep-tier items are "new" until marked reviewed.
fn status(tier: ArtifactTier, reviewed: bool) -> Option<(&'static str, Style)> {
    match (tier, reviewed) {
        (ArtifactTier::Keep, false) => {
            Some(("new", Style::new().fg(YELLOW).add_modifier(Modifier::BOLD)))
        }
        (ArtifactTier::Keep, true) => Some(("✓", Style::new().fg(GREEN))),
        _ => None,
    }
}

/// Spans for one artifact row, fitted to `info.width`.
pub(crate) fn row_spans(a: &Artifact, info: &RowInfo) -> Vec<Span<'static>> {
    let tier = a.tier();
    let dim = theme::dim();
    let (mark, kind_style, title_style) = match tier {
        ArtifactTier::Keep => (
            Span::styled("◆ ", Style::new().fg(GREEN)),
            Style::new().fg(MAGENTA).add_modifier(Modifier::BOLD),
            Style::new()
                .fg(if info.reviewed { FG } else { YELLOW })
                .add_modifier(Modifier::BOLD),
        ),
        ArtifactTier::Normal => (
            Span::raw("◆ "),
            Style::new().fg(MAGENTA),
            Style::new().fg(FG),
        ),
        ArtifactTier::Transient => (Span::styled("◇ ", dim), dim, dim),
    };
    let mark = if info.pinned { shelf::star(true) } else { mark };
    let wide = info.width >= WIDE;
    let status = status(tier, info.reviewed);
    let mut spans = vec![mark, tier_badge(tier)];
    let mut used = 2 + TIER_COL;
    if wide {
        spans.push(Span::styled(
            format!("{:<KIND_COL$}", a.kind.as_str()),
            kind_style,
        ));
        used += KIND_COL;
    }
    let tail = wide.then(|| {
        format!(
            "  {} · {}{}",
            a.run,
            human_bytes(a.bytes),
            info.extra
                .as_deref()
                .map(|e| format!("  {e}"))
                .unwrap_or_default()
        )
    });
    let status_w = status.map_or(0, |(t, _)| t.width() + 1);
    let tail_w = tail.as_ref().map_or(0, |t| t.width());
    let avail = info.width.saturating_sub(used + status_w + tail_w).max(8);
    let title = truncate(&a.title, avail);
    let title_w = title.width();
    spans.push(Span::styled(title, title_style));
    if let Some((text, style)) = status {
        spans.push(Span::styled(format!(" {text}"), style));
    }
    let room = avail.saturating_sub(title_w);
    if let Some(e) = info
        .excerpt
        .filter(|_| tier == ArtifactTier::Keep && room >= 14)
    {
        spans.push(Span::styled(
            format!(" — {}", truncate(e, room - 3)),
            dim.add_modifier(Modifier::ITALIC),
        ));
    }
    if let Some(tail) = tail {
        spans.push(Span::styled(tail, dim));
    }
    spans
}

/// The selected artifact's one-line summary: tier, review state and its excerpt, cut to `width`.
pub(crate) fn summary_line(
    a: &Artifact,
    reviewed: bool,
    excerpt: Option<&str>,
    width: usize,
) -> Line<'static> {
    let tier = a.tier();
    let state = match status(tier, reviewed) {
        Some(("new", _)) => "unreviewed",
        Some(_) => "reviewed",
        None => "",
    };
    let head = [tier.as_str(), state]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" · ");
    let text = match excerpt {
        Some(e) => format!("↳ {e} · {head}"),
        None => format!("↳ {head}"),
    };
    Line::from(Span::styled(
        truncate(&text, width),
        theme::dim().add_modifier(Modifier::ITALIC),
    ))
}

impl ArtList {
    pub fn new(ctx: &Ctx, arts: &[Artifact]) -> ArtList {
        let mut l = ArtList {
            fold_open: false,
            reviewed: BTreeSet::new(),
            excerpts: HashMap::new(),
            order: Vec::new(),
        };
        l.sync(ctx, arts);
        l
    }

    /// Recomputes the importance order, reviewed marks and keep-tier excerpts.
    pub fn sync(&mut self, ctx: &Ctx, arts: &[Artifact]) {
        self.reviewed = corral_ops::reviewed_set(ctx);
        self.order = (0..arts.len()).collect();
        self.order
            .sort_by(|&a, &b| corral_ops::cmp_importance(&arts[a], &arts[b]));
        self.excerpts
            .retain(|p, _| arts.iter().any(|a| &a.path == p));
        for a in arts.iter().filter(|a| a.tier() == ArtifactTier::Keep) {
            self.excerpts
                .entry(a.path.clone())
                .or_insert_with(|| corral_ops::excerpt(ctx, a));
        }
    }

    pub fn is_reviewed(&self, a: &Artifact) -> bool {
        self.reviewed.contains(&a.path)
    }

    pub fn excerpt(&self, a: &Artifact) -> Option<&str> {
        self.excerpts.get(&a.path).and_then(|e| e.as_deref())
    }

    pub fn raw_count(&self, arts: &[Artifact]) -> usize {
        arts.iter()
            .filter(|a| a.tier() == ArtifactTier::Transient)
            .count()
    }

    pub fn unreviewed_keep(&self, arts: &[Artifact]) -> usize {
        arts.iter()
            .filter(|a| a.tier() == ArtifactTier::Keep && !self.is_reviewed(a))
            .count()
    }

    /// Display rows in importance order; transient items follow a fold header and show only when it is open.
    pub fn rows(&self, arts: &[Artifact]) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut folded = false;
        for &i in &self.order {
            if arts[i].tier() == ArtifactTier::Transient {
                if !folded {
                    rows.push(Row::Fold);
                    folded = true;
                }
                if !self.fold_open {
                    continue;
                }
            }
            rows.push(Row::Item(i));
        }
        rows
    }

    fn visible(&self, arts: &[Artifact]) -> Vec<usize> {
        self.rows(arts)
            .into_iter()
            .filter_map(|r| match r {
                Row::Item(i) => Some(i),
                Row::Fold => None,
            })
            .collect()
    }

    /// The most important visible artifact (first in importance order).
    pub fn first(&self, arts: &[Artifact]) -> usize {
        self.visible(arts).first().copied().unwrap_or(0)
    }

    /// Opens or closes the raw fold, keeping the selection on a visible row.
    pub fn toggle_fold(&mut self, arts: &[Artifact], sel: &mut usize) {
        self.fold_open = !self.fold_open;
        self.clamp(arts, sel);
    }

    /// Display row of artifact `sel` (0 when hidden), for scroll anchoring.
    pub fn row_of(&self, arts: &[Artifact], sel: usize) -> usize {
        self.rows(arts)
            .iter()
            .position(|r| *r == Row::Item(sel))
            .unwrap_or(0)
    }

    /// Moves `sel` `delta` visible rows through the importance order.
    pub fn step(&self, arts: &[Artifact], sel: &mut usize, delta: isize) {
        let v = self.visible(arts);
        if v.is_empty() {
            return;
        }
        let pos = v.iter().position(|&i| i == *sel).unwrap_or(0);
        *sel = v[pos.saturating_add_signed(delta).min(v.len() - 1)];
    }

    /// Keeps `sel` on a visible artifact (after a refresh or a fold change).
    pub fn clamp(&self, arts: &[Artifact], sel: &mut usize) {
        let v = self.visible(arts);
        if !v.contains(sel) {
            *sel = v.first().copied().unwrap_or(0);
        }
    }

    /// Selects the next keep-tier artifact, unreviewed ones first. False when nothing is keep-tier.
    pub fn next_important(&self, arts: &[Artifact], sel: &mut usize) -> bool {
        let keeps: Vec<usize> = self
            .order
            .iter()
            .copied()
            .filter(|&i| arts[i].tier() == ArtifactTier::Keep)
            .collect();
        if keeps.is_empty() {
            return false;
        }
        let start = keeps.iter().position(|&i| i == *sel).map_or(0, |p| p + 1);
        let rotated: Vec<usize> = (0..keeps.len())
            .map(|k| keeps[(start + k) % keeps.len()])
            .collect();
        let pick = rotated
            .iter()
            .copied()
            .find(|&i| !self.is_reviewed(&arts[i]))
            .unwrap_or(rotated[0]);
        *sel = pick;
        true
    }

    /// Toggles the reviewed check mark of artifact `sel`; returns the new state.
    pub fn toggle_reviewed(
        &mut self,
        ctx: &Ctx,
        arts: &[Artifact],
        sel: usize,
    ) -> Result<Option<bool>> {
        let Some(a) = arts.get(sel) else {
            return Ok(None);
        };
        let now = corral_ops::toggle_reviewed(ctx, a)?;
        self.reviewed = corral_ops::reviewed_set(ctx);
        Ok(Some(now))
    }

    /// Shared list keys: j/k and arrows move, `f` folds raw items, `n` walks keep-tier items (unreviewed
    /// first), `v` toggles the reviewed mark. None when the key is not a list key.
    pub fn on_key(
        &mut self,
        ctx: &Ctx,
        arts: &[Artifact],
        sel: &mut usize,
        key: &KeyEvent,
    ) -> Option<Option<Toast>> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.step(arts, sel, 1),
            KeyCode::Char('k') | KeyCode::Up => self.step(arts, sel, -1),
            KeyCode::Char('f') => self.toggle_fold(arts, sel),
            KeyCode::Char('n') => {
                if !self.next_important(arts, sel) {
                    return Some(Some(Toast::new(
                        Tone::Info,
                        "Nothing important",
                        "no keep-tier artifacts",
                    )));
                }
            }
            KeyCode::Char('v') => {
                return Some(match self.toggle_reviewed(ctx, arts, *sel) {
                    Ok(Some(true)) => Some(Toast::new(Tone::Info, "Reviewed", "marked reviewed")),
                    Ok(Some(false)) => {
                        Some(Toast::new(Tone::Info, "Reviewed", "review mark removed"))
                    }
                    Ok(None) => None,
                    Err(e) => Some(Toast::new(
                        Tone::Bad,
                        "Could not mark reviewed",
                        corral_herdr::friendly_error(&e),
                    )),
                });
            }
            _ => return None,
        }
        Some(None)
    }

    /// Row lines fitted to `width`, then a blank and the selected artifact's summary; the selected artifact
    /// is highlighted when `focused`.
    pub fn lines(
        &self,
        arts: &[Artifact],
        task_id: &str,
        sel: usize,
        focused: bool,
        pins: &HashSet<String>,
        width: usize,
    ) -> Vec<Line<'static>> {
        let raw = self.raw_count(arts);
        let mut lines: Vec<Line<'static>> = self
            .rows(arts)
            .into_iter()
            .map(|r| match r {
                Row::Fold => {
                    let (glyph, hint) = if self.fold_open {
                        ("▾", "f hide")
                    } else {
                        ("▸", "f show")
                    };
                    Line::from(Span::styled(
                        format!("{glyph} raw ({raw})  {hint}"),
                        theme::dim(),
                    ))
                }
                Row::Item(i) => {
                    let a = &arts[i];
                    let info = RowInfo {
                        reviewed: self.is_reviewed(a),
                        excerpt: self.excerpt(a),
                        pinned: pins.contains(&shelf::artifact_ref(task_id, a)),
                        width,
                        extra: None,
                    };
                    let mut spans = row_spans(a, &info);
                    if focused && i == sel {
                        spans
                            .iter_mut()
                            .for_each(|s| s.style = s.style.bg(SELECTED_BG));
                        return Line::from(spans).style(Style::new().bg(SELECTED_BG));
                    }
                    Line::from(spans)
                }
            })
            .collect();
        if let Some(a) = arts.get(sel) {
            lines.push(Line::default());
            lines.push(summary_line(a, self.is_reviewed(a), self.excerpt(a), width));
        }
        lines
    }
}
