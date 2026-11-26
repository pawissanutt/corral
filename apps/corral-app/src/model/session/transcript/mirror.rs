//! The session stream mutates the same document that history reads publish.
use std::collections::BTreeMap;

use corral_protocol::detail::{DetailBody, DetailRef, SessionItem, SessionMessage, SessionPage};
use corral_protocol::subscription::{TranscriptOp, TranscriptPart};

use crate::model::daemon::DetailCache;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub(crate) struct Mirror {
    pub active: bool,
    pub open: Option<String>,
    pub identity: BTreeMap<String, String>,
    before: BTreeMap<String, Option<String>>,
    sizes: BTreeMap<String, usize>,
    reset: Option<Option<String>>,
}

pub(crate) const MAX_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn apply(cache: &mut DetailCache, ops: Vec<TranscriptOp>, more: bool) {
    if cache.transcript_seams.mirror.reset.is_none()
        && ops.iter().all(|op| matches!(op, TranscriptOp::Other))
    {
        return;
    }
    let DetailBody::Session(page) = cache.merged.get_or_insert_with(|| {
        DetailBody::Session(SessionPage {
            id: match &cache.target {
                DetailRef::Session { id } => id.clone(),
                _ => String::new(),
            },
            role: String::new(),
            task: None,
            run: None,
            machine: None,
            messages: Vec::new(),
            older: None,
            replyable: false,
            reply_refusal: None,
            host: Default::default(),
        })
    }) else {
        return;
    };
    let mirror = &mut cache.transcript_seams.mirror;
    mirror.active = true;
    let mut spliced = false;
    let mut restarted = false;
    for op in ops {
        match op {
            TranscriptOp::Reset { older } => mirror.reset = Some(older),
            TranscriptOp::Put {
                mut message,
                before,
                replaces,
                open,
                cut,
            } => {
                if let Some(older) = mirror.reset.take() {
                    splice(page, mirror, Some(&message.id), older);
                    spliced = true;
                }
                let id = message.id.clone();
                let at = page.messages.iter().position(|m| m.id == id).or_else(|| {
                    let replaces = replaces.as_deref()?;
                    page.messages.iter().position(|m| m.id == replaces)
                });
                if let Some(at) = at {
                    keep_words(&page.messages[at], &mut message);
                }
                mirror.sizes.insert(id.clone(), size(&message));
                if let Some(at) = at {
                    let old = &page.messages[at].id;
                    if old != &id {
                        let key = mirror.identity.remove(old).unwrap_or_else(|| old.clone());
                        mirror.forget(old);
                        mirror.identity.insert(id.clone(), key);
                    }
                    page.messages[at] = message;
                } else {
                    page.messages.push(message);
                }
                if open {
                    mirror.open = Some(id.clone());
                    mirror
                        .identity
                        .entry(id.clone())
                        .or_insert_with(|| id.clone());
                } else if mirror.open.as_ref() == Some(&id) {
                    mirror.open = None;
                }
                if cut {
                    // Only the record's last items: its earlier ones are a page back, so
                    // nothing held before it joins it. The document starts here again.
                    let at = page.messages.iter().position(|m| m.id == id).unwrap_or(0);
                    for message in page.messages.drain(..at) {
                        mirror.forget(&message.id);
                    }
                    page.older.clone_from(&before);
                    restarted = true;
                }
                mirror.before.insert(id, before);
            }
            TranscriptOp::Append {
                id,
                part,
                offset,
                text,
            } if mirror.open.as_ref() == Some(&id) => {
                if let Some(message) = page.messages.iter_mut().find(|m| m.id == id) {
                    append(&mut message.items, part, offset, text);
                    mirror.sizes.insert(id, size(message));
                }
            }
            TranscriptOp::Drop { id } => {
                page.messages.retain(|m| m.id != id);
                mirror.forget(&id);
            }
            _ => {}
        }
    }
    if !more && let Some(older) = mirror.reset.take() {
        splice(page, mirror, None, older);
        spliced = true;
    }
    if restarted {
        cache.pages.clear();
    }
    if spliced || restarted {
        cache.transcript_seams.empty_cursors.clear();
        cache.stale = false;
        cache.error = None;
    }
    bound(cache);
}

