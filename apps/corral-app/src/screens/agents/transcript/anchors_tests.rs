use super::anchors::{self, Mark};

/// An older page first showed its text and thinking as previews; its full continuation came
/// after, above the reader, with no new first message and no new row. The hold on the reader
/// was spent when the page was first placed, so the rows filling in above slid the reader down.
/// A view scrolled up holds its reader through every later layout, from the place it saw last,
/// until the reader scrolls on their own (then from there) or follows the end; another view of
/// the same scope holds its own.
#[test]
fn rows_filling_in_above_after_a_page_showed_keep_the_reader_until_they_scroll() {
    let scope = "B-relayout";
    let (view, second) = (anchors::token(), anchors::token());
    let reader = "B-relayout/entry/e5";
    let (earlier, read) = (format!("{reader}|2"), format!("{reader}|1"));
    let settled = || -> Vec<f32> {
        let moves = anchors::settle(scope);
        moves.into_iter().map(|(_, at)| at).collect()
    };
    let row = |anchor: &str, top: f32, height: f32| {
        anchors::placed(scope, anchor, anchors::token(), top, height)
    };
    // The reader's message holds a preview (40) then the item they read (60), 10 into it.
    let items = |preview: f32| {
        let token = anchors::token();
        anchors::item_placed(scope, reader, &earlier, None, token, 0.0, preview);
        anchors::item_placed(scope, reader, &read, None, token, preview, 60.0);
        preview + 60.0
    };
    anchors::set_origin(scope, anchors::token(), 0.0);
    row("B-relayout/entry/e3", 0.0, 200.0);
    row("B-relayout/entry/e4", 200.0, 200.0);
    let high = items(40.0);
    row(reader, 400.0, high);
    row("B-relayout/entry/e6", 400.0 + high, 300.0);
    assert!(settled().is_empty(), "nothing holds a reader at the end");

    // The reader scrolls up to 50 into their message; a second view of the scope reads e3.
    anchors::scrolled(scope, view, 450.0);
    anchors::scrolled(scope, second, 20.0);
    assert!(settled().is_empty(), "scrolling alone moves no one");

    // The older page shows above as a 60 high preview: the first message changed.
    anchors::hold(scope, view, reader.to_owned(), 50.0);
    row("B-relayout/entry/e1", 0.0, 60.0);
    row("B-relayout/entry/e3", 60.0, 200.0);
    row("B-relayout/entry/e4", 260.0, 200.0);
    row(reader, 460.0, high);
    assert_eq!(settled(), [510.0, 80.0], "each view keeps its reader");
    // The scroll the hold made comes back: it is not the reader's own.
    anchors::scrolled(scope, view, 510.0);

    // Later the page fills in: its message grows to 300, and the preview in the reader's own
    // message to 140, above the item they read. Nothing new shows, the first message stays.
    row("B-relayout/entry/e1", 0.0, 300.0);
    row("B-relayout/entry/e3", 300.0, 200.0);
    row("B-relayout/entry/e4", 500.0, 200.0);
    let high = items(140.0);
    row(reader, 700.0, high);
    assert_eq!(
        settled(),
        [700.0 + 140.0 + 10.0, 320.0],
        "the reader stays 10 into the item they read"
    );
    assert!(settled().is_empty(), "settled once per layout");
    anchors::scrolled(scope, view, 850.0);
    row("B-relayout/entry/e6", 700.0 + high, 360.0);
    assert!(settled().is_empty(), "a row growing below moves no one");

    // The reader scrolls on their own to 20 into e4: they are held there, not where they were.
    anchors::scrolled(scope, view, 520.0);
    anchors::release(scope, second);
    row("B-relayout/entry/e1", 0.0, 100.0);
    row("B-relayout/entry/e3", 100.0, 200.0);
    row("B-relayout/entry/e4", 300.0, 200.0);
    row(reader, 500.0, high);
    assert_eq!(
        settled(),
        [320.0],
        "held where they scrolled to; the other view let go"
    );

    // Back at the end, nothing holds them.
    anchors::release(scope, view);
    row("B-relayout/entry/e1", 0.0, 400.0);
    assert!(settled().is_empty());
}

