//! Wait and owner-decision readback shared by board cards and the detail view.

use corral_core::{ResumeDelivery, Run, WaitRecord, WaitState};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::theme::{BLUE, DIM, GREEN, RED, YELLOW};

/// Pending is blue (healthy, like a running run), ready green, cancelled dim; uncertain states are yellow and
/// broken ones red.
pub(crate) fn state_color(state: WaitState) -> Color {
    match state {
        WaitState::Pending => BLUE,
        WaitState::Ready => GREEN,
        WaitState::Cancelled => DIM,
        WaitState::Unsupported | WaitState::Unverifiable | WaitState::DeliveryUnknown => YELLOW,
        WaitState::Failed | WaitState::Lost | WaitState::Expired | WaitState::MismatchedRun => RED,
    }
}

pub(crate) fn state_icon(state: WaitState) -> &'static str {
    match state {
        WaitState::Pending => "⧗",
        WaitState::Ready => "✓",
        WaitState::Cancelled => "·",
        WaitState::Failed
        | WaitState::Lost
        | WaitState::Expired
        | WaitState::Unsupported
        | WaitState::Unverifiable
        | WaitState::MismatchedRun
        | WaitState::DeliveryUnknown => "⚠",
    }
}

pub(crate) fn state_span(state: WaitState) -> Span<'static> {
    Span::styled(
        format!("{} {}", state_icon(state), state.as_str()),
        Style::new().fg(state_color(state)),
    )
}

/// Waits still worth a card line: all but cancelled ones and ready ones whose resume was acknowledged.
pub(crate) fn open_waits(run: &Run) -> impl Iterator<Item = &WaitRecord> {
    run.obligations.waits.iter().filter(|w| {
        let delivered = w
            .receipt
            .as_ref()
            .is_some_and(|r| r.delivery == ResumeDelivery::Acknowledged);
        !(w.settled_job_exit()
            || w.observation.state == WaitState::Cancelled
            || w.observation.state == WaitState::Ready && delivered)
    })
}

/// True while the run legitimately waits on a declared condition (so "idle" is expected, not a warning).
pub(crate) fn waiting(run: &Run) -> bool {
    open_waits(run).any(|w| w.observation.state == WaitState::Pending)
}

/// The Running card's state for a run that released its coding slot.
pub(crate) fn parked_label(task: &str, run: &Run) -> Option<String> {
    if let Some(slot) = run
        .obligations
        .slot_wait
        .as_ref()
        .filter(|_| run.outcome.is_none())
    {
        return Some(format!(
            "⧗ woken from {} · queued for a slot",
            slot.on.join(", ")
        ));
    }
    let on = run.parked_on(task);
    (!on.is_empty()).then(|| format!("⧗ parked on {} · slot released", on.join(", ")))
}

/// Card lines for a run's obligations: one `<state> <condition>` line per open wait, the pending owner
/// question when `with_decision`, and answered decisions whose delivery is unknown.
pub(crate) fn card_lines(run: &Run, with_decision: bool) -> Vec<Line<'static>> {
    card_lines_from(&run.obligations, with_decision)
}

pub(crate) fn card_lines_from(
    obligations: &corral_core::RunObligations,
    with_decision: bool,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for d in &obligations.decisions {
        let state = d.observation.state;
        if (d.pending() && with_decision) || (!d.pending() && state == WaitState::DeliveryUnknown) {
            out.push(Line::from(vec![
                Span::styled("? ", Style::new().fg(YELLOW)),
                state_span(state),
                Span::styled(format!(" {}", d.request.question), Style::new().fg(YELLOW)),
            ]));
        }
    }
    let settled_jobs = obligations
        .waits
        .iter()
        .filter(|wait| wait.settled_job_exit())
        .count();
    if settled_jobs > 0 {
        out.push(Line::from(format!("jobs: {settled_jobs} settled")));
    }
    for w in obligations.waits.iter().filter(|w| {
        !(w.settled_job_exit()
            || w.observation.state == WaitState::Cancelled
            || w.observation.state == WaitState::Ready
                && w.receipt
                    .as_ref()
                    .is_some_and(|r| r.delivery == ResumeDelivery::Acknowledged))
    }) {
        out.push(Line::from(vec![
            state_span(w.observation.state),
            Span::styled(format!(" {}", w.request.condition), crate::theme::dim()),
        ]));
    }
    out
}
