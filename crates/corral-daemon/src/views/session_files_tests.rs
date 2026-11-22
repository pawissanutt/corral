use super::*;
use std::{fs, os::unix::fs::symlink};

#[test]
fn browse_untracked_without_following_links_or_listing_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let worktree = tmp.path().join("checkout");
    fs::create_dir(&worktree).unwrap();
    fs::write(worktree.join("notes.md"), "notes").unwrap();
    fs::write(worktree.join(".git"), "gitdir: elsewhere").unwrap();
    let external = tmp.path().join("outside");
    fs::write(&external, "private").unwrap();
    symlink(&external, worktree.join("symlink")).unwrap();
    symlink(tmp.path(), worktree.join("symlink-dir")).unwrap();
    fs::hard_link(&external, worktree.join("hardlink")).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        worktree.join("fifo"),
        FileType::Fifo,
        Mode::RUSR,
        0,
    )
    .unwrap();
    let fd = root(&worktree).unwrap();
    let listed = list(&fd, "", "").unwrap();
    let filtered = list(&fd, "", "NOTES").unwrap();
    assert_eq!(filtered.entries[0].name, "notes.md");
    assert_eq!(filtered.entries.len(), 1);
    let names: Vec<_> = listed.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["hardlink", "notes.md"]);
    assert_eq!(preview(&fd, "notes.md").unwrap().kind, Kind::Markdown);
    for path in [
        "../outside",
        "hardlink",
        "symlink",
        "symlink-dir/outside",
        "fifo",
    ] {
        assert!(preview(&fd, path).is_err(), "{path}");
    }
    assert!(root(&worktree.join("symlink-dir")).is_err());
    for name in [".git", "..", ".corral-next", "a\nb", "a/b"] {
        assert!(!visible(name));
    }
}

#[test]
fn sparse_sixty_four_mib_html_stays_within_control_frames() {
    use crate::{
        app::dispatch,
        conn::{Conn, Peer},
        tests::Fixture,
        topics::Subscriptions,
    };
    use corral_protocol::{
        self as wire, app,
        frame::{CONTROL_MAX, Frame, FrameKind},
        rpc,
    };
    use serde_json::json;

    #[derive(Default)]
    struct Capture(Option<Frame>);
    impl Conn for Capture {
        fn send_frame(&mut self, frame: &Frame) -> Result<()> {
            assert!(self.0.replace(frame.clone()).is_none());
            Ok(())
        }
    }

    let fixture = Fixture::new();
    let repo = fixture.root().canonicalize().unwrap();
    let worktree = repo.join(".worktrees/build-walk");
    let record = repo.join("pi-data/build/B-1");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&record).unwrap();
    fs::write(
        record.join("session.json"),
        json!({
            "id": "B-1", "name": "walk", "workspaceRoot": repo, "repoRoot": repo,
            "worktree": worktree, "here": false,
        })
        .to_string(),
    )
    .unwrap();
    let file = File::create(worktree.join("prototype.HTML")).unwrap();
    file.set_len(SESSION_HTML_MAX_BYTES).unwrap();
    let last = SESSION_HTML_MAX_BYTES - SESSION_HTML_CHUNK_BYTES as u64;
    let mut bytes = vec![b'a'; SESSION_HTML_CHUNK_BYTES];
    file.write_all_at(&bytes, 0).unwrap();
    bytes.fill(b'z');
    file.write_all_at(&bytes, last).unwrap();

    let shared = fixture.shared();
    let caps = vec![app::SESSION_FILES_V1.into(), app::SESSION_HTML_V1.into()];
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
    let call = |peer: Peer, target: DetailRef| -> Result<DetailReply, rpc::Error> {
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
                    target,
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
            rpc::ResponseOutcome::Result { result } => Ok(result),
            rpc::ResponseOutcome::Error { error } => Err(error),
        }
    };
    let path = "prototype.HTML";
    let select = |offset, revision| DetailRef::SessionFileChunk {
        id: "B-1".into(),
        path: path.into(),
        offset,
        revision,
    };
    assert_eq!(
        call(Peer::Tcp("127.0.0.1:1".parse().unwrap()), select(0, None))
            .unwrap_err()
            .code,
        rpc::ErrorCode::Unauthenticated
    );
    let old = call(
        Peer::Unix,
        DetailRef::SessionFile {
            id: "B-1".into(),
            path: path.into(),
        },
    )
    .unwrap();
    let Some(DetailBody::SessionFile(old)) = old.body else {
        panic!("expected preview")
    };
    assert_eq!(old.kind, Kind::TooLarge);
    assert!(old.data.is_empty());
    assert_eq!(old.bytes, SESSION_HTML_MAX_BYTES);

    let first = call(Peer::Unix, select(0, None)).unwrap();
    assert!(first.next.is_none());
    let Some(DetailBody::SessionFileChunk(first)) = first.body else {
        panic!("expected chunk")
    };
    assert_eq!(first.offset, 0);
    assert_eq!(first.size, SESSION_HTML_MAX_BYTES);
    assert_eq!(
        STANDARD.decode(first.data).unwrap(),
        vec![b'a'; SESSION_HTML_CHUNK_BYTES]
    );
    let tail = call(Peer::Unix, select(last, Some(first.revision.clone()))).unwrap();
    let Some(DetailBody::SessionFileChunk(tail)) = tail.body else {
        panic!("expected chunk")
    };
    assert_eq!(tail.offset, last);
    assert_eq!(tail.size, first.size);
    assert_eq!(tail.revision, first.revision);
    assert_eq!(STANDARD.decode(tail.data).unwrap(), bytes);
    assert_eq!(
        call(Peer::Unix, select(last, None)).unwrap_err().code,
        rpc::ErrorCode::InvalidParams
    );
    assert_eq!(
        call(Peer::Unix, select(1, Some(first.revision.clone())))
            .unwrap_err()
            .code,
        rpc::ErrorCode::InvalidParams
    );

    file.set_len(SESSION_HTML_MAX_BYTES + 1).unwrap();
    assert_eq!(
        call(Peer::Unix, select(last, Some(first.revision)))
            .unwrap_err()
            .code,
        rpc::ErrorCode::Conflict
    );
    assert_eq!(
        call(Peer::Unix, select(0, None)).unwrap_err().code,
        rpc::ErrorCode::TooLarge
    );
}