/// An opened group was one item of its message: when calls joined it before and after the one
/// the reader was in, its height changed, nothing held the reader and the view slid down by
/// the calls that joined below. Its calls are placed by their ids through the group's rows, the
/// group and the message's column of items, and the reader stays on the call they read.
#[test]
fn calls_joining_an_opened_group_keep_the_reader_on_their_call() {
    let scope = "B-group/child/reader";
    let row = "B-group/child/reader/entry/e1";
    // In the phone child scroll, padding + header + gap precede the transcript.
    let prefix = 8.0 + 102.0 + 10.0;
    anchors::set_origin(scope, anchors::token(), prefix);
    let key = |from_end: usize| format!("{row}|{from_end}");
    let column = format!("{row}#items");
    // One render: its tokens, so the next render's places outlive this one's teardown.
    let render = |items: &[(String, Option<String>, f32, f32)],
                  parts: &[(String, Option<String>, f32, f32)]| {
        let mut placed = Vec::new();
        for (key, within, top, height) in parts {
            let token = anchors::token();
            anchors::part_placed(scope, row, key, within.as_deref(), token, *top, *height);
            placed.push((key.clone(), token));
        }
        for (key, within, top, height) in items {
            let token = anchors::token();
            anchors::item_placed(scope, row, key, within.as_deref(), token, *top, *height);
            placed.push((key.clone(), token));
        }
        placed
    };
    let at = |key: &str, within: &str, top: f32, height: f32| {
        (key.to_owned(), Some(within.to_owned()), top, height)
    };
    // A terminal: rows a cell (18) high, the head a row above the column of items, the group's
    // head a row above its calls. The reader is 12 into call c, the group's last, 108 into the
    // message; the item after the group starts 18 below them, inside the head's height.
    let group = key(2);
    let (block, calls) = (format!("{group}#group"), format!("{group}#rows"));
    let row_token = anchors::token();
    anchors::placed(scope, row, row_token, 100.0, 162.0);
    let before = render(
        &[
            at(&key(3), &column, 0.0, 36.0),
            at(&group, &column, 36.0, 72.0),
            at("call:a", &calls, 0.0, 18.0),
            at("call:b", &calls, 18.0, 18.0),
            at("call:c", &calls, 36.0, 18.0),
            at(&key(1), &column, 108.0, 36.0),
        ],
        &[
            (column.clone(), None, 18.0, 144.0),
            at(&block, &group, 0.0, 72.0),
            at(&calls, &block, 18.0, 54.0),
        ],
    );
    anchors::placed(scope, "next message", anchors::token(), 270.0, 54.0);
    let viewport = prefix + 100.0 + 120.0;
    let (visible, into) = anchors::at(scope, viewport).unwrap();
    assert_eq!((visible.as_str(), into), (row, 120.0));
    let mark = anchors::mark(scope, &visible, into);
    let Mark::Held { items, .. } = &mark else {
        panic!("the row was placed: {mark:?}");
    };
    assert!(
        items.contains(&("call:c".to_owned(), 108.0, 18.0)),
        "the call is placed in the message through its group: {items:?}"
    );

    // Rows above add 60; the record's earlier items add a paragraph and call z at the group's
    // front (the group now keyed by z), and call d joins it at its end.
    let group = key(4);
    let (block, calls) = (format!("{group}#group"), format!("{group}#rows"));
    anchors::placed(scope, row, anchors::token(), 160.0, 234.0);
    render(
        &[
            at(&key(5), &column, 0.0, 36.0),
            at(&key(3), &column, 36.0, 36.0),
            at(&group, &column, 72.0, 108.0),
            at("call:z", &calls, 0.0, 18.0),
            at("call:a", &calls, 18.0, 18.0),
            at("call:b", &calls, 36.0, 18.0),
            at("call:c", &calls, 54.0, 18.0),
            at("call:d", &calls, 72.0, 18.0),
            at(&key(1), &column, 180.0, 36.0),
        ],
        &[
            (column.clone(), None, 18.0, 216.0),
            at(&block, &group, 0.0, 108.0),
            at(&calls, &block, 18.0, 90.0),
        ],
    );
    // The first render goes after the second placed its rows.
    anchors::forget(scope, row, row_token);
    for (key, token) in before {
        anchors::item_forget(scope, &key, token);
    }
    // Call c is now 18 + 72 + 18 + 54 = 162 into the message, 54 lower than it was.
    assert_eq!(
        anchors::offset(scope, row, &mark),
        Some(prefix + 160.0 + 120.0 + 54.0),
        "the reader stays 12 into call c"
    );
}
