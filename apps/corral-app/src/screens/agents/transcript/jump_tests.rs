use std::cell::RefCell;
use std::rc::Rc;

use nsh_core::element::{Runtime, create_root};

use super::jump::{Place, kept};
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