#[test]
fn chunk_reads_keep_the_same_path_confinement() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = tmp.path().join("checkout");
    fs::create_dir(&checkout).unwrap();
    let external = tmp.path().join("external.html");
    fs::write(&external, "private").unwrap();
    fs::hard_link(&external, checkout.join("hard.html")).unwrap();
    symlink(&external, checkout.join("link.html")).unwrap();
    symlink(tmp.path(), checkout.join("link-dir")).unwrap();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        checkout.join("fifo.html"),
        FileType::Fifo,
        Mode::RUSR,
        0,
    )
    .unwrap();
    fs::write(checkout.join("binary.html"), b"\0not HTML").unwrap();
    let fd = root(&checkout).unwrap();
    for path in ["../external.html", ".git/config.html", "notes.md", ""] {
        assert!(ChunkRequest::new("B-1", path, 0, None).is_err(), "{path}");
    }
    for path in [
        "hard.html",
        "link.html",
        "link-dir/external.html",
        "fifo.html",
        "binary.html",
    ] {
        assert!(
            ChunkRequest::new("B-1", path, 0, None)
                .unwrap()
                .read(&fd)
                .is_err(),
            "{path}"
        );
    }
}

#[test]
fn chunk_revisions_reject_same_length_replacement_and_wrong_selector() {
    let tmp = tempfile::tempdir().unwrap();
    let fd = root(tmp.path()).unwrap();
    let path = "prototype.htm";
    fs::write(tmp.path().join(path), b"<h1>one</h1>").unwrap();
    fs::write(tmp.path().join("replacement.htm"), b"<h1>two</h1>").unwrap();
    let first = ChunkRequest::new("B-1", path, 0, None)
        .unwrap()
        .read(&fd)
        .unwrap();
    let read = |id, path| {
        ChunkRequest::new(id, path, 0, Some(&first.revision))
            .unwrap()
            .read(&fd)
    };
    assert!(read("B-2", path).unwrap_err().is::<StaleWorktreeFile>());
    assert!(
        read("B-1", "replacement.htm")
            .unwrap_err()
            .is::<StaleWorktreeFile>()
    );
    fs::rename(tmp.path().join("replacement.htm"), tmp.path().join(path)).unwrap();
    assert!(read("B-1", path).unwrap_err().is::<StaleWorktreeFile>());
    fs::remove_file(tmp.path().join(path)).unwrap();
    assert!(read("B-1", path).unwrap_err().is::<StaleWorktreeFile>());
}

#[test]
fn opaque_binary_and_oversize_content_never_get_payloads() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("big.html"), vec![b'a'; BYTES + 1]).unwrap();
    fs::write(tmp.path().join("opaque.png"), b"\0not png").unwrap();
    let fd = root(tmp.path()).unwrap();
    for (path, kind) in [("big.html", Kind::TooLarge), ("opaque.png", Kind::Binary)] {
        let served = preview(&fd, path).unwrap();
        assert_eq!(served.kind, kind);
        assert!(served.data.is_empty());
    }
}
