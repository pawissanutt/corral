use std::cell::RefCell;
use std::rc::Rc;

use nsh_core::element::{Runtime, create_root};

use super::anchors::Mark;
use super::jump::{Place, Shown, kept};
use crate::model::Id;
use crate::model::session::transcript::Reading;

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
    place.shown(4, Some((scope.clone(), format!("{record}|2"))));
    assert_eq!(
        place.shown(7, Some((scope.clone(), format!("{record}|5")))),
        Shown::Up {
            held: Some((scope.clone(), record.to_owned(), 300.0)),
            arrived: 0,
        },
        "the record's earlier items are held above the reader, not counted"
    );
    assert_eq!(
        place.shown(8, Some((scope, format!("{record}|5")))),
        Shown::Up {
            held: None,
            arrived: 1,
        },
        "a reply below still counts"
    );
    // The message was 500 high with the reader 300 into it; its earlier items add 200 above.
    let (top, height) = (40.0, 500.0);
    let mark = Mark::Bottom(height - 300.0);
    assert_eq!(
        mark.offset(top, height + 200.0),
        top + 500.0,
        "the reader stays on the item they read"
    );
}
