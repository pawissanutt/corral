//! Settle tools by call id. Normally the call owns the row, but a standalone result on a
//! newer page keeps its position (and thus its scroll anchor) when the call is prepended.

use std::borrow::Cow;
use std::collections::BTreeMap;

use corral_protocol::detail::{DetailBody, SessionItem, SessionMessage, SessionToolState};

use crate::model::daemon::{DetailCache, Feed};

use super::replay::{Rest, resolve_at};

pub(super) struct Pair<'a> {
    pub position: &'a SessionItem,
    pub item: Cow<'a, SessionItem>,
    pub rest: Rest<'a>,
    pub anchor: &'a str,
    pub result_position: bool,
    page: usize,
}

pub(super) type Pairs<'a> = BTreeMap<&'a str, Pair<'a>>;

/// Cache pages are newest first. A joined record can straddle a seam, but joining records
/// preserves the order and number of raw items, including continuations.
fn page_starts(cache: Option<&DetailCache>) -> Vec<usize> {
    let lengths: Vec<_> = cache
        .into_iter()
        .flat_map(DetailCache::bodies)
        .filter_map(|body| match body {
            DetailBody::Session(page) => {
                Some(page.messages.iter().map(|row| row.items.len()).sum())
            }
            _ => None,
        })
        .collect();
    lengths
        .into_iter()
        .rev()
        .scan(0, |start, len: usize| {
            let next = *start + len;
            Some(std::mem::replace(start, next))
        })
        .collect()
}

pub(super) fn collect<'a>(
    messages: &'a [SessionMessage],
    cache: Option<&'a DetailCache>,
    feed: &'a Feed,
) -> Pairs<'a> {
    let starts = page_starts(cache);
    let mut base = 0;
    let mut pairs = Pairs::new();
    for message in messages {
        for (at, item, rest) in resolve_at(&message.items, feed) {
            let SessionItem::ToolCall {
                call_id,
                paired,
                call,
                result,
                ..
            } = item
            else {
                continue;
            };
            if call_id.is_empty() {
                continue;
            }
            let page = starts.partition_point(|&start| start <= base + at);
            let pin = cache.and_then(|cache| cache.transcript_seams.tools.get(call_id));
            let at_pin = pin.is_some_and(|pin| pin.anchor == message.id);
            match pairs.entry(call_id.as_str()) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(Pair {
                        position: item,
                        item: Cow::Borrowed(item),
                        rest,
                        anchor: &message.id,
                        result_position: !paired && result.is_some(),
                        page,
                    });
                }
                std::collections::btree_map::Entry::Occupied(mut slot) => {
                    let pair = slot.get_mut();
                    if !pin.is_some_and(|pin| pin.anchor == pair.anchor)
                        && (at_pin
                            || (pair.page < page
                                && !paired
                                && result.is_some()
                                && matches!(
                                    pair.item.as_ref(),
                                    SessionItem::ToolCall { result: None, .. }
                                )))
                    {
                        pair.position = item;
                        pair.anchor = &message.id;
                        pair.result_position = !paired && result.is_some();
                        pair.page = page;
                    }
                    // Keep the complete REST copy only if this is the same raw result.
                    // One call can survive a branch reset with a different result.
                    if !paired
                        && call.is_none()
                        && let Some(result) = result.as_ref().filter(|raw| !raw.is_empty())
                        && !matches!(rest, Rest::Whole)
                        && matches!(pair.rest, Rest::Whole)
                        && matches!(
                            pair.item.as_ref(),
                            SessionItem::ToolCall {
                                paired: true,
                                result: Some(prior),
                                ..
                            } if prior == result
                        )
                    {
                        continue;
                    }
                    merge(pair.item.to_mut(), item);
                    if !matches!(rest, Rest::Whole) {
                        pair.rest = rest;
                    }
                }
            }
        }
        base += message.items.len();
    }
    pairs
}

fn merge(original: &mut SessionItem, incoming: &SessionItem) {
    let (
        SessionItem::ToolCall {
            name,
            arguments,
            summary,
            state,
            paired,
            call,
            output,
            result,
            elapsed_ms,
            diff,
            content,
            ..
        },
        SessionItem::ToolCall {
            name: new_name,
            arguments: new_arguments,
            summary: new_summary,
            state: new_state,
            paired: new_paired,
            call: new_call,
            output: new_output,
            result: new_result,
            elapsed_ms: new_elapsed,
            diff: new_diff,
            content: new_content,
            ..
        },
    ) = (original, incoming)
    else {
        return;
    };
    if name.is_empty() {
        name.clone_from(new_name);
    }
    if arguments.is_empty() {
        arguments.clone_from(new_arguments);
    }
    if summary.is_empty() {
        summary.clone_from(new_summary);
    }
    if *new_state != SessionToolState::Running {
        *state = *new_state;
    }
    *paired |= *new_paired || (*state != SessionToolState::Running && !arguments.is_empty());
    if call.is_none() {
        call.clone_from(new_call);
    }
    if new_output.is_some() {
        output.clone_from(new_output);
    }
    if new_result.is_some() {
        result.clone_from(new_result);
    }
    if new_elapsed.is_some() {
        *elapsed_ms = *new_elapsed;
    }
    if new_diff.is_some() {
        diff.clone_from(new_diff);
    }
    if new_content.is_some() {
        content.clone_from(new_content);
    }
}
