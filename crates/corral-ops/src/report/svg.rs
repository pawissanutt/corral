//! Inline SVG charts rendered in Rust: fixed viewBoxes scaled to the container width,
//! colours from CSS classes, no external references.
use super::html::escape;

const WIDTH: f64 = 640.0;
const ROW: f64 = 22.0;
const LABEL: f64 = 190.0;
const VALUE: f64 = 110.0;

fn fmt(x: f64) -> String {
    format!("{x:.1}")
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn empty() -> String {
    "<p class=\"empty\">No data in this window.</p>\n".into()
}

/// Horizontal bars: `(label, value, value text)`. Bars scale to the largest value.
pub(crate) fn hbars(title: &str, rows: &[(String, f64, String)]) -> String {
    if rows.is_empty() {
        return empty();
    }
    let max = rows.iter().map(|r| r.1).fold(0.0, f64::max);
    let span = WIDTH - LABEL - VALUE;
    let height = ROW * f64::from(u32::try_from(rows.len()).unwrap_or(u32::MAX)) + 4.0;
    let mut svg = format!(
        "<div class=\"scroll\"><svg class=\"chart\" role=\"img\" aria-label=\"{}\" viewBox=\"0 0 {WIDTH} {}\" preserveAspectRatio=\"xMinYMin meet\">\n",
        escape(title),
        fmt(height)
    );
    let mut y = 2.0;
    for (label, value, text) in rows {
        let w = if max > 0.0 { value / max * span } else { 0.0 };
        svg.push_str(&format!(
            "<text class=\"lbl\" x=\"{}\" y=\"{}\" text-anchor=\"end\">{}</text>\
<rect class=\"bar\" x=\"{LABEL}\" y=\"{}\" width=\"{}\" height=\"{}\"><title>{}: {}</title></rect>\
<text class=\"val\" x=\"{}\" y=\"{}\">{}</text>\n",
            fmt(LABEL - 6.0),
            fmt(y + 15.0),
            escape(&clip(label, 28)),
            fmt(y + 3.0),
            fmt(w.max(0.0)),
            fmt(ROW - 6.0),
            escape(label),
            escape(text),
            fmt(LABEL + w.max(0.0) + 4.0),
            fmt(y + 15.0),
            escape(text),
        ));
        y += ROW;
    }
    svg.push_str("</svg></div>\n");
    svg
}

/// Vertical columns over an ordered axis (days, buckets); every n-th label is shown.
pub(crate) fn columns(title: &str, rows: &[(String, f64, String)]) -> String {
    if rows.is_empty() {
        return empty();
    }
    let height = 180.0;
    let base = 150.0;
    let max = rows.iter().map(|r| r.1).fold(0.0, f64::max);
    let n = f64::from(u32::try_from(rows.len()).unwrap_or(u32::MAX));
    let slot = (WIDTH - 20.0) / n;
    let every = (rows.len() / 8).max(1);
    let mut svg = format!(
        "<div class=\"scroll\"><svg class=\"chart\" role=\"img\" aria-label=\"{}\" viewBox=\"0 0 {WIDTH} {height}\" preserveAspectRatio=\"xMinYMin meet\">\n\
<line class=\"axis\" x1=\"10\" y1=\"{base}\" x2=\"{}\" y2=\"{base}\"/>\n",
        escape(title),
        fmt(WIDTH - 10.0)
    );
    for (i, (label, value, text)) in rows.iter().enumerate() {
        let x = 10.0 + slot * f64::from(u32::try_from(i).unwrap_or(u32::MAX));
        let h = if max > 0.0 {
            value / max * (base - 14.0)
        } else {
            0.0
        };
        svg.push_str(&format!(
            "<rect class=\"bar\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"><title>{}: {}</title></rect>\n",
            fmt(x + slot * 0.1),
            fmt(base - h),
            fmt(slot * 0.8),
            fmt(h),
            escape(label),
            escape(text)
        ));
        if i % every == 0 {
            svg.push_str(&format!(
                "<text class=\"lbl\" x=\"{}\" y=\"{}\" text-anchor=\"middle\">{}</text>\n",
                fmt(x + slot / 2.0),
                fmt(base + 16.0),
                escape(&clip(label, 12))
            ));
        }
    }
    svg.push_str("</svg></div>\n");
    svg
}

/// One stacked bar of shares with a legend: `(label, value)`.
pub(crate) fn stacked(title: &str, parts: &[(String, f64)]) -> String {
    let total: f64 = parts.iter().map(|p| p.1).sum();
    if total <= 0.0 {
        return empty();
    }
    let mut svg = format!(
        "<div class=\"scroll\"><svg class=\"chart\" role=\"img\" aria-label=\"{}\" viewBox=\"0 0 {WIDTH} {}\" preserveAspectRatio=\"xMinYMin meet\">\n",
        escape(title),
        fmt(34.0 + ROW * f64::from(u32::try_from(parts.len()).unwrap_or(u32::MAX)))
    );
    let mut x = 0.0;
    for (i, (label, value)) in parts.iter().enumerate() {
        let w = value / total * WIDTH;
        svg.push_str(&format!(
            "<rect class=\"s{}\" x=\"{}\" y=\"2\" width=\"{}\" height=\"22\"><title>{}: {:.1}%</title></rect>\n",
            i % 6,
            fmt(x),
            fmt(w),
            escape(label),
            value / total * 100.0
        ));
        x += w;
    }
    let mut y = 34.0;
    for (i, (label, value)) in parts.iter().enumerate() {
        svg.push_str(&format!(
            "<rect class=\"s{}\" x=\"0\" y=\"{}\" width=\"12\" height=\"12\"/>\
<text class=\"lbl\" x=\"18\" y=\"{}\">{} {:.1}%</text>\n",
            i % 6,
            fmt(y + 2.0),
            fmt(y + 12.0),
            escape(label),
            value / total * 100.0
        ));
        y += ROW;
    }
    svg.push_str("</svg></div>\n");
    svg
}
