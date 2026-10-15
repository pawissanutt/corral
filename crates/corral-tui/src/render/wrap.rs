//! Word wrapping and box tables over styled text, shared by the Markdown and CSV renderers.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::{self, DIM};

pub(crate) type Seg = (String, Style);
pub(crate) type Chunk = Vec<Seg>;

/// Splits styled segments into words; pieces with no whitespace between them stay in one chunk.
pub(crate) fn chunks(segs: &[Seg]) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut cur: Chunk = Vec::new();
    for (text, style) in segs {
        let mut buf = String::new();
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !buf.is_empty() {
                    cur.push((std::mem::take(&mut buf), *style));
                }
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            } else {
                buf.push(ch);
            }
        }
        if !buf.is_empty() {
            cur.push((buf, *style));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn chunk_width(c: &Chunk) -> usize {
    c.iter().map(|(t, _)| t.width()).sum()
}

fn push_chunk(cur: &mut Vec<Span<'static>>, c: &Chunk) {
    cur.extend(c.iter().map(|(t, s)| Span::styled(t.clone(), *s)));
}

/// Greedy word wrap at `width` columns; a word wider than the width is split by character.
pub(crate) fn wrap(chunks: &[Chunk], width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut w = 0;
    for c in chunks {
        let cw = chunk_width(c);
        let need = if w == 0 { cw } else { w + 1 + cw };
        if need <= width {
            if w > 0 {
                cur.push(Span::raw(" "));
            }
            push_chunk(&mut cur, c);
            w = need;
            continue;
        }
        if w > 0 {
            lines.push(std::mem::take(&mut cur));
            w = 0;
        }
        if cw <= width {
            push_chunk(&mut cur, c);
            w = cw;
            continue;
        }
        for (t, s) in c {
            for ch in t.chars() {
                let cwid = ch.width().unwrap_or(0);
                if w + cwid > width {
                    lines.push(std::mem::take(&mut cur));
                    w = 0;
                }
                cur.push(Span::styled(ch.to_string(), *s));
                w += cwid;
            }
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

fn column_widths(rows: &[Vec<Vec<Seg>>], cols: usize, avail: usize) -> Vec<usize> {
    let natural: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|cell| cell.iter().map(|(t, _)| t.width()).sum::<usize>())
                .max()
                .unwrap_or(0)
                .max(1)
        })
        .collect();
    let total: usize = natural.iter().sum();
    if total <= avail {
        return natural;
    }
    let longest_word: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .flat_map(|cell| chunks(cell))
                .map(|w| chunk_width(&w))
                .max()
                .unwrap_or(1)
                .clamp(1, natural[c])
        })
        .collect();
    let mut widths: Vec<usize> = (0..cols)
        .map(|c| (natural[c] * avail / total).max(longest_word[c]))
        .collect();
    while widths.iter().sum::<usize>() > avail.max(cols) {
        let shrinkable = (0..cols).filter(|&i| widths[i] > longest_word[i]);
        let Some(i) = shrinkable.max_by_key(|&i| widths[i]) else {
            break;
        };
        widths[i] -= 1;
    }
    while widths.iter().sum::<usize>() > avail.max(cols) {
        let Some(i) = (0..cols)
            .filter(|&i| widths[i] > 1)
            .max_by_key(|&i| widths[i])
        else {
            break;
        };
        widths[i] -= 1;
    }
    widths
}

fn border(widths: &[usize], l: &str, m: &str, r: &str) -> Line<'static> {
    let mut s = String::from(l);
    for (i, w) in widths.iter().enumerate() {
        s.push_str(&"─".repeat(w + 2));
        s.push_str(if i + 1 == widths.len() { r } else { m });
    }
    Line::from(Span::styled(s, theme::dim()))
}

/// A box table fitted to `width`: cells wrap inside their column, the first row is bold when `header`.
pub(crate) fn table(rows: &[Vec<Vec<Seg>>], width: usize, header: bool) -> Vec<Line<'static>> {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return Vec::new();
    }
    let avail = width.saturating_sub(3 * cols + 1);
    let widths = column_widths(rows, cols, avail);
    let bar = Span::styled("│", Style::new().fg(DIM));
    let mut out = vec![border(&widths, "┌", "┬", "┐")];
    for (ri, row) in rows.iter().enumerate() {
        let bold = header && ri == 0;
        let cells: Vec<Vec<Vec<Span<'static>>>> = (0..cols)
            .map(|c| {
                let segs: Vec<Seg> = row
                    .get(c)
                    .map(|cell| {
                        cell.iter()
                            .map(|(t, s)| {
                                let s = if bold {
                                    s.add_modifier(Modifier::BOLD)
                                } else {
                                    *s
                                };
                                (t.clone(), s)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                wrap(&chunks(&segs), widths[c])
            })
            .collect();
        let height = cells.iter().map(Vec::len).max().unwrap_or(1);
        for line in 0..height {
            let mut spans = vec![bar.clone()];
            for (c, cell) in cells.iter().enumerate() {
                let content = cell.get(line).cloned().unwrap_or_default();
                let used: usize = content.iter().map(|s| s.content.width()).sum();
                spans.push(Span::raw(" "));
                spans.extend(content);
                spans.push(Span::raw(" ".repeat(widths[c].saturating_sub(used) + 1)));
                spans.push(bar.clone());
            }
            out.push(Line::from(spans));
        }
        if bold && rows.len() > 1 {
            out.push(border(&widths, "├", "┼", "┤"));
        }
    }
    out.push(border(&widths, "└", "┴", "┘"));
    out
}
