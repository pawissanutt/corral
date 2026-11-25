use super::*;
use crate::model::session::transcript::{More, Prose};
use crate::model::{Id, daemon::Feed, session::Ui};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

fn message_at(anchor: &str, live: bool, words: &str) -> Message {
    Message {
        id: anchor.into(),
        anchor: anchor.into(),
        kind: MessageKind::Agent,
        avatar: "A",
        hue: Hue::Held,
        head: vec![span(format!("head {anchor}"))],
        author: anchor.into(),
        detail: String::new(),
        tag: None,
        tag_facts: None,
        items: vec![MessageItem::Prose(Prose {
            markdown: words.into(),
            more: More::Whole,
            streaming: live,
        })],
        keys: if live {
            vec![]
        } else {
            vec![format!("{anchor}#0")]
        },
        footer: None,
        live,
    }
}

#[component]
fn StreamingFixture() -> impl IntoElement {
    let app = AppState::install(Form::Terminal);
    let step = signal(0);
    let shown = computed(move || {
        let step = step.get();
        let mut page = crate::model::session::transcript::transcript(
            &Id::from("fixture"),
            &Ui::default(),
            None,
            &Feed::default(),
        );
        page.fill = Fill::Ready;
        page.messages = vec![
            message_at("one", false, "committed one"),
            message_at("two", false, "committed two"),
        ];
        if step == 2 {
            page.messages
                .insert(0, message_at("older", false, "earlier message"));
        }
        page.live = vec![message_at(
            "live",
            true,
            if step == 0 {
                "stream initial"
            } else {
                "stream updated"
            },
        )];
        Some(page)
    });
    view()
        .child(button_with(
            app,
            "Advance",
            ButtonKind::Quiet,
            ButtonOptions::default(),
            "advance",
            move || step.update(|n| *n += 1),
        ))
        .child(transcript(app, &Reader::new(app), shown))
}

#[test]
fn streaming_delta_retains_mounted_messages_and_item_wrappers() {
    let mut config = EngineConfig::new(Root::of::<StreamingFixture>());
    config.environment.idiom = nsh_core::platform::Idiom::Terminal;
    config.environment.cell = Some(nsh_core::Size::new(
        nsh_core::Points(8.0),
        nsh_core::Points(16.0),
    ));
    let mut app = Headless::start(config).unwrap();
    app.wait_for("id:item-2").unwrap();
    let nodes = |app: &mut Headless, first: usize| {
        let nodes = app.screen().layout_nodes();
        let item = |n| {
            nodes
                .iter()
                .find(|node| node.test_id.as_deref() == Some(format!("item-{n}").as_str()))
                .unwrap()
        };
        let text = |words| {
            nodes
                .iter()
                .find(|node| node.text.as_ref().is_some_and(|t| t.text.text == words))
                .unwrap()
                .id
        };
        [
            nodes
                .iter()
                .find(|node| node.test_id.as_deref() == Some("session-transcript-items"))
                .unwrap()
                .id,
            item(first).id,
            item(first + 1).id,
            item(first + 2).id,
            text("committed one"),
            text("committed two"),
            text("▌"),
            text("head live"),
            item(first).parent,
            item(first + 1).parent,
        ]
    };
    let original = nodes(&mut app, 0);
    app.press("id:advance").unwrap();
    app.wait_for("text:stream updated").unwrap();
    assert_eq!(
        original,
        nodes(&mut app, 0),
        "stream delta remounted retained nodes"
    );
    app.press("id:advance").unwrap();
    app.wait_for("id:item-3").unwrap();
    let prepended = nodes(&mut app, 1);
    assert_eq!(
        original[0], prepended[0],
        "prepend remounted the measured column"
    );
    assert_eq!(
        &original[6..8],
        &prepended[6..8],
        "prepend remounted the live header"
    );
    assert_eq!(
        &original[8..],
        &prepended[8..],
        "prepend remounted the placed item wrappers"
    );
}
