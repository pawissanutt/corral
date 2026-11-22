//! The sample build's plan mode, answered as corral-pi answers it through corrald: the plan
//! document served in the host's 40 KiB pages, the review card that carries its first 8 KiB,
//! the owner's answer (an execute option starts and leaves plan mode, Keep planning delivers the
//! feedback and returns to planning, a dismissal keeps the card) and the Plan mode action.

use corral_protocol::detail::{
    CardKind, CardOption, CardQuestion, DetailBody, DetailRef, DetailReply, PlanModeDocPage,
    SessionCard,
};
use corral_protocol::session_host::{SessionActionDone, SessionCardAnswer};
use corral_protocol::session_state::{PlanModeBrief, PlanModePhase};
use serde_json::{Value, json};

use super::build_ops::answered;
use super::parts::Parts;
use super::pi::Host;
use super::pi_rows::settle;
use super::stamp;
use crate::model::Id;
use crate::model::daemon::{DetailCache, Feed, detail_key};

/// The host's page of the document, and what a card carries of it.
const PAGE: usize = 40 * 1024;
const CARD_BODY: usize = 8 * 1024;

const TITLE: &str = "Preview · sample plan: redirect the old guide";
const REVISION: u32 = 2;
const DOC: &str = "\
# Redirect the old guide

Preview · sample plan. Nothing here was planned by an agent.

## Context

Review's open finding is where links to the old guide go. `docs/guide.md` was renamed to
`docs/parity-guide.md`; three pages and the README still link to the old path.

## Approach

1. Add `docs/guide.md` back as a stub that names the new page and links to it.
2. Point the README and the three linking pages at `docs/parity-guide.md`.
3. Add the old path to `docs/compatibility.md` under *Renamed pages*.

| Step | File | Change |
|---|---|---|
| 1 | `docs/guide.md` | stub with one link |
| 2 | `README.md`, `docs/*.md` | four links |
| 3 | `docs/compatibility.md` | one row |

## Verification

- `test -f docs/guide.md`
- `grep -rn \"guide.md\" docs README.md` lists only the stub and the compatibility row.

## Assumptions and contingencies

- The stub stays for one release; removing it is a follow-up on the board.
";