/// A record carries a long item as a preview and where the rest is read. Words this client
/// was sent whole while they were written are those same words, so they stay: a reply does
/// not shrink to its preview and grow back when its record arrives, or when a tool result
/// is folded into it later.
fn keep_words(held: &SessionMessage, message: &mut SessionMessage) {
    for whole in &held.items {
        let (SessionItem::Text { text: words } | SessionItem::Thinking { text: words }) = whole
        else {
            continue;
        };
        let Ok(size) = serde_json::to_vec(whole).map(|bytes| bytes.len() as u64) else {
            continue;
        };
        let preview = message.items.windows(2).position(|pair| {
            let same = match (&pair[0], whole) {
                (SessionItem::Text { text }, SessionItem::Text { .. })
                | (SessionItem::Thinking { text }, SessionItem::Thinking { .. }) => {
                    words.starts_with(text.as_str())
                }
                _ => false,
            };
            // The continuation says how large the whole item is.
            same && matches!(&pair[1], SessionItem::Continuation { bytes, .. } if *bytes == size)
        });
        if let Some(at) = preview {
            message.items[at] = whole.clone();
            message.items.remove(at + 1);
        }
    }
}

fn splice(page: &mut SessionPage, mirror: &mut Mirror, first: Option<&str>, older: Option<String>) {
    let keep = first
        .and_then(|id| page.messages.iter().position(|m| m.id == id))
        .unwrap_or(0);
    for message in page.messages.drain(keep..) {
        if Some(message.id.as_str()) != first {
            mirror.forget(&message.id);
        }
    }
    mirror.open = None;
    if keep == 0 {
        page.older = older;
    }
}

impl Mirror {
    pub(crate) fn bytes(&self) -> usize {
        self.sizes.values().sum()
    }

    pub(crate) fn message_bytes(&self, id: &str) -> Option<usize> {
        self.sizes.get(id).copied()
    }

    pub(crate) fn remember_edge(&mut self, page: &SessionPage) {
        if let Some(first) = page.messages.first() {
            self.before.insert(first.id.clone(), page.older.clone());
        }
    }

    pub(crate) fn joined_fragment(&mut self, id: &str) {
        self.sizes.remove(id);
    }

    fn forget(&mut self, id: &str) {
        if self.open.as_deref() == Some(id) {
            self.open = None;
        }
        self.identity.remove(id);
        self.before.remove(id);
        self.sizes.remove(id);
    }
}

fn append(items: &mut Vec<SessionItem>, part: TranscriptPart, offset: u64, text: String) {
    let thinking = match part {
        TranscriptPart::Text => false,
        TranscriptPart::Thinking => true,
    };
    let existing = items.iter_mut().find_map(|item| match item {
        SessionItem::Thinking { text } if thinking => Some(text),
        SessionItem::Text { text } if !thinking => Some(text),
        _ => None,
    });
    if let Some(existing) = existing {
        let Ok(start) = usize::try_from(offset) else {
            return;
        };
        let end = start.saturating_add(text.len()).min(existing.len());
        if existing.is_char_boundary(start) && existing.is_char_boundary(end) {
            existing.replace_range(start..end, &text);
        }
    } else if offset == 0 {
        if thinking {
            items.insert(0, SessionItem::Thinking { text });
        } else {
            items.push(SessionItem::Text { text });
        }
    }
}

fn size(message: &SessionMessage) -> usize {
    serde_json::to_vec(message).map_or(0, |bytes| bytes.len() + 1)
}

pub(crate) fn bound(cache: &mut DetailCache) {
    let Some(DetailBody::Session(page)) = &mut cache.merged else {
        return;
    };
    let mirror = &mut cache.transcript_seams.mirror;
    let mut bytes = 0;
    for message in &page.messages {
        bytes += *mirror
            .sizes
            .entry(message.id.clone())
            .or_insert_with(|| size(message));
    }
    let mut cut = 0;
    if bytes > MAX_BYTES {
        for (index, message) in page.messages.iter().enumerate().skip(1) {
            bytes -= mirror.sizes[&page.messages[index - 1].id];
            if mirror.before.get(&message.id).is_some_and(Option::is_some) {
                cut = index;
                if bytes <= MAX_BYTES {
                    break;
                }
            }
        }
    }
    if cut != 0 {
        for message in page.messages.drain(..cut) {
            mirror.forget(&message.id);
        }
        page.older = page
            .messages
            .first()
            .and_then(|m| mirror.before.get(&m.id))
            .cloned()
            .flatten();
        cache.pages.clear();
    }
}
