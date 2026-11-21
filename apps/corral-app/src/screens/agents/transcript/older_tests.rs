use super::older::{Earlier, Reach};
use crate::model::Id;
use crate::model::session::Action;

fn offered(first: &str, cursor: &str, paging: bool) -> Earlier {
    Earlier {
        older: Some(Action::Older(Id::from("B-test"))),
        cursor: Some(cursor.to_owned()),
        paging,
        first: Some(first.to_owned()),
    }
}

/// Every scroll event near the top would ask for the page before again, and a page that
/// failed would be asked for in a loop while the reader stays there; a page that showed
/// nothing new would stall the reader at the top, and a page arriving above must not be read
/// past before the reader scrolls again.
#[test]
fn reading_near_the_top_asks_for_each_page_once() {
    let mut reach = Reach::default();
    let shown = offered("e9", "c1", false);
    assert!(
        reach.changed(Some(&shown), false).is_none(),
        "not scrolled yet"
    );
    assert!(
        reach.scrolled(Some(&shown), 300.0, 400.0, false).is_some(),
        "near the top asks for the page before"
    );
    assert!(
        reach.scrolled(Some(&shown), 200.0, 400.0, false).is_none(),
        "asked once"
    );
    assert!(
        reach.changed(Some(&shown), false).is_none(),
        "a failed page is not asked again on its own"
    );
    assert!(reach.scrolled(Some(&shown), 900.0, 400.0, false).is_none());
    assert!(
        reach.scrolled(Some(&shown), 100.0, 400.0, false).is_some(),
        "away from the top and back asks again"
    );
    assert!(
        reach
            .changed(Some(&offered("e9", "c2", false)), false)
            .is_some(),
        "a page that showed nothing above asks for the next while the reader stays"
    );
    let arrived = offered("e1", "c3", false);
    assert!(
        reach.changed(Some(&arrived), false).is_none(),
        "rows above moved the reader: wait for their next scroll"
    );
    assert!(
        reach
            .scrolled(Some(&arrived), 100.0, 400.0, false)
            .is_some()
    );
    let loading = offered("e0", "c4", true);
    assert!(reach.scrolled(Some(&loading), 50.0, 400.0, false).is_none());
    assert!(
        reach
            .scrolled(Some(&offered("e0", "c4", false)), 50.0, 400.0, true)
            .is_none(),
        "the end of a short transcript is not reading back"
    );
}
