//! Runs-tab readback of one run's live state: the stuck / idle warning and every persistent wait and owner
//! decision record (request, observation, receipt); plus the header badge and the no-pane decision fallback.

use corral_core::{
    DecisionRecord, ResumeReceipt, Run, WaitHandle, WaitObservation, WaitRecord, WaitState,
};
use jiff::Timestamp;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::artifacts::ui;
use crate::obligations::{state_color, state_span, waiting};
use crate::theme::{self, FG, RED, YELLOW};

pub(super) fn lines(r: &Run, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let (Some(s), None) = (&r.stuck, r.outcome) {
        lines.push(Line::from(Span::styled(
            format!(
                "⚠ stuck: {} (pid {}, started {}); press u to unstick",
                s.argv.join(" "),
                s.pid,
                s.started
            ),
            Style::new().fg(RED),
        )));
    } else if r.idle_no_report && r.outcome.is_none() && !waiting(r) {
        let idle = r
            .idle_since
            .map(|since| format!(" for {}", ui::elapsed(since, None)))
            .unwrap_or_default();
        lines.push(Line::from(Span::styled(
            format!("⚠ idle, no report: the agent looks idle{idle} without `corral done`"),
            Style::new().fg(YELLOW),
        )));
    }
    let o = &r.obligations;
    if o.is_empty() {
        return lines;
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("obligations", theme::bold())));
    for d in &o.decisions {
        lines.extend(decision_lines(d, width));
    }
    for w in &o.waits {
        lines.extend(wait_lines(w, width));
    }
    lines
}

fn title(kind: &str, id: &str, state: WaitState, owner: &str, source: String) -> Line<'static> {
    Line::from(vec![
        state_span(state),
        Span::styled(format!(" {kind} {id}"), theme::bold()),
        Span::styled(format!(" · owner {owner} · source {source}"), theme::dim()),
    ])
}

fn field(label: &str, text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let head = format!("  {label}: ");
    let room = width.saturating_sub(head.width()).max(10);
    wrap(text, room)
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| {
            let lead = if i == 0 {
                Span::styled(head.clone(), theme::dim())
            } else {
                Span::raw(" ".repeat(head.width()))
            };
            Line::from(vec![lead, Span::styled(chunk, style)])
        })
        .collect()
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines() {
        let mut cur = String::new();
        for word in para.split(' ') {
            if !cur.is_empty() && cur.width() + 1 + word.width() > width {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn when(registered: Timestamp, deadline: Option<Timestamp>, width: usize) -> Vec<Line<'static>> {
    let deadline = deadline.map_or_else(|| "none".to_string(), |d| d.to_string());
    let mut lines = field("registered", &registered.to_string(), width, theme::dim());
    lines.extend(field("deadline", &deadline, width, theme::dim()));
    lines
}

fn observed(o: &WaitObservation, width: usize) -> Vec<Line<'static>> {
    let style = Style::new().fg(state_color(o.state));
    let text = if o.detail.is_empty() {
        "—"
    } else {
        &o.detail
    };
    field(
        "observed",
        &format!("{} {}: {text}", o.at, o.state.as_str()),
        width,
        style,
    )
}

fn receipt(r: Option<&ResumeReceipt>, width: usize) -> Vec<Line<'static>> {
    let Some(r) = r else {
        return vec![Line::styled("  receipt: none", theme::dim())];
    };
    let outcome = format!("{:?}", r.outcome).to_lowercase();
    let delivery = format!("{:?}", r.delivery).to_lowercase();
    let style = match r.delivery {
        corral_core::ResumeDelivery::Acknowledged => Style::new().fg(FG),
        corral_core::ResumeDelivery::Unknown => Style::new().fg(YELLOW),
    };
    let mut out = field(
        "receipt",
        &format!(
            "{}: {outcome} · delivery {delivery} · {}",
            r.id, r.observed_at
        ),
        width,
        style,
    );
    if let Some(a) = &r.answer {
        out.extend(field("answer", a, width, Style::new().fg(FG)));
    }
    out
}

fn decision_lines(d: &DecisionRecord, width: usize) -> Vec<Line<'static>> {
    let q = &d.request;
    let source = format!("{:?}", q.source).to_lowercase();
    let mut out = vec![title(
        "decision",
        &q.id,
        d.observation.state,
        &q.owner,
        source,
    )];
    out.extend(field(
        "question",
        &q.question,
        width,
        Style::new().fg(YELLOW),
    ));
    out.extend(field(
        "action",
        &q.requested_action,
        width,
        Style::new().fg(FG),
    ));
    out.extend(when(d.registered_at, q.deadline, width));
    out.extend(observed(&d.observation, width));
    out.extend(receipt(d.receipt.as_ref(), width));
    out
}

fn wait_lines(w: &WaitRecord, width: usize) -> Vec<Line<'static>> {
    let q = &w.request;
    let source = format!("{:?}", q.source).to_lowercase();
    let mut out = vec![title("wait", &q.id, w.observation.state, &q.owner, source)];
    out.extend(field("condition", &q.condition, width, Style::new().fg(FG)));
    let handle = match &q.handle {
        WaitHandle::Gate { sha, pane } => format!("gate {sha} · pane {pane}"),
        WaitHandle::Channel {
            channel_id,
            capability,
        } => match capability {
            Some(c) => format!("channel {channel_id} ({c})"),
            None => format!("channel {channel_id}"),
        },
    };
    out.extend(field("handle", &handle, width, theme::dim()));
    out.extend(when(w.registered_at, q.deadline, width));
    out.extend(observed(&w.observation, width));
    out.extend(receipt(w.receipt.as_ref(), width));
    out
}

/// Header badge: the pending owner decision, else the first open wait in its state color.
pub(super) fn badge(r: &Run) -> Option<Span<'static>> {
    if let Some(d) = r.obligations.pending_decision() {
        return Some(Span::styled(
            format!("  ? decision {}", d.request.id),
            Style::new().fg(YELLOW),
        ));
    }
    let w = crate::obligations::open_waits(r).next()?;
    let state = w.observation.state;
    Some(Span::styled(
        format!(
            "  {} wait {} {}",
            crate::obligations::state_icon(state),
            w.request.id,
            state.as_str()
        ),
        Style::new().fg(state_color(state)),
    ))
}

/// Live box without a pane: the exact pending owner question and requested action, ahead of any excerpt.
pub(super) fn decision_fallback(r: &Run, width: usize) -> Option<Vec<Line<'static>>> {
    let d = r.obligations.pending_decision()?;
    let mut out = vec![Line::styled(
        format!(
            "decision {} for {} (pane not live):",
            d.request.id, d.request.owner
        ),
        theme::dim(),
    )];
    out.extend(field(
        "question",
        &d.request.question,
        width,
        Style::new().fg(YELLOW),
    ));
    out.extend(field(
        "action",
        &d.request.requested_action,
        width,
        Style::new().fg(FG),
    ));
    Some(out)
}
