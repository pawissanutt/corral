#[path = "follow_fixture.rs"]
mod follow_fixture;

use std::cell::RefCell;
use std::rc::Rc;

use corral_protocol::detail::{
    DetailBody, DetailRef, DetailReply, HostPresence, SessionItem, SessionMessage, SessionPage,
    SessionToolState,
};
use nsh_core::element::{Runtime, create_root};

use super::anchors::Mark;
use super::jump::{Place, Shown, kept};
use crate::model::Id;
use crate::model::daemon::{DetailCache, Feed};
use crate::model::session::Ui;
use crate::model::session::transcript::{MessageItem, Reading, Transcript, transcript};

/// Where the reader was used to live in `stored` values and a signal of the transcript view's
/// owner, read by that owner's cleanup. NSH frees what an owner made before it runs the owner's
/// cleanups, so at teardown the cleanup read "at the end" and no scope, and leaving a session
/// never saved the row the reader had scrolled up to. The cleanup must save it.
#[test]
fn leaving_a_transcript_saves_where_the_reader_was() {
    let runtime = Runtime::default();
    let _entered = runtime.enter();
    let saved: Rc<RefCell<Vec<(Id, Reading)>>> = Rc::new(RefCell::new(Vec::new()));
    let place = Rc::new(RefCell::new(Place {
        at_end: true,
        ..Place::default()
    }));

    let (view, ()) = create_root(|| {
        let saved = saved.clone();
        kept(place.clone(), move |scope, reading| {
            saved.borrow_mut().push((scope, reading));
        });
    });
    // The view showed B-test, the reader scrolled up to a row and new rows arrived.
    {
        let mut known = place.borrow_mut();
        known.scope = Some(Id::from("B-test"));
        known.at_end = false;
        known.left_at = Some(7);
        known.top = Some(("B-test/entry/e3".to_owned(), 12.0));
        known.rows = 9;
    }
    view.dispose();
    assert_eq!(
        *saved.borrow(),
        [(
            Id::from("B-test"),
            Reading::At {
                key: "B-test/entry/e3".to_owned(),
                offset: 12,
                rows: 7,
            },
        )],
        "the view going saves the row the reader was at"
    );
}

/// A record split across pages gains its earlier items at the front of the message the reader
/// is in. Rows above were told only by a new first message, so these were counted as new
/// replies and the reader's place, kept from the message's top, slid down by what arrived.
#[test]
fn a_split_record_growing_above_the_reader_holds_their_row_and_is_not_new() {
    let scope = Id::from("B-test");
    let record = "B-test/entry/e3";
    let mut place = Place {
        scope: Some(scope.clone()),
        at_end: false,
        top: Some((record.to_owned(), 300.0)),
        ..Place::default()
    };
    let tail = || Some((record.to_owned(), 0));
    place.shown(
        4,
        Some((scope.clone(), format!("{record}|2"))),
        tail(),
        |_| None,
    );
    assert_eq!(
        place.shown(
            8,
            Some((scope.clone(), format!("{record}|5"))),
            Some(("reply".into(), 0)),
            |_| Some(1)
        ),
        Shown::Up {
            held: Some((scope.clone(), record.to_owned(), 300.0)),
            arrived: 1,
        },
        "three earlier rows are held above the reader; the simultaneous reply counts"
    );
    assert_eq!(
        place.shown(9, Some((scope, format!("{record}|5"))), tail(), |_| None),
        Shown::Up {
            held: None,
            arrived: 2,
        },
        "another reply below still counts"
    );
    // The message was 500 high with the reader 300 into it, in its last item; its earlier items
    // add 200 above. Items are keyed from the record's end, so the ones shown keep their keys.
    let key = |from_end: usize| format!("{record}|{from_end}");
    let mark = Mark::Held {
        into: 300.0,
        height: 500.0,
        items: vec![(key(2), 0.0, 200.0), (key(1), 200.0, 280.0)],
    };
    let now = [
        (key(4), 0.0, 100.0),
        (key(3), 100.0, 100.0),
        (key(2), 200.0, 200.0),
        (key(1), 400.0, 280.0),
    ];
    let top = 40.0;
    assert_eq!(
        mark.offset(top, 700.0, |wanted| placed(&now, wanted)),
        top + 500.0,
        "the reader stays on the item they read"
    );
}

/// Where item `wanted` is among `items` (key, top, height).
fn placed(items: &[(String, f32, f32)], wanted: &str) -> Option<(f32, f32)> {
    items
        .iter()
        .find(|(key, ..)| key == wanted)
        .map(|(_, top, height)| (*top, *height))
}

