use std::{fs, os::unix::fs::symlink};

use corral_protocol::{
    self as wire, app,
    detail::{DetailBody, DetailGet, DetailRef, DetailReply, SESSION_FILE_SEARCH_LIMIT_MAX},
    frame::{CONTROL_MAX, Frame, FrameKind},
    rpc,
};
use serde_json::json;

use crate::{
    app::dispatch,
    conn::{Conn, Peer},
    tests::Fixture,
    topics::Subscriptions,
};

#[test]
fn file_search_ranks_nested_matches_and_only_serves_confined_unignored_files() {
    #[derive(Default)]
    struct Capture(Option<Frame>);
    impl Conn for Capture {
        fn send_frame(&mut self, frame: &Frame) -> anyhow::Result<()> {
            assert!(self.0.replace(frame.clone()).is_none());
            Ok(())
        }
    }

    let fixture = Fixture::new();
    let repo = fixture.root().canonicalize().unwrap();
    let checkout = repo.join(".worktrees/build-walk");
    let record = repo.join("pi-data/build/B-1");
    let nested = "apps/corral-app/src/model/session";
    for dir in [nested, "blocked", ".corral", "sessmod-directory"] {
        fs::create_dir_all(checkout.join(dir)).unwrap();
    }
    fs::create_dir_all(&record).unwrap();
    fs::write(
        record.join("session.json"),
        json!({
            "id": "B-1", "name": "walk", "workspaceRoot": repo, "repoRoot": repo,
            "worktree": checkout, "here": false,
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        checkout.join(".gitignore"),
        "blocked/\n!blocked/sessmod.rs\n*.generated.rs\n!{sessmod,a}.generated.rs\n",
    )
    .unwrap();
    fs::write(
        checkout.join(nested).join(".gitignore"),
        "/sessmod.rs\n!/mod.generated.rs\n",
    )
    .unwrap();
    let best = "apps/corral-app/src/model/session/mod.rs";
    let included = "apps/corral-app/src/model/session/mod.generated.rs";
    for path in [
        best,
        included,
        &format!("{nested}/sessmod.rs"),
        "blocked/sessmod.rs",
        ".corral/sessmod.rs",
        "sessmod.generated.rs",
    ] {
        fs::write(checkout.join(path), "").unwrap();
    }
    let external = repo.join("sessmod-outside.rs");
    fs::write(&external, "private").unwrap();
    symlink(&external, checkout.join("sessmod-symlink.rs")).unwrap();
    symlink(&repo, checkout.join("symlink-dir")).unwrap();
    fs::hard_link(&external, checkout.join("sessmod-hardlink.rs")).unwrap();

    let shared = fixture.shared();
    let caps = vec![app::SESSION_FILE_SEARCH_V1.into()];
    let welcome = dispatch::welcome(
        shared,
        wire::Hello {
            proto: wire::Proto {
                major: wire::PROTO_MAJOR,
                minor: wire::PROTO_MINOR,
            },
            caps: caps.clone(),
            client: wire::ClientInfo {
                kind: "test".into(),
                version: "1".into(),
            },
        },
    );
    assert_eq!(welcome.caps, caps);
    let call = |peer, id: &str, query: &str, limit| {
        let mut capture = Capture::default();
        dispatch::serve(
            &mut capture,
            &peer,
            shared,
            rpc::Request {
                jsonrpc: rpc::JSONRPC.into(),
                id: json!(1),
                method: app::DETAIL_GET.into(),
                params: serde_json::to_value(DetailGet {
                    target: DetailRef::SessionFileSearch {
                        id: id.into(),
                        query: query.into(),
                        limit,
                    },
                    etag: None,
                    page: None,
                })
                .unwrap(),
            },
            None,
            &mut Subscriptions::new(shared),
        )
        .unwrap();
        let frame = capture.0.unwrap();
        assert_eq!(frame.kind, FrameKind::Control);
        assert!(frame.payload.len() <= CONTROL_MAX.min(app::APP_CONTROL_MAX));
        let response: rpc::Response<DetailReply> = serde_json::from_slice(&frame.payload).unwrap();
        match response.outcome {
            rpc::ResponseOutcome::Result { result } => {
                assert!(result.next.is_none());
                let Some(DetailBody::SessionFileSearch(body)) = result.body else {
                    panic!("expected file search detail");
                };
                Ok(body)
            }
            rpc::ResponseOutcome::Error { error } => Err(error.code),
        }
    };
    let capped = call(Peer::Unix, "B-1", "SeSsMoD", 1).unwrap();
    assert_eq!(capped.paths, [best]);
    assert!(capped.truncated);
    let full = call(Peer::Unix, "B-1", "sessmod", SESSION_FILE_SEARCH_LIMIT_MAX).unwrap();
    assert_eq!(full.paths, [best, included]);
    assert!(!full.truncated);
    assert_eq!(
        call(
            Peer::Tcp("127.0.0.1:1".parse().unwrap()),
            "B-1",
            "sessmod",
            1
        )
        .unwrap_err(),
        rpc::ErrorCode::Unauthenticated
    );
    assert_eq!(
        call(Peer::Unix, "B-2", "sessmod", 1).unwrap_err(),
        rpc::ErrorCode::NotFound
    );
    assert_eq!(
        call(Peer::Unix, "B-1/child/unavailable", "sessmod", 1).unwrap_err(),
        rpc::ErrorCode::InvalidParams
    );
    for (query, limit) in [
        ("", 1),
        ("s\n", 1),
        (&"é".repeat(65), 1),
        ("sessmod", 0),
        ("sessmod", SESSION_FILE_SEARCH_LIMIT_MAX + 1),
    ] {
        assert_eq!(
            call(Peer::Unix, "B-1", query, limit).unwrap_err(),
            rpc::ErrorCode::InvalidParams
        );
    }
}
