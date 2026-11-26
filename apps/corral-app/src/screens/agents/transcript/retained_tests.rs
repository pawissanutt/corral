use super::*;
use crate::model::session::Action;
use crate::model::session::transcript::{More, Prose, Thinking};
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
        items: live
            .then(|| {
                MessageItem::Thinking(Thinking {
                    text: "reasoning so far".into(),
                    open: true,
                    hidden: String::new(),
                    hidden_all: false,
                    more: More::Whole,
                    streaming: true,
                    action: Action::Thinking(Id::from("fixture")),
                })
            })
            .into_iter()
            .chain([MessageItem::Prose(Prose {
                markdown: words.into(),
                more: More::Whole,
                streaming: live,
            })])
            .collect(),
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
        page.messages.push(message_at(
            "live",
            true,
            if step == 0 {
                "stream settled\n\nstream initial"
            } else {
                "stream settled\n\nstream updated"
            },
        ));
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

#[component]
fn ReplacementFixture() -> impl IntoElement {
    use crate::model::{
        daemon::DetailCache,
        session::transcript::{mirror, transcript as replay},
    };
    use corral_protocol::{
        detail::{DetailRef, SessionItem, SessionMessage},
        subscription::TranscriptOp,
    };
    let app = AppState::install(Form::Terminal);
    let recorded = signal(false);
    let shown = computed(move || {
        let mut cache = DetailCache {
            target: DetailRef::Session {
                id: "fixture".into(),
            },
            pages: Vec::new(),
            transcript_seams: Default::default(),
            merged: None,
            loading: false,
            stale: false,
            error: None,
        };
        let put = |id: &str, replaces: Option<&str>, open| TranscriptOp::Put {
            message: SessionMessage {
                id: id.into(),
                author: "assistant".into(),
                at: None,
                items: vec![SessionItem::Text {
                    text: "persistent words about T-9".into(),
                }],
                model: None,
                stop_reason: None,
                usage: None,
                input: None,
                attachments: Vec::new(),
            },
            before: None,
            replaces: replaces.map(str::to_owned),
            open,
            cut: false,
        };
        mirror::apply(
            &mut cache,
            vec![put("fixture/open/1.1.1", None, true)],
            false,
        );
        if recorded.get() {
            mirror::apply(
                &mut cache,
                vec![put(
                    "fixture/entry/record",
                    Some("fixture/open/1.1.1"),
                    false,
                )],
                false,
            );
        }
        Some(replay(
            &Id::from("fixture"),
            &Ui::default(),
            Some(&cache),
            &Feed::default(),
        ))
    });
    view()
        .child(button_with(
            app,
            "Record",
            ButtonKind::Quiet,
            ButtonOptions::default(),
            "record",
            move || recorded.set(true),
        ))
        .child(transcript(app, &Reader::new(app), shown))
}

#[test]
fn replacing_open_message_keeps_its_text_node() {
    let mut config = EngineConfig::new(Root::of::<ReplacementFixture>());
    config.environment.idiom = nsh_core::platform::Idiom::Terminal;
    config.environment.cell = Some(nsh_core::Size::new(
        nsh_core::Points(8.0),
        nsh_core::Points(16.0),
    ));
    let mut app = Headless::start(config).unwrap();
    let words = |app: &mut Headless| {
        app.screen()
            .layout_nodes()
            .iter()
            .find(|node| {
                node.text
                    .as_ref()
                    .is_some_and(|text| text.text.text == "persistent words about T-9")
            })
            .unwrap()
            .id
    };
    app.wait_for("text:▌").unwrap();
    let before = words(&mut app);
    app.press("id:record").unwrap();
    app.wait_for_gone("text:▌", std::time::Duration::from_secs(2))
        .unwrap();
    assert_eq!(before, words(&mut app));
    // What the words cite is listed once the reply has ended, beside the view they keep.
    app.wait_for("id:cite-T-9").unwrap();
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
    // What is being written that neither a delta nor a renumbering changes: the reply's
    // earlier paragraph and the reasoning before it.
    let earlier = |app: &mut Headless| {
        let nodes = app.screen().layout_nodes();
        ["stream settled", "reasoning so far"].map(|words| {
            nodes
                .iter()
                .find(|node| node.text.as_ref().is_some_and(|t| t.text.text == words))
                .unwrap()
                .id
        })
    };
    let original = nodes(&mut app, 0);
    let words = earlier(&mut app);
    app.press("id:advance").unwrap();
    app.wait_for("text:stream updated").unwrap();
    assert_eq!(
        original,
        nodes(&mut app, 0),
        "stream delta remounted retained nodes"
    );
    assert_eq!(
        words,
        earlier(&mut app),
        "stream delta remounted the reply's earlier words"
    );
    app.press("id:advance").unwrap();
    app.wait_for("id:item-4").unwrap();
    let prepended = nodes(&mut app, 1);
    assert_eq!(
        words,
        earlier(&mut app),
        "renumbering remounted the reply's words"
    );
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

const HANDOFF: &str = "Paste this into Code mode:\n\n\
                       > Fix the selection in `markdown.rs`.\n>\n\
                       > Then run the gate and attach the log.\n\n\
                       That is all.";

#[component]
fn HandoffFixture() -> impl IntoElement {
    let app = AppState::install(Form::Desktop);
    let shown = computed(move || {
        let mut page = crate::model::session::transcript::transcript(
            &Id::from("fixture"),
            &Ui::default(),
            None,
            &Feed::default(),
        );
        page.fill = Fill::Ready;
        page.messages = vec![message_at("handoff", false, HANDOFF)];
        Some(page)
    });
    view().child(transcript(app, &Reader::new(app), shown))
}

/// T-2119: a quoted multi-paragraph handoff in a desktop reply could not be selected, and
/// nothing copied the reply whole. Its quoted words now sit in a selectable view, and Copy
/// reply puts the recorded Markdown, quote lines and breaks included, on the clipboard.
#[test]
fn a_desktop_reply_with_a_quoted_handoff_selects_and_copies_whole() {
    let mut config = EngineConfig::new(Root::of::<HandoffFixture>());
    config.environment.os = nsh_core::platform::Os::Web;
    config.environment.idiom = nsh_core::platform::Idiom::Desktop;
    let mut app = Headless::start(config).unwrap();
    app.wait_for("text:/Then run the gate/").unwrap();
    for words in ["text:/Then run the gate/", "text:/That is all/"] {
        assert_eq!(app.screen().in_selectable_view(words), Ok(true), "{words}");
    }
    app.press("id:btn-copy-reply-1").unwrap();
    assert_eq!(app.screen().clipboard(), Some(HANDOFF));
}
