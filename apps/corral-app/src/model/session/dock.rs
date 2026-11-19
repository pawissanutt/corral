//! The dock above the composer: what waits on the owner (the host's open cards) and the owner's
//! queued input, each present only when it has content, in the terminal UI's order (a docked
//! choice, owner questions, the queue). This is the frame the answerable cards and the queue
//! controls fill: it lists them as the host's state briefs them, and says where they are
//! answered while the page cannot answer them itself.

use corral_protocol::detail::CardKind;
use corral_protocol::session_state::{InputOrigin, SendPolicy};

use crate::model::hue::Hue;

use super::{Cx, Host};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dock {
    /// A docked choice (an exit choice, a child's question, a proposed board plan): the first.
    pub choice: Option<DockCard>,
    /// Owner questions and decisions waiting, oldest first.
    pub cards: Vec<DockCard>,
    pub queue: Option<QueueSummary>,
    /// Why the cards cannot be answered from here now, when they cannot.
    pub inert: Option<String>,
}

impl Dock {
    pub fn is_empty(&self) -> bool {
        self.choice.is_none() && self.cards.is_empty() && self.queue.is_none()
    }
}

/// One waiting card as the host's state briefs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockCard {
    /// `dock-<card>`.
    pub id: String,
    pub card: String,
    pub kind: CardKind,
    /// `▲ a2 asks: which API?`, `An owner question is waiting`.
    pub title: String,
    pub child: Option<String>,
    /// When the host gives up waiting, as the host stamped it.
    pub expires_at: Option<String>,
    pub hue: Hue,
}

/// The queue's header and its first rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSummary {
    /// `QUEUE · 2 waiting · all run when the agent is next idle`.
    pub header: String,
    pub rows: Vec<QueueLine>,
    /// `+3 more` beyond the rows the host listed.
    pub more: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueLine {
    /// `queue-<n>`.
    pub id: String,
    /// `→ parent`, `→ a2`.
    pub to: String,
    pub steer: bool,
    /// The text as the host lists it (at most 200 bytes).
    pub text: String,
    /// `inbox`, `extension`: input that is not the owner's, read-only.
    pub source: Option<&'static str>,
}

fn docked(kind: CardKind) -> bool {
    matches!(
        kind,
        CardKind::Exit | CardKind::ChildQuestion | CardKind::BoardPlan
    )
}

fn card(brief: &corral_protocol::session_state::CardBrief) -> DockCard {
    let title = match (&brief.kind, &brief.child) {
        (CardKind::ChildQuestion, Some(child)) => format!("▲ {child} asks: {}", brief.title),
        _ => brief.title.clone(),
    };
    DockCard {
        id: format!("dock-{}", brief.id),
        card: brief.id.clone(),
        kind: brief.kind,
        title,
        child: brief.child.clone(),
        expires_at: brief.expires_at.clone(),
        hue: Hue::You,
    }
}

fn queue(cx: &Cx<'_>) -> Option<QueueSummary> {
    let state = cx.state?;
    if state.queue_total == 0 && state.queue.is_empty() {
        return None;
    }
    let receiver = match cx.role {
        super::SessionRole::Build => "parent",
        _ => "agent",
    };
    let follow_ups = state
        .queue
        .iter()
        .all(|item| item.policy == SendPolicy::FollowUp);
    let mut header = format!("QUEUE · {} waiting", state.queue_total);
    if follow_ups {
        header.push_str(" · all run when the agent is next idle");
    }
    let listed = u32::try_from(state.queue.len()).unwrap_or(u32::MAX);
    Some(QueueSummary {
        header,
        rows: state
            .queue
            .iter()
            .enumerate()
            .map(|(n, item)| QueueLine {
                id: format!("queue-{n}"),
                to: format!("→ {}", item.to.as_deref().unwrap_or(receiver)),
                steer: item.policy == SendPolicy::Steer,
                text: item.text.clone(),
                source: match item.origin {
                    InputOrigin::Owner => None,
                    InputOrigin::Auto => Some("automated"),
                    InputOrigin::Pane => Some("terminal"),
                    InputOrigin::Other => Some("other source"),
                },
            })
            .collect(),
        more: (state.queue_total > listed).then(|| format!("+{} more", state.queue_total - listed)),
    })
}

/// The dock of the session `cx` reads.
pub fn dock(cx: &Cx<'_>) -> Dock {
    let Some(state) = cx.state else {
        return Dock::default();
    };
    let mut choice = None;
    let mut cards = Vec::new();
    for brief in &state.cards {
        if choice.is_none() && docked(brief.kind) {
            choice = Some(card(brief));
        } else {
            cards.push(card(brief));
        }
    }
    let inert = match cx.host {
        _ if cx.feed.link != crate::model::daemon::Link::Up => {
            Some("answer when corrald is back".to_owned())
        }
        Host::NotRunning | Host::Unreachable | Host::Ended => {
            Some("the host must be running to take an answer".to_owned())
        }
        _ => None,
    }
    .filter(|_| choice.is_some() || !cards.is_empty());
    Dock {
        choice,
        cards,
        queue: queue(cx),
        inert,
    }
}