#[test]
fn a_combined_publish_counts_new_rows_inside_the_same_assistant_tail() {
    let scope = Id::from("B-test");
    let text = |text: &str| SessionItem::Text { text: text.into() };
    let read = |id: &str| SessionItem::ToolCall {
        call_id: id.into(),
        name: "read".into(),
        arguments: r#"{"path":"src/lib.rs"}"#.into(),
        summary: String::new(),
        state: SessionToolState::Done,
        paired: true,
        call: None,
        output: Some("fn main() {}".into()),
        result: None,
        elapsed_ms: None,
        diff: None,
        content: None,
    };
    let record = |id: &str, author: &str, items| SessionMessage {
        id: format!("{scope}/entry/{id}"),
        author: author.into(),
        at: None,
        items,
        model: None,
        stop_reason: None,
        usage: None,
        input: None,
        attachments: Vec::new(),
    };
    let publish = |older: Vec<SessionMessage>, latest: Vec<SessionMessage>| {
        let page = |messages, older| SessionPage {
            id: scope.to_string(),
            role: "worker".into(),
            task: None,
            run: None,
            machine: None,
            messages,
            older,
            replyable: true,
            reply_refusal: None,
            host: HostPresence::NotRunning,
        };
        let reply = |body| DetailReply {
            etag: "snapshot".into(),
            as_of: String::new(),
            seq: 0,
            next: None,
            body: Some(DetailBody::Session(body)),
        };
        let has_older = !older.is_empty();
        let mut pages = vec![reply(page(latest.clone(), Some("previous".into())))];
        let mut all = older;
        if has_older {
            pages.push(reply(page(all.clone(), None)));
        }
        all.extend(latest);
        let cache = DetailCache {
            transcript_seams: Default::default(),
            target: DetailRef::Session {
                id: scope.to_string(),
            },
            pages,
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::Session(page(
                all,
                (!has_older).then(|| "previous".into()),
            ))),
        };
        transcript(&scope, &Ui::default(), Some(&cache), &Feed::default())
    };
    let reader = record("reader", "owner", vec![text("Reader's row")]);
    let mut latest = vec![
        reader.clone(),
        record("tail-head", "assistant", vec![read("c1")]),
        record("tail-end", "assistant", vec![read("c2")]),
    ];
    let before = publish(Vec::new(), latest.clone());
    assert_eq!(before.rows(), 2);
    assert!(matches!(
        before.messages.last().unwrap().items.as_slice(),
        [MessageItem::Group(_)]
    ));

    // The reader is 40 into the merged tail: in its folded calls, under the bubble's head.
    let held = before.messages.last().unwrap().clone();
    assert_eq!(held.keys.len(), held.items.len());
    let mut place = Place {
        scope: Some(scope.clone()),
        top: Some((held.anchor.clone(), 40.0)),
        ..Place::default()
    };
    let show = |place: &mut Place, transcript: &Transcript| {
        let following = transcript.following();
        place.shown(
            following.rows,
            transcript.first.clone().map(|key| (scope.clone(), key)),
            following.tail,
            |key| transcript.rows_below(key),
        )
    };
    show(&mut place, &before);

    latest.push(record(
        "fresh",
        "assistant",
        vec![read("c3"), text("New reply"), text("Second paragraph")],
    ));
    let after = publish(
        vec![record("older", "owner", vec![text("Earlier history")])],
        latest,
    );
    let tail = after.messages.last().unwrap();
    assert_eq!(tail.anchor, before.messages.last().unwrap().anchor);
    assert!(matches!(
        tail.items.as_slice(),
        [
            MessageItem::Group(_),
            MessageItem::Prose(_),
            MessageItem::Prose(_)
        ]
    ));
    assert_eq!(
        show(&mut place, &after),
        Shown::Up {
            held: Some((scope, held.anchor.clone(), 40.0)),
            arrived: 2,
        },
        "the older row is held above, the existing group remains one row, and both fresh rows count"
    );
    assert_eq!(tail.keys.len(), tail.items.len());
    assert_eq!(
        tail.keys[0], held.keys[0],
        "the folded calls keep their key when a later call joins them"
    );

    // Laid out: the bubble's head takes 20 above its items, the group 30, each paragraph 40.
    // It was at 100, 86 high; the older row adds 50 above, the paragraphs 80 inside it below.
    let mark = Mark::Held {
        into: 40.0,
        height: 86.0,
        items: vec![(held.keys[0].clone(), 0.0, 30.0)],
    };
    let now = [
        (tail.keys[0].clone(), 0.0, 30.0),
        (tail.keys[1].clone(), 30.0, 40.0),
        (tail.keys[2].clone(), 70.0, 40.0),
    ];
    assert_eq!(
        mark.offset(150.0, 166.0, |wanted| placed(&now, wanted)),
        150.0 + 40.0,
        "the reader moves with the rows above, not with the paragraphs below them"
    );
}
