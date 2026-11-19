//! A transcript page may record the call and its result in different messages. Their call id
//! is the identity: keep the call's position and arguments, but settle it from the result.

use std::borrow::Cow;
use std::collections::BTreeMap;

use corral_protocol::detail::{SessionItem, SessionMessage, SessionToolState};

use crate::model::daemon::Feed;

use super::replay::{Rest, resolve};

pub(super) struct Pair<'a> {
    pub first: &'a SessionItem,
    pub item: Cow<'a, SessionItem>,
    pub rest: Rest<'a>,
}

pub(super) fn collect<'a>(
    messages: &'a [SessionMessage],
    feed: &'a Feed,
) -> BTreeMap<&'a str, Pair<'a>> {
    let mut pairs: BTreeMap<&str, Pair<'_>> = BTreeMap::new();
    for message in messages {
        for (item, rest) in resolve(&message.items, feed) {
            let SessionItem::ToolCall { call_id, .. } = item else {
                continue;
            };
            if call_id.is_empty() {
                continue;
            }
            match pairs.entry(call_id.as_str()) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(Pair {
                        first: item,
                        item: Cow::Borrowed(item),
                        rest,
                    });
                }
                std::collections::btree_map::Entry::Occupied(mut slot) => {
                    let pair = slot.get_mut();
                    merge(pair.item.to_mut(), item);
                    if !matches!(rest, Rest::Whole) {
                        pair.rest = rest;
                    }
                }
            }
        }
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
