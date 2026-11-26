//! Citations pressed where the side pane's chat draws them inline: a span of the message's
//! Markdown whose press goes to the text view's own link handler (`on_link_press`), not the
//! citation rows (`cite-<citation>`) under the message, which `pane.rs` presses. A citation that
//! resolves opens its task and leaves the chat as it was read, its draft included, beside a
//! wide page and over a narrow one; one that does not resolve draws no link, and its row says
//! why and opens nothing.

use std::time::Instant;

use nsh_core::Point;

use super::*;

/// How long the sample's answer may take: its turn moves a beat per second of real time.
const STREAMED: Duration = Duration::from_secs(30);

/// The `view:` selectors of the inline links reading `spelled` in `scope`: spans that hear the
/// press themselves (a text inside a pressable row is pressed through the row), outside every
/// citation row.
fn inline_links(app: &Headless, scope: &str, spelled: &str) -> Vec<String> {
    let screen = app.screen();
    (0..)
        .map_while(|n| {
            screen
                .id(&format!("{scope} >> text:{spelled} >> nth={n}"))
                .ok()
        })
        .filter(|&id| screen.press_target(&format!("view:{id}")) == Ok(Some(id)))
        .map(|id| format!("view:{id}"))
        .filter(|link| screen.matches(&format!("id:cite-{spelled} >> {link}")) == Ok(false))
        .collect()
}

/// The one inline link reading `spelled` in `scope`, drawn once the citation resolves. Its
/// view is looked up right before the press: the text is laid out again as citations resolve.
fn inline_link(app: &mut Headless, scope: &str, spelled: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let links = inline_links(app, scope, spelled);
        if let [link] = links.as_slice() {
            return link.clone();
        }
        assert!(
            links.is_empty(),
            "{spelled} is linked once in {scope}: {links:?}"
        );
        assert!(
            Instant::now() < deadline,
            "no inline link reads {spelled} in {scope}; texts: {:?}",
            app.screen().texts()
        );
        app.next_change(Duration::from_millis(100)).unwrap();
    }
}

/// Where the side pane's chat is read: T-14's citation row's top below the top of the
/// transcript's scroll view, which moves with the scroll offset.
fn reading(app: &Headless) -> f32 {
    let screen = app.screen();
    let nodes = screen.layout_nodes();
    let top = |selector: &str| {
        let id = screen.id(selector).unwrap();
        nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap_or_else(|| panic!("{selector} is laid out"))
            .bounds
            .origin
            .y
            .0
    };
    top("id:side-pane >> id:cite-T-14") - top("id:side-pane >> id:session-transcript")
}

/// T-14 linked inline in Ask's answer, pressed while the chat is scrolled up from its end and
/// holds a draft: the task's page opens, and the chat is read where it was with its draft,
/// beside the wide page at once, over the narrow one once one press brings it back.
#[test]
fn an_inline_task_link_in_the_side_pane_opens_and_keeps_the_chat_where_it_was_read() {
    for (width, narrow) in [(1440.0, false), (800.0, true)] {
        // Short enough that one long question makes the chat scroll.
        let mut app = preview(
            desktop_sized(width, 560.0),
            &[("corral.destination", "tasks")],
        );
        app.wait_for("id:list-tasks").unwrap();
        app.press_key("/").unwrap();
        app.wait_for("id:side-pane >> id:cite-T-14").unwrap();
        // The answer to a question naming T-11 cites T-11 alone: T-14 stays linked once.
        let question = format!("T-11 {}", "filler ".repeat(160));
        app.type_text("id:ask-input", question.trim_end()).unwrap();
        app.press("id:btn-send").unwrap();
        // The sample streams its answer a few words a second of real time: the whole answer
        // and the turn's end come before the scroll, so the transcript is laid out for good.
        app.wait_for_within(
            "id:side-pane >> text:/I run the corral command for you/",
            STREAMED,
        )
        .unwrap();
        app.wait_for_gone("id:side-pane >> text:/^Working/", STREAMED)
            .unwrap();
        app.type_text("id:ask-input", "still here").unwrap();
        app.wheel(
            "id:side-pane >> id:session-transcript",
            Point::new(Points(0.0), Points(-120.0)),
        )
        .unwrap();
        app.wait_for("id:side-pane >> id:btn-jump-latest").unwrap();
        let read = reading(&app);

        let link = inline_link(&mut app, "id:side-pane", "T-14");
        app.press(&link).unwrap();
        app.wait_for("id:task-page >> id:more-T-14").unwrap();
        if narrow {
            assert_eq!(app.screen().count("id:side-pane").unwrap(), 0);
            app.press("id:pane-back").unwrap();
        }
        app.wait_for("id:side-pane >> id:btn-jump-latest").unwrap();
        let kept = reading(&app);
        assert!(
            (kept - read).abs() < 1.0,
            "{width} wide: the chat is read at {kept}, not where it was ({read})"
        );
        assert_eq!(
            app.screen().input_text("id:ask-input").unwrap(),
            Some("still here")
        );
        app.wait_for("id:task-page >> id:more-T-14").unwrap();
    }
}

/// A citation that does not resolve in a session's chat beside the page: the build's sample
/// turn quotes the message, so its answer cites T-404, which the sample does not have. The
/// answer draws no link over it, its row says "not found", and pressing the row says so and
/// opens nothing: the page stays, and a narrow pane stays over it with its draft.
#[test]
fn an_unresolved_citation_in_the_side_pane_draws_no_link_and_opens_nothing() {
    for width in [1440.0, 800.0] {
        let mut app = preview(
            desktop_sized(width, 900.0),
            &[("corral.destination", "tasks")],
        );
        app.wait_for("id:list-tasks").unwrap();
        app.press_key("ctrl+g").unwrap();
        app.press("id:switch-side-B-k3x9").unwrap();
        app.wait_for("id:dock-tab-B-k3x9").unwrap();
        app.type_text("id:dock-pane >> id:field-message", "Check T-404 first")
            .unwrap();
        app.press("id:dock-pane >> id:btn-send").unwrap();
        app.wait_for("id:dock-pane >> text:/Read your message: “Check T-404 first”/")
            .unwrap();
        let row = "id:dock-pane >> id:cite-T-404 >> nth=0";
        app.wait_for(&format!("{row} >> text:not found")).unwrap();
        assert_eq!(
            inline_links(&app, "id:dock-pane", "T-404"),
            Vec::<String>::new()
        );

        app.type_text("id:dock-pane >> id:field-message", "still here")
            .unwrap();
        app.press(row).unwrap();
        app.wait_for("text:/T-404: not found/").unwrap();
        assert_eq!(app.screen().count("id:task-page").unwrap(), 0);
        assert_eq!(app.screen().count("id:pane-back").unwrap(), 0);
        app.wait_for("id:list-tasks").unwrap();
        app.wait_for("id:side-pane >> id:dock-tab-B-k3x9").unwrap();
        assert_eq!(
            app.screen()
                .input_text("id:dock-pane >> id:field-message")
                .unwrap(),
            Some("still here")
        );
    }
}