/// The longest start of `text` within `max` bytes that ends on a character.
fn head(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn parts(host: &mut Host) -> Result<&mut Parts, String> {
    host.parts
        .as_deref_mut()
        .ok_or_else(|| "this session's host has no plan mode".to_owned())
}

fn option(id: &str, label: &str, detail: &str) -> CardOption {
    CardOption {
        id: id.into(),
        label: label.into(),
        detail: Some(detail.into()),
        unavailable: None,
        needs_text: None,
        destructive: false,
    }
}

fn card(doc: &str, model: Option<&str>, at: String) -> SessionCard {
    let body = head(doc, CARD_BODY);
    SessionCard {
        id: format!("plan-{REVISION}-3fa9c1d2"),
        kind: CardKind::PlanReview,
        title: TITLE.into(),
        meta: Some(match model {
            Some(model) => format!("revision {REVISION} · {model}"),
            None => format!("revision {REVISION}"),
        }),
        body: Some(body.to_owned()),
        body_more: body.len() < doc.len(),
        questions: vec![CardQuestion {
            header: None,
            text: "How should this plan proceed?".into(),
            options: vec![
                option("execute", "Execute", "Keep the conversation as it is"),
                option(
                    "execute_clean",
                    "Execute with clean context",
                    "Replace the context with the plan",
                ),
                option(
                    "execute_compact",
                    "Execute with compacted context",
                    "Summarise the planning discussion, then execute",
                ),
                option("revise", "Keep planning", "Your text is the feedback"),
            ],
            initial: None,
            multi: false,
            free_text: true,
        }],
        child: None,
        tool: Some("plan_submit".into()),
        opened_at: at,
        expires_at: None,
    }
}

/// The sample build waits on the review of its second plan.
pub(super) fn seed(host: &mut Host, model: Option<&str>, at: String) {
    let Some(parts) = host.parts.as_deref_mut() else {
        return;
    };
    parts.plan_doc = DOC.into();
    parts.plan_mode = Some(PlanModeBrief {
        phase: PlanModePhase::Review,
        title: Some(TITLE.into()),
        revision: Some(REVISION),
    });
    host.pending.push(card(DOC, model, at));
}

/// The plan document of `host`, in the host's pages: each with the token of the next.
pub(super) fn serve(feed: &mut Feed, target: &DetailRef, id: &str) -> Result<(), String> {
    let (doc, brief) = feed
        .hosts
        .get(id)
        .and_then(|host| host.parts.as_deref())
        .filter(|parts| !parts.plan_doc.is_empty())
        .map(|parts| (parts.plan_doc.clone(), parts.plan_mode.clone()))
        .ok_or("not_found: this session has no plan document")?;
    let brief = brief.as_ref();
    let page = |offset: usize, body: &str| PlanModeDocPage {
        title: brief
            .and_then(|brief| brief.title.clone())
            .unwrap_or_else(|| "Plan".into()),
        revision: brief.and_then(|brief| brief.revision).unwrap_or(0),
        body: body.to_owned(),
        bytes: doc.len() as u64,
        offset: offset as u64,
    };
    let mut pages = Vec::new();
    let mut offset = 0;
    while offset < doc.len() {
        let body = head(&doc[offset..], PAGE);
        let next = offset + body.len();
        pages.push(DetailReply {
            etag: format!("preview-{}", feed.changes_head),
            as_of: stamp(feed.now),
            seq: feed.changes_head,
            next: (next < doc.len()).then(|| next.to_string()),
            body: Some(DetailBody::SessionPlanModeDoc(page(offset, body))),
        });
        offset = next;
    }
    let merged = DetailBody::SessionPlanModeDoc(page(0, &doc));
    feed.details.insert(
        detail_key(target),
        DetailCache {
            transcript_seams: Default::default(),
            target: target.clone(),
            pages,
            loading: false,
            stale: false,
            error: None,
            merged: Some(merged),
        },
    );
    Ok(())
}

/// The owner's answer to the review: an execute option starts and leaves plan mode; Keep
/// planning, or words with no option, delivers the feedback and plans on; a dismissal keeps the
/// card.
pub(super) fn answer(
    feed: &mut Feed,
    id: &Id,
    index: usize,
    answer: &SessionCardAnswer,
) -> Result<Value, String> {
    if answer.dismiss {
        return answered("dismissed");
    }
    let at = stamp(feed.now);
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .ok_or_else(|| format!("{id} has no Pi host"))?;
    let given = answer.answers.first();
    let said = given
        .and_then(|given| given.text.as_deref())
        .is_some_and(|text| !text.trim().is_empty());
    let chosen = match given.and_then(|given| given.options.first()) {
        Some(option) => option.as_str(),
        None if said => "revise",
        None => return Err("choose how the plan proceeds".into()),
    };
    let status = match chosen {
        "execute" | "execute_clean" | "execute_compact" => "started",
        "revise" => "delivered",
        other => return Err(format!("{other} is not an option of this review")),
    };
    host.pending.remove(index);
    let parts = parts(host)?;
    parts.plan_mode = match (status, parts.plan_mode.take()) {
        ("delivered", Some(brief)) => Some(PlanModeBrief {
            phase: PlanModePhase::Planning,
            ..brief
        }),
        _ => None,
    };
    parts.event(&at, "plan_review", json!({"choice": chosen}).to_string());
    settle(feed, id.as_str());
    answered(status)
}

/// The Plan mode action: on plans from now; off leaves plan mode and withdraws a waiting review.
pub(super) fn toggle(feed: &mut Feed, id: &Id, enabled: bool) -> Result<Value, String> {
    let host = feed
        .hosts
        .get_mut(id.as_str())
        .ok_or_else(|| format!("{id} has no Pi host"))?;
    if host.busy() {
        return Err("Wait for the current turn to finish".into());
    }
    let parts = parts(host)?;
    let text = if enabled {
        parts.plan_mode.get_or_insert(PlanModeBrief {
            phase: PlanModePhase::Planning,
            title: None,
            revision: None,
        });
        "Plan mode is on: the working tree is read-only until a plan is approved"
    } else {
        parts.plan_mode = None;
        host.pending
            .retain(|card| card.kind != CardKind::PlanReview);
        "Plan mode is off"
    };
    settle(feed, id.as_str());
    serde_json::to_value(SessionActionDone {
        text: Some(text.into()),
    })
    .map_err(|error| error.to_string())
}
