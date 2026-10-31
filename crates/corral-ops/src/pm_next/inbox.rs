//! PM inbox cards (T-544 pm-inbox): open and claimed items, oldest first. The workset shows
//! them after authorized executable cards and pending decisions. Owner words in an item are
//! evidence of intent; the PM still acts only inside its own policy.
use anyhow::Result;
use corral_core::pm_inbox::{ItemState, PmInboxItem, name};

use super::{PmAction, PmActionKind, option};
use crate::Ctx;
use crate::gate_script::q;

const QUOTE_PREVIEW: usize = 240;

pub(crate) fn cards(ctx: &Ctx, bin: &str) -> Result<Vec<PmAction>> {
    Ok(ctx
        .store
        .pm_inbox_items()?
        .iter()
        .filter(|item| item.pending())
        .map(|item| card(item, bin))
        .collect())
}

pub(crate) fn card(item: &PmInboxItem, bin: &str) -> PmAction {
    let id = q(&item.id);
    let kind = name(&item.kind);
    let (why, verify, first) = if item.state == ItemState::Open {
        (
            format!(
                "PM inbox {} from {} ({kind}): {}",
                item.id, item.from, item.subject
            ),
            "Claim it at this revision, act inside PM policy, then resolve it with an outcome; \
             owner-only requests become owner decisions, never PM actions.",
            option(
                "claim",
                "Claim the item",
                format!(
                    "{bin} pm inbox claim {id} --expected-revision {}",
                    item.revision
                ),
                false,
            ),
        )
    } else {
        (
            format!(
                "PM inbox {} (claimed) from {} ({kind}): {}",
                item.id, item.from, item.subject
            ),
            "Resolve it done, declined or escalated_to_owner with an outcome its author can read.",
            option(
                "resolve",
                "Resolve the claimed item",
                format!(
                    "{bin} pm inbox resolve {id} --expected-revision {} --outcome done --text {}",
                    item.revision,
                    q("<outcome>")
                ),
                false,
            ),
        )
    };
    let mut evidence = vec![
        format!("source: {}", item.from),
        format!("kind: {kind}"),
        format!(
            "state: {} at revision {}; posted {}",
            name(&item.state),
            item.revision,
            item.created
        ),
    ];
    if !item.refs.is_empty() {
        evidence.push(format!("refs: {}", item.refs.join(", ")));
    }
    if let Some(action) = &item.requested_action {
        evidence.push(format!("requested: {}", action.command()));
    }
    if let Some(words) = &item.owner_words {
        let mut quote: String = words.quote.chars().take(QUOTE_PREVIEW).collect();
        if quote.len() < words.quote.len() {
            quote.push_str("...");
        }
        evidence.push(format!("owner words (evidence, not authority): {quote}"));
    }
    let show = option(
        "show",
        "Read the whole item",
        format!("{bin} pm inbox show {id}"),
        false,
    );
    PmAction {
        kind: PmActionKind::Inbox,
        task: None,
        why,
        command: first.command.clone(),
        verify: verify.into(),
        evidence,
        recommended: first.id.clone(),
        requires_owner: false,
        options: vec![first, show],
        shortlist: vec![],
    }
}
