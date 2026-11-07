use super::*;
use crate::app::mouse_tests::render;
use crate::board::tests::{code, key, seeded};
use crate::{Mode, board::BoardView};
use corral_core::pm_inbox::{ItemKind, NewPmItem};

fn post(ctx: &Ctx, from: &str, subject: &str) -> String {
    ctx.store
        .pm_inbox_post(NewPmItem {
            from: from.into(),
            kind: ItemKind::Question,
            subject: subject.into(),
            body: "A question for the PM".into(),
            refs: vec![],
            requested_action: None,
            owner_words: None,
            key: None,
        })
        .unwrap()
        .0
        .id
}

#[test]
fn board_driver_sessions_inbox_badge_and_cowork_done() {
    let fx = seeded();
    let session = fx
        .ctx
        .store
        .create_assistant("pi", Some("T-003"), Some("w2:p1"), None)
        .unwrap();
    let ended = fx
        .ctx
        .store
        .create_assistant("pi", None, None, None)
        .unwrap();
    fx.ctx.store.end_assistant(&ended.id).unwrap();
    let from = format!("assistant:{}", session.id);
    let open = post(&fx.ctx, &from, "Check cache evidence");
    let closed = post(&fx.ctx, &from, "Closed item must not count");
    fx.ctx
        .store
        .pm_inbox_withdraw(&closed, 0, &from, "No longer needed")
        .unwrap();
    fx.ctx
        .store
        .update("T-007", |t| {
            t.cowork = Some(corral_core::Cowork {
                completion: None,
                session: session.id.clone(),
                scratch: "cache".into(),
                run: "R1".into(),
                review: corral_core::CoworkReview::None,
                owner_confirmed: false,
            });
            Ok(())
        })
        .unwrap();
    let mut app = fx.app(Mode::Board);
    let text = render(&mut app);
    assert!(text.contains("S asst inbox 1"), "{text}");
    app.handle_key(key('S'));
    let text = render(&mut app);
    for expected in [
        "asst A-1",
        "focus T-003",
        "pane w2:p1",
        "cost unknown",
        "scratch 0",
        "open inbox 1",
        "asst A-2",
        "ended",
        &open,
        "Check cache evidence",
        "question",
        "PM inbox [1]",
    ] {
        assert!(text.contains(expected), "missing {expected}:\n{text}");
    }
    assert!(!text.contains("Closed item must not count"));
    println!("TUI DRIVER assistants panel and PM inbox:\n{text}");
    app.handle_key(code(KeyCode::Esc));
    let text = render(&mut app);
    assert!(
        text.lines()
            .any(|l| l.contains("T-007") && l.contains("asst")),
        "{text}"
    );
    app.handle_key(key('Y'));
    let history = render(&mut app);
    assert!(
        history
            .lines()
            .any(|l| l.contains("T-007") && l.contains("asst")),
        "{history}"
    );
    // A card action is a new assistant, not the run-pane jump or a PM handoff.
    let mut board = BoardView::new(&fx.ctx).unwrap();
    assert!(matches!(
        board.on_key(key('s'), &fx.ctx),
        Action::OpenAssistant(_)
    ));
    assert_eq!(board.on_key(key('S'), &fx.ctx), Action::OpenAssistants);
    println!(
        "TUI DRIVER: session fields, ended session, open-only PM inbox badge/list, asst source and Done cowork card PASS\n{text}"
    );
}

#[test]
fn remapped_panel_key_and_badge_click_use_the_same_route() {
    let mut fx = seeded();
    fx.ctx
        .store
        .config
        .ui
        .keys
        .insert("assistants".into(), "z".into());
    let mut app = fx.app(Mode::Board);
    let text = render(&mut app);
    assert!(text.contains("z asst inbox 0"));
    app.handle_key(key('S'));
    assert!(render(&mut app).contains("z asst inbox 0"));
    app.handle_key(key('z'));
    assert!(render(&mut app).contains("No assistant sessions yet"));
    app.handle_key(code(KeyCode::Esc));
    let text = render(&mut app);
    let (x, y) = crate::app::mouse_tests::find_any(&text, "z asst inbox 0");
    crate::app::mouse_tests::click(&mut app, x, y);
    assert!(render(&mut app).contains("No assistant sessions yet"));
}

#[test]
fn launch_command_quotes_each_argument() {
    assert_eq!(
        launch_command(std::path::Path::new("/a path/corral's"), "T-42"),
        "'/a path/corral'\\''s' pi --task 'T-42'"
    );
}

#[test]
fn empty_panel_and_refresh() {
    let fx = seeded();
    let mut panel = AssistantsView::new(&fx.ctx).unwrap();
    assert!(
        panel
            .lines()
            .iter()
            .any(|l| l.to_string().contains("No assistant sessions"))
    );
    post(&fx.ctx, "owner", "Owner request");
    panel.refresh(&fx.ctx);
    assert_eq!(panel.inbox.len(), 1);
    assert!(
        panel
            .lines()
            .iter()
            .any(|l| l.to_string().contains("owner · question"))
    );
}
