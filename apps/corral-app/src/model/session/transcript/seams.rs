//! Local layout identity for a verified replacement page chain. Only the replacement's
//! recorded items are replayed; these hints say where to place the rows they make.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::detail::{DetailBody, SessionItem, SessionMessage};

use crate::model::daemon::{DetailCache, Feed};

use super::{
    page,
    pairs::{self, Pair},
    replay::recorded_layout,
};

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct TranscriptSeams {
    pub starts: BTreeSet<String>,
    /// Newer items of the last resident record that were paged out. Include these in
    /// end-relative item keys so evicting a fragment doesn't rename a visible item.
    pub suffix: Option<(String, usize)>,
    /// Empty branch scans skipped on the way to this window. Count the work, not
    /// empty payloads, when bounding a later re-seek.
    pub skipped: usize,
    /// Consecutive empty cursors consumed at the shared older edge (no payload).
    pub empty_cursors: BTreeSet<String>,
    // A moved physical cut inside a displayed block must not split the reader's item anchor
    // from its bubble. The oldest cut (which protects a future prepend) is never suppressed.
    joins: BTreeSet<String>,
    pub tools: BTreeMap<String, ToolPlacement>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToolPlacement {
    pub(super) anchor: String,
    // If a result is folded into its call, its own record can disappear. Keep it after
    // the preceding surviving record rather than moving the reader above that record.
    pub(super) after: Option<String>,
    order: usize,
}

fn physical_starts(cache: &DetailCache) -> impl Iterator<Item = &str> {
    cache
        .bodies()
        .filter_map(|body| match body {
            DetailBody::Session(page) => page.messages.first().map(|m| m.id.as_str()),
            _ => None,
        })
        .filter(|id| !id.is_empty())
}

pub(super) fn boundaries(cache: Option<&DetailCache>) -> BTreeSet<&str> {
    cache
        .into_iter()
        .flat_map(|cache| {
            let seams = &cache.transcript_seams;
            seams
                .starts
                .iter()
                .map(String::as_str)
                .chain(physical_starts(cache).filter(|id| !seams.joins.contains(*id)))
        })
        .collect()
}

#[derive(Default)]
pub(super) struct Projection<'a, 'p> {
    placed: BTreeMap<&'a str, Vec<&'p Pair<'a>>>,
    after: BTreeMap<&'a str, Vec<(usize, &'a str)>>,
    pub moved: BTreeSet<&'a str>,
}

impl<'a, 'p> Projection<'a, 'p> {
    pub fn of(
        messages: &'a [SessionMessage],
        cache: Option<&'a DetailCache>,
        pairs: &'p BTreeMap<&str, Pair<'a>>,
    ) -> Self {
        let mut projection = Self::default();
        let Some(cache) = cache.filter(|cache| !cache.transcript_seams.tools.is_empty()) else {
            return projection;
        };
        let ids: BTreeSet<_> = messages.iter().map(|m| m.id.as_str()).collect();
        for (call, pin) in &cache.transcript_seams.tools {
            let Some(pair) = pairs.get(call.as_str()) else {
                continue;
            };
            if pair.anchor == pin.anchor
                || !matches!(
                    pair.item.as_ref(),
                    SessionItem::ToolCall {
                        paired: true,
                        result: Some(_),
                        ..
                    }
                )
            {
                continue;
            }
            let anchor = pin.anchor.as_str();
            if !ids.contains(anchor) {
                let Some(neighbour) = pin.after.as_deref().filter(|id| ids.contains(id)) else {
                    continue;
                };
                projection
                    .after
                    .entry(neighbour)
                    .or_default()
                    .push((pin.order, anchor));
            } else if pin.after.is_none() {
                // Only a verified result position can be projected without a raw tool there.
                continue;
            }
            projection.placed.entry(anchor).or_default().push(pair);
            projection.moved.insert(call.as_str());
        }
        for rows in projection.after.values_mut() {
            rows.sort_unstable();
            rows.dedup();
        }
        projection
    }

    pub fn placed(&self, anchor: &str) -> &[&'p Pair<'a>] {
        self.placed.get(anchor).map_or(&[], Vec::as_slice)
    }

