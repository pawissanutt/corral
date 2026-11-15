//! Transport text parts are one message, not separate replies or owner decisions. Keep the
//! first part's identity and metadata so cards, waiting actions and outbox retirement stay
//! attached to the canonical message. Unsplit entries remain borrowed.

use std::borrow::Cow;
use std::collections::BTreeMap;

use corral_protocol::assistant::{EntryKind, ThreadEntry};

fn part(entry: &ThreadEntry) -> Option<(&str, usize)> {
    if let Some(part) = &entry.part {
        return Some((part.id.as_str(), part.index as usize));
    }
    let (base, index) = entry.id.rsplit_once('/')?;
    let index = index.parse().ok()?;
    match entry.kind {
        EntryKind::Answer | EntryKind::PmReply if base.ends_with("/answer") => Some((base, index)),
        EntryKind::Message | EntryKind::PmMessage if base.ends_with("/text") => {
            Some((base.strip_suffix("/text")?, index))
        }
        _ => None,
    }
}

pub(super) fn assemble(
    entries: &BTreeMap<crate::model::Id, ThreadEntry>,
) -> Vec<Cow<'_, ThreadEntry>> {
    let mut groups: BTreeMap<&str, Vec<(usize, &ThreadEntry)>> = BTreeMap::new();
    for entry in entries.values() {
        if let Some((base, index)) = part(entry) {
            groups.entry(base).or_default().push((index, entry));
        }
    }
    let mut result = Vec::with_capacity(entries.len());
    for entry in entries.values() {
        // Answers explicitly name part zero; owner messages use the bare message id.
        let base = match part(entry) {
            Some((base, 0)) => base,
            Some(_) => continue,
            None => entry.id.as_str(),
        };
        let Some(parts) = groups.get_mut(base) else {
            result.push(Cow::Borrowed(entry));
            continue;
        };
        parts.sort_unstable_by_key(|(index, _)| *index);
        let continuation = parts.iter().any(|(_, part)| part.id != entry.id);
        if !continuation {
            result.push(Cow::Borrowed(entry));
            continue;
        }
        let mut text = String::with_capacity(
            entry.text.as_ref().map_or(0, String::len)
                + parts
                    .iter()
                    .filter(|(_, part)| part.id != entry.id)
                    .filter_map(|(_, part)| part.text.as_ref())
                    .map(String::len)
                    .sum::<usize>(),
        );
        if part(entry).is_none() {
            text.push_str(entry.text.as_deref().unwrap_or_default());
        }
        for (_, part) in parts.iter() {
            if part.kind == entry.kind && part.rev == entry.rev {
                text.push_str(part.text.as_deref().unwrap_or_default());
            }
        }
        let bytes = text.len() as u64;
        let mut merged = ThreadEntry {
            id: entry.id.clone(),
            rev: entry.rev,
            at: entry.at.clone(),
            typed_at: entry.typed_at.clone(),
            kind: entry.kind,
            device: entry.device.clone(),
            via: entry.via.clone(),
            text: Some(text),
            state: entry.state.clone(),
            model: entry.model.clone(),
            cost_usd: entry.cost_usd,
            tools: entry.tools.clone(),
            part: entry.part.clone(),
            completion: entry.completion,
            cost_basis: entry.cost_basis,
            links: entry.links.clone(),
            card: entry.card.clone(),
            attachments: entry.attachments.clone(),
        };
        if let Some(metadata) = &mut merged.part {
            metadata.complete = metadata.complete
                && parts.len() == metadata.total as usize
                && parts.iter().enumerate().all(|(expected, (index, part))| {
                    expected == *index
                        && part.rev == entry.rev
                        && part.part.as_ref().is_some_and(|part| {
                            part.complete
                                && part.bytes == metadata.bytes
                                && part.total == metadata.total
                        })
                })
                && bytes == metadata.bytes;
        }
        result.push(Cow::Owned(merged));
    }
    result.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
    result
}

/// A cold host may have a durable conversation even before this device opens its transcript.
pub(super) fn has_history(
    feed: &crate::model::daemon::Feed,
    scope: &str,
    row: &corral_protocol::rows::AssistantRow,
) -> bool {
    row.turn.is_some_and(|turn| turn > 0)
        || row.cost_usd.is_some_and(|cost| cost > 0.0)
        || feed.threads.get(scope).is_some_and(|thread| {
            thread
                .entries
                .keys()
                .any(|id| super::entry_session(id.as_str()) == Some(row.id.as_str()))
        })
}

/// A stream is only a temporary override. Never replace a newer canonical suffix with an
/// older stream snapshot, and wait for every part when the server reports completeness.
pub(super) fn covers(entry: &ThreadEntry, stream: &str) -> bool {
    let Some(text) = entry.text.as_deref() else {
        return false;
    };
    match &entry.part {
        Some(part) => part.complete && text.len() as u64 == part.bytes,
        None => text.starts_with(stream),
    }
}

/// Older daemons use a workspace label as the title; prefer the first loaded owner message
/// without opening every session's history just to paint the wall.
pub(super) fn title<'a>(
    feed: &'a crate::model::daemon::Feed,
    row: &'a corral_protocol::rows::AssistantRow,
) -> Cow<'a, str> {
    let generic = row.title == "workspace"
        || row
            .title
            .strip_prefix(row.id.as_str())
            .is_some_and(|tail| tail.trim_start_matches([' ', '·']) == "workspace");
    if generic {
        let first = feed.threads.get(row.id.as_str()).and_then(|thread| {
            thread
                .entries
                .values()
                .filter(|entry| {
                    entry.kind == EntryKind::Message
                        && part(entry).is_none_or(|(_, index)| index == 0)
                })
                .filter_map(|entry| {
                    entry.text.as_deref().and_then(|text| {
                        text.lines()
                            .map(str::trim)
                            .find(|line| !line.is_empty())
                            .map(|line| (entry, line))
                    })
                })
                .min_by(|(a, _), (b, _)| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)))
                .map(|(_, line)| line)
        });
        if let Some(first) = first {
            return Cow::Borrowed(first);
        }
    }
    Cow::Borrowed(&row.title)
}
