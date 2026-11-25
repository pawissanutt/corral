//! Order and group the displayed rows after replay resolves recorded items and tool pairs.

use corral_protocol::detail::SessionMessage;

use crate::model::daemon::DetailCache;

use super::super::{
    Message, MessageItem, MessageKind, bubbles, group, message_rows, pairs, seams, tool,
};
use super::{Cx, avatar, clock, item_key, message, millis, quiet_message, row_key};

/// The end of one source message within its bubble, before tool groups are folded.
struct MessageEnd {
    anchor: String,
    bubble: usize,
    items: usize,
    footer: bool,
}

impl MessageEnd {
    fn at(anchor: String, bubble: usize, message: &Message) -> Self {
        Self {
            anchor,
            bubble,
            items: message.items.len(),
            footer: message.footer.is_some(),
        }
    }
}

/// Consecutive messages of one agent read as one reply and a run of calls can fold; the
/// merged message keeps the first's head and the latest foot. Consecutive quiet rows stand
/// together under one blank row. Keep each source message's end so a later publish can find
/// the old tail, even when the last bubble absorbed a new message.
fn merged(
    messages: impl Iterator<Item = (bool, Message)>,
    cx: Cx<'_>,
) -> (Vec<Message>, Vec<(String, usize)>) {
    let count = messages.size_hint().0;
    let mut bubbles = Vec::<Message>::with_capacity(count);
    let mut ends = Vec::with_capacity(count);
    for (boundary, message) in messages {
        let next = bubbles.len();
        if !boundary
            && let Some(last) = bubbles.last_mut()
            && last.kind == message.kind
            && match message.kind {
                MessageKind::Agent => last.author == message.author,
                MessageKind::Quiet => true,
                MessageKind::Bubble => false,
            }
        {
            if keyed(last) && keyed(&message) {
                last.keys.extend(message.keys);
            } else {
                last.keys.clear();
            }
            last.items.extend(message.items);
            if message.footer.is_some() {
                last.footer = message.footer;
            }
            ends.push(MessageEnd::at(message.anchor, next - 1, last));
        } else {
            ends.push(MessageEnd::at(message.anchor.clone(), next, &message));
            bubbles.push(message);
        }
    }
    let mut rows = 0;
    let mut message_ends = Vec::with_capacity(ends.len());
    let mut ends = ends.into_iter().peekable();
    for (bubble, message) in bubbles.iter_mut().enumerate() {
        let whole = keyed(message);
        message.items = group::fold(std::mem::take(&mut message.items), cx);
        // A folded run keeps the key of its first call.
        let mut keys = std::mem::take(&mut message.keys).into_iter();
        if whole {
            for item in &message.items {
                message.keys.extend(keys.next());
                if let MessageItem::Group(group) = item {
                    keys.by_ref().take(group.count - 1).for_each(drop);
                }
            }
        }
        let mut items = message.items.iter();
        let mut consumed = 0;
        let mut drawn = 0;
        while let Some(end) = ends.next_if(|end| end.bubble == bubble) {
            while consumed < end.items
                && let Some(item) = items.next()
            {
                // A group crossing an end already occupies a row on its earlier side.
                consumed += match item {
                    MessageItem::Group(group) => group.count,
                    _ => 1,
                };
                drawn += 1;
            }
            message_ends.push((end.anchor, rows + drawn + usize::from(end.footer)));
        }
        rows += message_rows(message);
    }
    (bubbles, message_ends)
}

/// Every item of `message` has its key.
fn keyed(message: &Message) -> bool {
    message.keys.len() == message.items.len()
}

fn projected_result(anchor: &str, pairs: &[&pairs::Pair<'_>], cx: Cx<'_>) -> Message {
    let mut items = Vec::new();
    let mut keys = Vec::new();
    for (n, pair) in pairs.iter().enumerate() {
        let key = item_key(anchor, pairs.len() - n);
        items.extend(tool::call(&pair.item, &key, pair.rest, cx));
        keys.extend((0..items.len() - keys.len()).map(|k| row_key(&key, k)));
    }
    quiet_message(anchor, "toolResult", anchor.into(), None, items, keys)
}

pub(super) type RecordedView = (Vec<Message>, Vec<(String, usize)>, Option<String>);

/// Recorded surface only; anchor reconciliation need not render the live turn and echoes.
pub(super) fn recorded_view(
    recorded: &[SessionMessage],
    cache: Option<&DetailCache>,
    cx: Cx<'_>,
    mut keep_cut: impl FnMut(&str, &str) -> bool,
) -> RecordedView {
    let pairs = pairs::collect(recorded, cache, cx.feed);
    let first_user = recorded.iter().position(|m| avatar(&m.author).0 == "Y");
    // Physical starts protect a prepend; retained displayed starts protect a moving seam.
    let boundaries = seams::boundaries(cache);
    let projection = seams::Projection::of(recorded, cache, &pairs);
    let mut cuts = Vec::new();
    // A split record keeps its anchor but gains items at its front. Track the first recorded
    // row before adding child steers or mail, which can stay ahead of it across pages.
    let mut first = None;
    let mut timed = Vec::new();
    let mut push = |id: &str, at: Option<u64>, shown: Message, slots: usize| {
        cuts.extend(boundaries.get(id).copied());
        if shown.items.is_empty() && shown.footer.is_none() {
            return;
        }
        // Pairing can remove a result's row, so consider a cut where it actually lands.
        let mut boundary = false;
        for cut in cuts.drain(..) {
            boundary |= keep_cut(cut, &shown.anchor);
        }
        first.get_or_insert_with(|| item_key(&shown.anchor, slots));
        timed.push((at, boundary, shown));
    };
    for (n, m) in recorded.iter().enumerate() {
        let at = millis(m.at.as_deref());
        let placed = projection.placed(m.id.as_str());
        let shown = message(
            m,
            cx,
            &pairs,
            Some(n) == first_user,
            placed,
            &projection.moved,
        );
        push(&m.id, at, shown, m.items.len() + placed.len());
        for &(_, anchor) in projection.after(m.id.as_str()) {
            // Its old result record no longer exists. Draw the replacement's result rows
            // at the retained anchor without constructing another protocol message.
            let pairs = projection.placed(anchor);
            push(anchor, at, projected_result(anchor, pairs, cx), pairs.len());
        }
    }
    // A child conversation's steers and agent messages stand before the first message that
    // started after them; those without a time go last.
    let at_clock = |ms: u64| {
        let at = jiff::Timestamp::from_millisecond(i64::try_from(ms).ok()?).ok()?;
        clock(Some(&at.to_string()), cx.feed)
    };
    let child = if cx.detached {
        Vec::new()
    } else {
        bubbles::child_bubbles(cx.id, cx.feed, at_clock)
    };
    for (at, bubble) in child {
        let place = at.and_then(|at| {
            timed
                .iter()
                .position(|(when, _, _)| when.is_some_and(|when| when > at))
        });
        timed.insert(place.unwrap_or(timed.len()), (at, false, bubble));
    }
    let (messages, message_ends) = merged(
        timed.into_iter().map(|(_, seam, message)| (seam, message)),
        cx,
    );
    (messages, message_ends, first)
}
