use super::*;

#[test]
fn authority_notice_refreshes_the_current_session_and_invalidates_preview_gates() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    feed.update(|feed| {
        feed.store = Some("/authority-notification-store".into());
    });
    let session = |act: bool| {
        json!({
            "device":{"id":"authority-device","name":"phone","platform":"web","app_version":"0",
                "enrolled_at":"2026-11-15T10:00:00Z","revoked_at":null},
            "authority":{"reply":true,"act":act,"assistant":true,"assistant_act":false,"admin":false},
            "seen":{"through":0,"at":null},"server_time":"2026-11-15T10:00:00Z"
        })
    };
    let mut app = AppState::new();
    app.event(
        feed,
        Event::Reply {
            method: APP_SESSION,
            tag: None,
            params: json!({}),
            outcome: Ok(session(true)),
            error_data: None,
        },
    )
    .unwrap();
    feed.update(|feed| {
        feed.operations.insert(
            Id::from("old-preview"),
            Operation {
                method: ACT_PREVIEW.into(),
                loading: false,
                value: Some(json!({})),
                error: None,
            },
        );
    });
    let outgoing = app
        .event(
            feed,
            Event::Notice {
                method: SESSION_AUTHORITY_CHANGED.into(),
                params: json!({"device":"authority-device","revision":1}),
            },
        )
        .unwrap();
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].0, APP_SESSION);
    assert!(feed.with_untracked(|feed| !feed.session.as_ref().unwrap().authority.act));
    assert!(feed.with_untracked(|feed| !feed.operations.contains_key("old-preview")));
    assert_eq!(feed.with_untracked(|feed| feed.authority_revision), 1);
    assert!(
        app.event(
            feed,
            Event::Reply {
                method: APP_SESSION,
                tag: None,
                params: json!({}),
                outcome: Ok(session(false)),
                error_data: None
            }
        )
        .unwrap()
        .is_empty()
    );
    assert!(
        feed.with_untracked(|feed| feed.session.as_ref().unwrap().authority.reply
            && !feed.session.as_ref().unwrap().authority.act)
    );
    assert!(
        app.event(
            feed,
            Event::Notice {
                method: SESSION_AUTHORITY_CHANGED.into(),
                params: json!({"device":"authority-device","revision":1})
            }
        )
        .unwrap()
        .is_empty()
    );
}
