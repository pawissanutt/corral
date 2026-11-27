use super::*;
use crate::model::session::transcript::More;
use nsh_core::{
    Size,
    platform::{Idiom, Os},
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, LayoutNode};

const TABLE_START: &str = "## Comparison\n\n- Outer item\n  - Nested item\n\n> Quoted note\n\n\
    | What we'd lose by default | What that means in Corral |\n\
    | --- | --- |\n\
    | **Kind-specific checks** | Today, a state lens can check its states against a Rust enum when those facts are available. A generic graph only knows that its links point to valid nodes. |\n\
    | **Specialized notation and layout** | A graph can show ER cardinalities as labels, but it won't automatically give you crow's feet. |\n\
    | **Authoring defaults** | A dedicated state format tells an agent";
const TABLE_END: &str = " where to put a `guard`. [Reference](https://example.org/) |\n\n```rust\nfn example() {}\n```\n\n[Manual](https://example.org/manual)";

#[component]
fn ComparisonFixture() -> impl IntoElement {
    let app = AppState::install(Form::Desktop);
    crate::screens::markdown::provide_styles(app);
    let streaming = signal(true);
    let narrow = signal(false);
    let words = computed(move || {
        if streaming.get() {
            TABLE_START.to_owned()
        } else {
            [TABLE_START, TABLE_END].concat()
        }
    });
    let saved = Prose {
        markdown: [TABLE_START, TABLE_END].concat(),
        more: More::Whole,
        streaming: false,
    };
    view()
        .w(move || px(if narrow.get() { 300.0 } else { 700.0 }))
        .h(px(450.0))
        .child(
            view()
                .flex_row()
                .gap(px(12.0))
                .child(
                    view()
                        .test_id("finish")
                        .on_press(move || streaming.set(false))
                        .child(text("Finish")),
                )
                .child(
                    view()
                        .test_id("narrow")
                        .on_press(move || narrow.set(true))
                        .child(text("Narrow")),
                ),
        )
        .child(
            scroll()
                .flex_1()
                .test_id("comparison-scroll")
                .child(view().h(px(150.0)))
                .child(
                    view()
                        .test_id("reading-anchor")
                        .child(text("Earlier reply")),
                )
                .child(
                    view()
                        .test_id("saved-comparison")
                        .child(prose(app, &saved, "saved", None)),
                )
                .child(view().test_id("live-comparison").child(prose(
                    app,
                    &saved,
                    "live",
                    Some(Written {
                        words,
                        number: computed(|| 1),
                        streaming: computed(move || streaming.get()),
                    }),
                ))),
        )
}

#[test]
fn wide_chat_aligns_comparison_columns_and_finishes_streaming_in_place() {
    for os in [Os::Web, Os::Linux] {
        let mut config = EngineConfig::new(Root::of::<ComparisonFixture>());
        config.viewport = Size::new(px(1100.0), px(700.0));
        config.environment.os = os;
        config.environment.idiom = Idiom::Desktop;
        let mut app = Headless::start(config).unwrap();
        app.wait_for("id:finish").unwrap();
        app.scroll("id:comparison-scroll", px(140.0)).unwrap();
        let before = app.screen().layout_nodes();
        comparison(&before, "saved-comparison", true);
        comparison(&before, "live-comparison", false);
        let anchor = region(&before, "reading-anchor");
        assert!(anchor.bounds.top() < px(70.0), "the reader scrolled");
        app.press("id:finish").unwrap();
        let after = app.screen().layout_nodes();
        comparison(&after, "live-comparison", true);
        assert_eq!(
            region(&after, "live-comparison").id,
            region(&before, "live-comparison").id
        );
        assert_eq!(
            region(&after, "reading-anchor").bounds,
            anchor.bounds,
            "finishing a reply must not move an earlier reader"
        );

        app.scroll("id:comparison-scroll", px(10_000.0)).unwrap();
        app.press("id:live-comparison >> text:Reference").unwrap();
        assert_eq!(app.screen().opened_urls(), ["https://example.org/"]);

        app.press("id:narrow").unwrap();
        let narrow = app.screen().layout_nodes();
        let scroll = region(&narrow, "comparison-scroll").bounds;
        let rows: Vec<_> = narrow
            .iter()
            .filter(|node| {
                node.text.as_ref().is_some_and(|t| {
                    t.text.text.contains("What that means in Corral")
                        && t.text.text.contains("a guard.")
                })
            })
            .collect();
        assert_eq!(
            rows.len(),
            2,
            "the same tables become labeled records when the pane narrows"
        );
        assert!(rows.iter().all(|r| r.bounds.right() <= scroll.right()));
    }
}