    pub fn after(&self, anchor: &str) -> &[(usize, &'a str)] {
        self.after.get(anchor).map_or(&[], Vec::as_slice)
    }
}

fn result_only(item: &SessionItem) -> bool {
    matches!(item, SessionItem::ToolCall { call_id, paired: false, call: None, result: Some(_), .. } if !call_id.is_empty())
}

/// Compare record contents apart from the two server layouts for a tool. The call's record
/// still has to match by ID, and all old tool payloads are checked separately by call ID.
fn ordinary(
    messages: &[SessionMessage],
) -> impl Iterator<Item = (&SessionMessage, Vec<&SessionItem>)> {
    messages.iter().filter_map(|message| {
        let mut items = message.items.iter().peekable();
        let mut kept = Vec::new();
        let mut has_result = false;
        while let Some(item) = items.next() {
            match item {
                SessionItem::ToolCall { call_id, .. } if !call_id.is_empty() => {
                    if result_only(item) {
                        has_result = true;
                    } else {
                        kept.push(item);
                    }
                    items.next_if(|item| matches!(item, SessionItem::Continuation { .. }));
                }
                _ => kept.push(item),
            }
        }
        (!has_result || !kept.is_empty()).then_some((message, kept))
    })
}

fn same_item(old: &&SessionItem, new: &&SessionItem) -> bool {
    match (old, new) {
        (SessionItem::ToolCall { call_id: a, .. }, SessionItem::ToolCall { call_id: b, .. })
            if !a.is_empty() =>
        {
            a == b
        }
        _ => old == new,
    }
}

fn covers_records(held: &[SessionMessage], replacement: &[SessionMessage]) -> bool {
    let mut old = ordinary(held).peekable();
    let Some((first, _)) = old.peek() else {
        return true;
    };
    let mut new = ordinary(replacement).skip_while(|(m, _)| m.id != first.id);
    let mut first = true;
    while let Some((a, ai)) = old.next() {
        let Some((b, bi)) = new.next() else {
            return false;
        };
        if a.id.is_empty()
            || a.id != b.id
            || a.author != b.author
            || a.at != b.at
            || ai.len() > bi.len()
        {
            return false;
        }
        let last = old.peek().is_none();
        let matches = |items: &[&SessionItem]| ai.iter().zip(items).all(|(a, b)| same_item(a, b));
        let covered = match (first, last) {
            // New pages can cover extra items at either end of a single split record.
            (true, true) => ai.is_empty() || bi.windows(ai.len()).any(matches),
            (true, false) => matches(&bi[bi.len() - ai.len()..]),
            (false, true) => matches(&bi[..ai.len()]),
            (false, false) => ai.len() == bi.len() && matches(&bi),
        };
        if !covered {
            return false;
        }
        first = false;
    }
    true
}

fn unambiguous(messages: &[SessionMessage]) -> bool {
    let mut tools = BTreeSet::new();
    messages
        .iter()
        .flat_map(|m| &m.items)
        .all(|item| match item {
            SessionItem::ToolCall {
                call_id,
                paired,
                result,
                ..
            } if !call_id.is_empty() => {
                tools.insert((call_id, result_only(item))) && (!*paired || result.is_some())
            }
            _ => true,
        })
}

// For a continued tool, regrouping can choose a different preview size. Compare the
// available prefixes (never a shortened payload on the side which is already complete).
fn covers_tool(old: &Pair<'_>, new: &Pair<'_>) -> bool {
    use super::replay::Rest;
    let (
        SessionItem::ToolCall {
            name: a,
            arguments: aa,
            call: ac,
            result: ar,
            output: ao,
            ..
        },
        SessionItem::ToolCall {
            name: b,
            arguments: ba,
            call: bc,
            result: br,
            output: bo,
            ..
        },
    ) = (old.item.as_ref(), new.item.as_ref())
    else {
        return false;
    };
    let text = |a: &str, b: &str| {
        a == b
            || (!matches!(old.rest, Rest::Whole) && b.starts_with(a))
            || (!matches!(new.rest, Rest::Whole) && a.starts_with(b))
    };
    let part = |a: &Option<String>, b: &Option<String>| {
        a.as_ref()
            .is_none_or(|a| b.as_ref().is_some_and(|b| text(a, b)))
    };
    text(a, b) && (aa.is_empty() || text(aa, ba)) && part(ac, bc) && part(ar, br) && part(ao, bo)
}

/// Keep the visible recorded anchors or retain the old snapshot. No carried seam is used to
/// pass the guard unless the replacement has the old records in order with compatible items.
/// A reader on its first page must get the same protection as a paged view.
pub(crate) fn preserve_anchors(
    feed: &Feed,
    held: &DetailCache,
    replacement: &mut DetailCache,
) -> bool {
    let (Some(old), Some(new)) = (page(Some(held)), page(Some(replacement))) else {
        return true;
    };
    if old.id != new.id {
        return true;
    }
    let held_layout = recorded_layout(held, feed, |_, _| true);
    if covers_records(&old.messages, &new.messages)
        && unambiguous(&old.messages)
        && unambiguous(&new.messages)
    {
        let old_pairs = pairs::collect(&old.messages, Some(held), feed);
        let new_pairs = pairs::collect(&new.messages, Some(replacement), feed);
        if old_pairs
            .iter()
            .all(|(id, old)| new_pairs.get(id).is_some_and(|new| covers_tool(old, new)))
        {
            let new_ids: BTreeSet<_> = new.messages.iter().map(|m| m.id.as_str()).collect();
            let mut previous = None;
            let before: BTreeMap<_, _> = old
                .messages
                .iter()
                .enumerate()
                .map(|(order, m)| {
                    let neighbour = previous;
                    if new_ids.contains(m.id.as_str()) {
                        previous = Some(m.id.as_str());
                    }
                    (m.id.as_str(), (order, neighbour))
                })
                .collect();
            let tools: BTreeMap<_, _> = old_pairs
                .iter()
                .filter_map(|(&id, old)| {
                    let new = &new_pairs[id];
                    let existing = held.transcript_seams.tools.get(id);
                    let anchor = existing.map_or(old.anchor, |pin| &pin.anchor);
                    if anchor.is_empty() || new.anchor == anchor {
                        return None;
                    }
                    let (order, previous) = existing
                        .and_then(|pin| {
                            pin.after.as_deref().map(|after| {
                                let previous = if new_ids.contains(after) {
                                    Some(after)
                                } else {
                                    before.get(after).and_then(|(_, previous)| *previous)
                                };
                                (pin.order, previous)
                            })
                        })
                        .unwrap_or_else(|| before.get(anchor).copied().unwrap_or((0, None)));
                    let after =
                        if old.result_position || existing.is_some_and(|pin| pin.after.is_some()) {
                            previous
                                .or(Some(new.anchor))
                                .filter(|id| !id.is_empty())
                                .map(str::to_owned)
                        } else {
                            None
                        };
                    Some((
                        id.to_owned(),
                        ToolPlacement {
                            anchor: anchor.into(),
                            after,
                            order,
                        },
                    ))
                })
                .collect();
            let current_cuts: BTreeSet<_> = physical_starts(replacement).collect();
            let tool_anchors: BTreeSet<_> =
                tools.values().map(|tool| tool.anchor.as_str()).collect();
            let starts = held_layout
                .starts
                .iter()
                .map(String::as_str)
                .filter(|id| {
                    !current_cuts.contains(id)
                        && (new_ids.contains(id) || tool_anchors.contains(id))
                })
                .map(str::to_owned)
                .collect();
            replacement.transcript_seams = TranscriptSeams {
                starts,
                joins: BTreeSet::new(),
                suffix: replacement.transcript_seams.suffix.take(),
                skipped: replacement.transcript_seams.skipped,
                empty_cursors: Default::default(),
                tools,
            };
            let interior: BTreeSet<_> = held_layout
                .ends
                .iter()
                .map(|(id, _)| id.as_str())
                .filter(|id| !held_layout.starts.contains(*id))
                .collect();
            let oldest = page(Some(replacement))
                .and_then(|page| page.messages.first())
                .map(|m| m.id.as_str());
            let mut joins = BTreeSet::new();
            let layout = recorded_layout(replacement, feed, |cut, displayed| {
                if Some(cut) != oldest
                    && !replacement.transcript_seams.starts.contains(cut)
                    && interior.contains(displayed)
                {
                    joins.insert(cut.to_owned());
                    false
                } else {
                    true
                }
            });
            replacement.transcript_seams.joins = joins;
            return held_layout.starts.is_subset(&layout.starts);
        }
    }
    // Bubble starts alone cannot prove that the items inside those bubbles survived.
    false
}
