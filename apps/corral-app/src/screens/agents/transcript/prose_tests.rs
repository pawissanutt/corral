use super::*;
use crate::model::session::transcript::More;
use nsh_core::{
    Points, Size,
    platform::{Idiom, Os},
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

#[component]
fn TableFixture() -> impl IntoElement {
    let app = AppState::install(Form::Terminal);
    let words = Prose {
        markdown: "| First | Second | Last |\n| --- | --- | --- |\n| a long first field | a long second field | sentinel |".into(),
        more: More::Whole,
        streaming: false,
    };
    view()
        .w(px(240.0))
        .test_id("conversation")
        .child(prose(app, &words, "fixture", None))
}

#[test]
fn chat_table_last_column_is_visible_in_a_narrow_conversation() {
    let mut config = EngineConfig::new(Root::of::<TableFixture>());
    config.viewport = Size::new(Points(960.0), Points(600.0));
    config.environment.os = Os::Linux;
    config.environment.idiom = Idiom::Terminal;
    config.environment.cell = Some(Size::new(Points(8.0), Points(16.0)));
    let mut app = Headless::start(config).unwrap();
    app.wait_for("id:conversation").unwrap();
    let nodes = app.screen().layout_nodes();
    let conversation = nodes
        .iter()
        .find(|n| n.test_id.as_deref() == Some("conversation"))
        .unwrap()
        .bounds;
    let text = nodes
        .iter()
        .find(|n| {
            n.text
                .as_ref()
                .is_some_and(|t| t.text.text.contains("sentinel"))
        })
        .expect("last cell is rendered");
    assert!(
        !text.hidden
            && text.bounds.origin.x.0 >= conversation.origin.x.0
            && text.bounds.origin.x.0 + text.bounds.size.width.0
                <= conversation.origin.x.0 + conversation.size.width.0,
        "last cell outside conversation: {:?}, conversation {:?}",
        text.bounds,
        conversation
    );
}
