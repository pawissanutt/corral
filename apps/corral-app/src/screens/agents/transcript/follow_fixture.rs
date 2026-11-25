use super::super::*;
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
fn FollowFixture() -> impl IntoElement {
    let app = AppState::install(Form::Terminal);
    let step = signal(0);
    let shown = computed(move || {
        let mut page = crate::model::session::transcript::transcript(
            &Id::from("follow-fixture"),
            &Ui::default(),
            None,
            &Feed::default(),
        );
        page.fill = Fill::Ready;
        page.messages = vec![
            message_at("one", false, "first committed message"),
            message_at("two", false, "second committed message"),
        ];
        let words =
            "A streaming paragraph wraps across the transcript. ".repeat(96 + step.get() * 32);
        page.live = vec![message_at("live", true, words.trim_end())];
        Some(page)
    });
    let content = transcript(app, shown);
    view()
        .w(px(400.0))
        .h(px(240.0))
        .child(
            view()
                .test_id("advance")
                .on_press(move || step.update(|n| *n += 1))
                .child(text("Advance")),
        )
        .child(jump::follow(
            app,
            scroll().test_id("transcript").flex_1().min_h(px(0.0)),
            content,
            move || shown.with(|t| t.as_ref().map(Transcript::following)),
            || None,
            move |key| shown.with(|t| t.as_ref()?.rows_below(key)),
            || 0,
        ))
}

fn start() -> Headless {
    let mut config = EngineConfig::new(Root::of::<FollowFixture>());
    config.environment.idiom = nsh_core::platform::Idiom::Terminal;
    config.environment.cell = Some(nsh_core::Size::new(px(8.0), px(16.0)));
    let mut app = Headless::start(config).unwrap();
    app.wait_for("id:item-2").unwrap();
    app
}

fn bounds(app: &mut Headless, id: &str) -> nsh_core::Bounds<Points> {
    app.screen()
        .layout_nodes()
        .iter()
        .find(|node| node.test_id.as_deref() == Some(id))
        .unwrap()
        .bounds
}

fn tail_gap(app: &mut Headless) -> f32 {
    bounds(app, "item-2").bottom().0 - bounds(app, "transcript").bottom().0
}

fn grow(app: &mut Headless) {
    let height = bounds(app, "item-2").size.height;
    app.press("id:advance").unwrap();
    assert!(
        bounds(app, "item-2").size.height > height,
        "the same item must reflow"
    );
}

#[test]
fn following_keeps_the_last_wrapped_line_visible() {
    let mut app = start();
    let before = tail_gap(&mut app);
    assert!(before <= 0.5, "initial tail is clipped by {before} pt");
    grow(&mut app);
    let after = tail_gap(&mut app);
    assert!(
        after <= 0.5,
        "growing the same item clipped the tail by {after} pt"
    );
}

#[test]
fn wheel_up_holds_the_reading_offset_during_stream_growth() {
    let mut app = start();
    let before = bounds(&mut app, "item-0").origin.y.0;
    app.wheel("id:transcript", nsh_core::Point::new(px(0.0), px(-80.0)))
        .unwrap();
    app.wait_for("id:btn-jump-latest").unwrap();
    let held = bounds(&mut app, "item-0").origin.y.0;
    assert!(held > before, "wheel must scroll toward older content");
    grow(&mut app);
    let after = bounds(&mut app, "item-0").origin.y.0;
    assert!(
        (after - held).abs() <= 0.5,
        "held content moved from {held} to {after} pt"
    );
}