fn region<'a>(nodes: &'a [LayoutNode], id: &str) -> &'a LayoutNode {
    nodes
        .iter()
        .find(|n| n.test_id.as_deref() == Some(id))
        .unwrap()
}

fn comparison(nodes: &[LayoutNode], id: &str, finished: bool) {
    let region = region(nodes, id);
    let find = |part: &str| {
        nodes
            .iter()
            .find(|n| {
                if !n
                    .text
                    .as_ref()
                    .is_some_and(|t| t.text.text.starts_with(part))
                {
                    return false;
                }
                let mut parent = n.parent;
                while let Some(ancestor) = nodes.iter().find(|n| n.id == parent) {
                    if ancestor.id == region.id {
                        return true;
                    }
                    parent = ancestor.parent;
                }
                false
            })
            .unwrap_or_else(|| panic!("{id}: missing {part:?}"))
    };
    let left = find("What we'd lose by default").bounds;
    let right = find("What that means in Corral").bounds;
    assert!(left.right() <= right.left());
    assert!(right.right() <= region.bounds.right());
    assert_eq!(left.top(), right.top());
    let mut previous = left.top();
    for (label, value) in [
        ("Kind-specific checks", "Today, a state lens"),
        ("Specialized notation and layout", "A graph can show"),
        ("Authoring defaults", "A dedicated state format"),
    ] {
        let label = find(label).bounds;
        let cell = find(value);
        let value = cell.bounds;
        assert_eq!(label.left(), left.left());
        assert_eq!(value.left(), right.left());
        assert_eq!(label.top(), value.top());
        assert!(value.right() <= region.bounds.right());
        assert!(label.top() > previous);
        previous = label.top();
    }
    let last = find("A dedicated state format").text.as_ref().unwrap();
    let tail = if finished {
        "a guard. Reference"
    } else {
        "an agent"
    };
    assert!(last.text.text.ends_with(tail));
    assert!(
        find("Today, a state lens").bounds.size.height > px(40.0),
        "long prose wraps inside the grid"
    );
    for part in ["Comparison", "Nested item", "Quoted note"] {
        find(part);
    }
    if finished {
        find("fn example() {}");
        find("Manual");
    }
}

#[component]
fn TableFixture() -> impl IntoElement {
    let app = AppState::install(match idiom() {
        Idiom::Terminal => Form::Terminal,
        Idiom::Desktop => Form::Desktop,
        _ => Form::Phone,
    });
    crate::screens::markdown::provide_styles(app);
    let value = "a".repeat(112);
    let words = Prose {
        markdown: format!(
            "| First | Second | Third | Fourth | Last |\n| --- | --- | --- | --- | --- |\n| a long first field | a long second field | `{value}` | *fourth value* | sentinel |"
        ),
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
    for (idiom, os, width) in [
        (Idiom::Terminal, Os::Linux, 640.0),
        (Idiom::Desktop, Os::Web, 1440.0), // a narrow dock in a wide window
        (Idiom::Phone, Os::Web, 320.0),
    ] {
        let mut config = EngineConfig::new(Root::of::<TableFixture>());
        config.viewport = Size::new(px(width), px(600.0));
        config.environment.os = os;
        config.environment.idiom = idiom;
        if idiom == Idiom::Terminal {
            config.environment.cell = Some(Size::new(px(8.0), px(16.0)));
        }
        let mut app = Headless::start(config).unwrap();
        app.wait_for("id:conversation").unwrap();
        let nodes = app.screen().layout_nodes();
        let conversation = region(&nodes, "conversation").bounds;
        for content in [
            "a long first field",
            "fourth value",
            "sentinel",
            &"a".repeat(112),
        ] {
            let node = nodes
                .iter()
                .find(|n| {
                    n.text
                        .as_ref()
                        .is_some_and(|t| t.text.text.contains(content))
                })
                .unwrap_or_else(|| panic!("{idiom:?}: missing cell {content}"));
            assert!(
                !node.hidden
                    && node.bounds.left() >= conversation.left()
                    && node.bounds.right() <= conversation.right(),
                "{idiom:?}: cell {content} fits inside conversation: {:?}, conversation {:?}",
                node.bounds,
                conversation,
            );
            if content.starts_with("aaaa") {
                assert!(
                    node.bounds.size.height > px(40.0),
                    "long code value must wrap"
                );
            }
        }
    }
}
