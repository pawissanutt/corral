use std::{
    io::{Read, Write},
    sync::{atomic::Ordering, mpsc},
    thread::{self, JoinHandle},
    time::Instant,
};

use corral_protocol::{
    app::{DETAIL_GET, SYSTEM_ENTRIES_V1, SYSTEM_RUNTIME_V1, SYSTEM_V1},
    detail::{DetailBody, DetailGet, DetailRef, DetailReply},
};
use corral_types::{
    RuntimeEdgeKind, SystemAt, SystemImpactCategory, SystemMapRow, SystemReview, SystemReviewRow,
    SystemRow, SystemRuntimeRow, SystemStatus,
};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::{CWD, Mode, mkfifoat},
};

use super::*;

mod lenses;

const WAIT: Duration = Duration::from_secs(8);
const CHANGED: &str = "app/src/feature/mod.rs";
const HELPER: &str = "app/src/bin/demo-helper.rs";
const SCRIPTS: usize = 100;
const EXTENSION: &str = "fixture";

#[test]
fn socket_system_map_and_review_refresh_from_background_facts() {
    let fixture = Fixture::with_repository();
    let repo = fixture.root().join("repo");
    fs::create_dir_all(repo.join("app/src/feature")).unwrap();
    fs::write(
        repo.join("app/Cargo.toml"),
        "[package]\nname = 'app'\nversion = '0.1.0'\n",
    )
    .unwrap();
    fs::write(repo.join("app/src/lib.rs"), "pub mod feature;\n").unwrap();
    fs::write(
        repo.join(CHANGED),
        "pub fn value() -> u8 { let _ = std::process::Command::new(\"git\"); 1 }\n",
    )
    .unwrap();
    // Small rows fill one page without requiring hundreds of source files.
    let scripts = (0..SCRIPTS)
        .map(|n| (format!("script-{n}"), json!("echo example")))
        .collect::<serde_json::Map<_, _>>();
    fs::write(
        repo.join("package.json"),
        json!({"name": "runtime-fixture", "scripts": scripts}).to_string(),
    )
    .unwrap();
    git(&repo, &["add", "app", "package.json"]);
    git(&repo, &["commit", "-m", "add example package"]);
    let mut extension = BlockedDelta::configure(fixture.root());
    let engine = fixture.shared().engine().unwrap();
    let task = engine
        .apply(
            Op::New {
                task: NewTask {
                    title: "update the example module".into(),
                    project: Some("demo".into()),
                    ..NewTask::default()
                },
            },
            false,
        )
        .unwrap()
        .task
        .unwrap()
        .id;
    let run = engine.apply(dispatch(&task), false).unwrap().run.unwrap();
    fs::create_dir_all(run.worktree.join("app/src/bin")).unwrap();
    fs::write(run.worktree.join(HELPER), "fn main() {}\n").unwrap();
    fs::write(
        run.worktree.join(CHANGED),
        r#"pub fn value() -> u8 {
    let _ = std::process::Command::new("git");
    let _ = std::process::Command::new("git");
    let _ = std::process::Command::new("demo-helper");
    2
}
"#,
    )
    .unwrap();
    let submitted = engine
        .apply(
            Op::Done {
                id: task.clone(),
                summary: "update the example value".into(),
            },
            false,
        )
        .unwrap()
        .run
        .unwrap();
    assert_eq!(
        engine.store.require_task(&task).unwrap().status,
        Status::Review
    );
    let candidate_head = submitted.head_sha.as_deref().unwrap();
    assert_ne!(candidate_head, submitted.base_sha);

    // Like the daemon's startup, start the real facts worker and watch a separate
    // store connection for external changes. No calls to refresh the facts inline.
    let _poll = PollGuard::start(fixture.shared());
    let path = fixture.unix_server();
    let map = DetailGet {
        target: DetailRef::SystemMap {
            project: "demo".into(),
            at: None,
        },
        etag: None,
        page: None,
    };
    let runtime = DetailGet {
        target: DetailRef::SystemRuntime {
            project: "demo".into(),
        },
        etag: None,
        page: None,
    };
    let review = DetailGet {
        target: DetailRef::SystemReview {
            task: task.clone(),
            at: None,
        },
        etag: None,
        page: None,
    };
    let sub = SubOpen {
        topic: Topic::System,
        since: None,
        scope: None,
    };
    let (mut legacy, welcome) =
        connect(&path, vec![wire::TASKS_V1.into(), SYSTEM_ENTRIES_V1.into()]);
    assert!(!welcome.caps.iter().any(|cap| cap == SYSTEM_V1));
    assert!(!welcome.caps.iter().any(|cap| cap == SYSTEM_ENTRIES_V1));
    for detail in [&map, &runtime, &review] {
        let error = legacy.call(DETAIL_GET, detail).unwrap_err();
        assert_eq!(error.code, rpc::ErrorCode::InvalidParams, "{error:?}");
    }
    let error = legacy.call(wire::SUB_OPEN, &sub).unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::InvalidParams, "{error:?}");

    let mut caps = vec![wire::TASKS_V1.into(), SYSTEM_V1.into()];
    let (mut previous_system_client, welcome) = connect(&path, caps.clone());
    assert!(welcome.caps.iter().any(|cap| cap == SYSTEM_V1));
    assert!(!welcome.caps.iter().any(|cap| cap == SYSTEM_RUNTIME_V1));
    caps.extend([SYSTEM_RUNTIME_V1.into(), SYSTEM_ENTRIES_V1.into()]);
    let (mut client, welcome) = connect(&path, caps.clone());
    assert!(welcome.caps.iter().any(|cap| cap == SYSTEM_RUNTIME_V1));
    assert!(welcome.caps.iter().any(|cap| cap == SYSTEM_ENTRIES_V1));
    // A separate connection for the topic avoids unsolicited deltas interleaving
    // with detail replies (the lightweight test Client only reads RPC responses).
    let mut topic = Client::connect_with(&path, caps);
    topic.ok::<()>(wire::SUB_OPEN, sub);
    let initial = next_system(&mut topic, |row| {
        row.status == SystemStatus::Ready
            && row.head.as_deref() == Some(submitted.base_sha.as_str())
            && row.snapshot_provided.as_deref() == Some(&[])
    });
    assert_eq!(initial.extension.as_deref(), Some(EXTENSION));
    assert_eq!(initial.last_extension_error, None);
    assert_eq!(initial.review_provided, None);
    let root = client.ok::<DetailReply>(DETAIL_GET, &map);
    let Some(DetailBody::SystemMap(root)) = root.body else {
        panic!("expected a system map: {root:?}");
    };
    assert!(root.rows.iter().any(|row| matches!(row,
        SystemMapRow::Container { container, .. } if container.path == "app"
    )));
    let package = client.ok::<DetailReply>(
        DETAIL_GET,
        DetailGet {
            target: DetailRef::SystemMap {
                project: "demo".into(),
                at: Some(SystemAt::Container { path: "app".into() }),
            },
            ..map.clone()
        },
    );
    let Some(DetailBody::SystemMap(package)) = package.body else {
        panic!("expected the package drill-in: {package:?}");
    };
    assert!(package.rows.iter().any(|row| matches!(row,
        SystemMapRow::Module { module, .. } if module.container == "app" && module.path == "feature"
    )));

    // Request the delta and wait for the provider to reach the barrier. It is
    // impossible for enriched rows to arrive before the provider is released.
    let _: DetailReply = client.ok(DETAIL_GET, &review);
    extension.wait_until_entered();
    let before = client.ok::<DetailReply>(DETAIL_GET, &review);
    let Some(DetailBody::SystemReview(built_in)) = before.body else {
        panic!("expected built-in review while the provider is blocked: {before:?}");
    };
    assert_eq!(built_in.head, candidate_head);
    assert_eq!(built_in.base, submitted.base_sha);
    assert!(!built_in.base_estimated);
    assert!(built_in.provider.is_none(), "{built_in:?}");
    assert!(built_in.categories.iter().any(|category| category.category
        == SystemImpactCategory::Definitions
        && !category.provided));
    let changed_files = |rows: &[SystemReviewRow]| {
        rows.iter()
            .filter_map(|row| match row {
                SystemReviewRow::File { path, .. } => Some(path.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(changed_files(&built_in.rows), [HELPER, CHANGED]);
    assert_runtime_review(&built_in);
    assert!(
        built_in
            .categories
            .iter()
            .any(|entry| entry.category == SystemImpactCategory::EntryPoints)
    );
    let previous = previous_system_client.ok::<DetailReply>(DETAIL_GET, &review);
    assert_ne!(previous.etag, before.etag);
    let Some(DetailBody::SystemReview(previous)) = previous.body else {
        panic!("expected legacy System review: {previous:?}");
    };
    assert!(previous.categories.iter().all(|entry| !matches!(
        entry.category,
        SystemImpactCategory::Runtime | SystemImpactCategory::EntryPoints
    )));
    assert!(!previous.rows.iter().any(|row| matches!(
        row,
        SystemReviewRow::RuntimeUnit { .. } | SystemReviewRow::RuntimeEdge { .. }
    )));
    assert_eq!(changed_files(&previous.rows), [HELPER, CHANGED]);
    // Approval can persist a different run base before it rewrites the head.
    // The already-submitted change and etag must be independent of that update.
    let mut rebasing = submitted.clone();
    rebasing.base_sha = candidate_head.to_owned();
    engine.store.update_run(&rebasing).unwrap();
    let unchanged = client.ok::<DetailReply>(
        DETAIL_GET,
        DetailGet {
            etag: Some(before.etag.clone()),
            ..review.clone()
        },
    );
    engine.store.update_run(&submitted).unwrap();
    assert!(unchanged.body.is_none(), "{unchanged:?}");
    let _: wire::Pong = client.ok(wire::PING, wire::Ping {});
    let other = client.new_task("socket mutation while the provider is blocked", Vec::new());
    assert_eq!(
        engine.store.require_task(&other).unwrap().title,
        "socket mutation while the provider is blocked"
    );

    // Fix the head's snapshot reply before Review can cache it. Its metadata
    // is valid but unservable when the landed map later presents that snapshot.
    let long_reason = "x".repeat(corral_protocol::app::APP_CONTROL_MAX);
    fs::write(
        fixture.root().join("overflow-facts"),
        json!({
            "type": "done", "provided": ["files"], "skipped": {}, "omitted": { (long_reason): 1 }
        })
        .to_string(),
    )
    .unwrap();
    // Clear the notification for the built-in delta before releasing enrichment;
    // the next revision is a wakeup to revalidate the same selector and etag.
    let built_in_revision = next_system(&mut topic, |row| row.revision > initial.revision).revision;
    extension.release();
    let revalidate = DetailGet {
        etag: Some(before.etag.clone()),
        ..review
    };
    let mut after = None;
    let enriched = next_system(&mut topic, |row| {
        if row.revision <= built_in_revision {
            return false;
        }
        let reply = client.ok::<DetailReply>(DETAIL_GET, &revalidate);
        let Some(DetailBody::SystemReview(body)) = reply.body else {
            return false;
        };
        if body.provider.is_some() {
            after = Some((reply.etag, body));
            return true;
        }
        false
    });
    assert_eq!(enriched.snapshot_provided, Some(vec![]));
    assert_eq!(enriched.review_provided, Some(vec!["changes".into()]));
    let (etag, after) = after.unwrap();
    assert_ne!(etag, before.etag);
    assert_eq!(after.task, task);
    assert_eq!(after.head, built_in.head);
    assert_eq!(after.base, built_in.base);
    assert_eq!(changed_files(&after.rows), [HELPER, CHANGED]);
    assert_runtime_review(&after);
    let provider = after.provider.as_ref().unwrap();
    assert_eq!(provider.extension, EXTENSION);
    assert_eq!(provider.error, None);
    assert!(after.categories.iter().any(|category| category.category
        == SystemImpactCategory::Definitions
        && category.provided
        && category.total == 1));
    assert!(after.rows.iter().any(|row| matches!(row,
        SystemReviewRow::Definition { path, name, .. } if path == CHANGED && name == "feature::value"
    )));

    // A separate Engine connection lands the change; the system subscription has
    // no direct operation callback or detail read to update the project's head.
    engine
        .apply(
            Op::Approve {
                id: task.clone(),
                onto: None,
            },
            false,
        )
        .unwrap();
    let event = engine
        .store
        .list_events(&task)
        .unwrap()
        .into_iter()
        .find(|event| event.kind == EventKind::Landed)
        .expect("approve should emit a landed event");
    assert_eq!(event.detail["sha"], candidate_head);
    let landed = next_system(&mut topic, |row| {
        row.status == SystemStatus::Ready
            && row.head.as_deref() == Some(candidate_head)
            && row.last_extension_error.is_some()
    });
    assert!(landed.revision > enriched.revision);
    assert_eq!(landed.snapshot_provided, None);
    assert_eq!(landed.review_provided, Some(vec!["changes".into()]));
    let current = client.ok::<DetailReply>(DETAIL_GET, &map);
    let Some(DetailBody::SystemMap(current)) = current.body else {
        panic!("expected built-in map after provider failure: {current:?}");
    };
    assert_eq!(current.head.as_deref(), Some(candidate_head));
    let processes: Vec<_> = current
        .rows
        .iter()
        .filter_map(|row| match row {
            SystemMapRow::Spawn { container, spawn } => {
                Some((container.as_str(), spawn.process.as_str(), spawn.count))
            }
            _ => None,
        })
        .collect();
    assert_eq!(processes, [("app", "git", 2)]);
    assert!(
        current
            .provider
            .is_some_and(|provider| provider.error.is_some())
    );
    assert_runtime_pages(
        &mut previous_system_client,
        &mut client,
        runtime,
        map,
        candidate_head,
    );
}

fn assert_runtime_review(review: &SystemReview) {
    for (category, total) in [
        (SystemImpactCategory::ExternalProcesses, 1),
        (SystemImpactCategory::Runtime, 2),
    ] {
        assert!(
            review
                .categories
                .iter()
                .any(|entry| entry.category == category && entry.provided && entry.total == total)
        );
    }
    assert!(review.rows.iter().any(|row| matches!(row,
        SystemReviewRow::Spawn { path, process, before: 1, after: 2 }
            if path == CHANGED && process == "git"
    )));
    assert!(review.rows.iter().any(|row| matches!(row,
        SystemReviewRow::RuntimeUnit { before: None, after: Some(unit) } if unit.id.path == HELPER
    )));
    assert!(review.rows.iter().any(|row| matches!(row,
        SystemReviewRow::RuntimeEdge { before: None, after: Some(edge) }
            if edge.kind == RuntimeEdgeKind::Starts && edge.to.path == HELPER
    )));
}

fn assert_runtime_pages(
    client: &mut Client,
    opted_in: &mut Client,
    query: DetailGet,
    map: DetailGet,
    head: &str,
) {
    let first = client.ok::<DetailReply>(DETAIL_GET, &query);
    let Some(DetailBody::SystemRuntime(first_body)) = first.body else {
        panic!("expected a runtime detail: {first:?}");
    };
    assert_eq!(first_body.head.as_deref(), Some(head));
    assert_eq!(first_body.status, SystemStatus::Ready);
    assert_eq!(first_body.error, None);
    assert_eq!(first_body.omitted_rows, 0);
    let token = first.next.expect("expected runtime continuation");
    let error = opted_in
        .call(
            DETAIL_GET,
            DetailGet {
                page: Some(token.clone()),
                ..query.clone()
            },
        )
        .unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::InvalidParams);
    let expanded = opted_in.ok::<DetailReply>(DETAIL_GET, &query);
    assert_ne!(expanded.etag, first.etag);
    let error = client
        .call(
            DETAIL_GET,
            DetailGet {
                page: Some(token.clone()),
                ..map
            },
        )
        .unwrap_err();
    assert_eq!(error.code, rpc::ErrorCode::InvalidParams);
    let unchanged = client.ok::<DetailReply>(
        DETAIL_GET,
        DetailGet {
            etag: Some(first.etag.clone()),
            ..query.clone()
        },
    );
    assert!(unchanged.body.is_none());
    let last = client.ok::<DetailReply>(
        DETAIL_GET,
        DetailGet {
            page: Some(token),
            ..query
        },
    );
    assert_eq!(last.etag, first.etag);
    assert!(last.next.is_none());
    let Some(DetailBody::SystemRuntime(last_body)) = last.body else {
        panic!("expected a runtime continuation: {last:?}");
    };
    assert_eq!(last_body.head, first_body.head);
    let rows: Vec<_> = first_body.rows.into_iter().chain(last_body.rows).collect();
    assert_eq!(rows.len(), SCRIPTS + 2);
    assert!(rows.iter().any(|row| matches!(row,
        SystemRuntimeRow::Unit { unit } if unit.id.path == HELPER
    )));
    assert!(matches!(rows.last(), Some(SystemRuntimeRow::Edge { edge })
        if edge.kind == RuntimeEdgeKind::Starts && edge.to.path == HELPER));
}

fn connect(path: &Path, caps: Vec<String>) -> (Client, wire::Welcome) {
    let stream = UnixStream::connect(path).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut client = Client(stream);
    let welcome = client.ok(
        wire::HELLO,
        wire::Hello {
            proto: wire::Proto {
                major: wire::PROTO_MAJOR,
                minor: wire::PROTO_MINOR,
            },
            caps,
            client: wire::ClientInfo {
                kind: "cli".into(),
                version: "test".into(),
            },
        },
    );
    (client, welcome)
}

fn next_system(topic: &mut Client, mut matches: impl FnMut(&SystemRow) -> bool) -> SystemRow {
    let deadline = Instant::now() + WAIT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for a system row");
        topic.0.set_read_timeout(Some(remaining)).unwrap();
        let SubDelta::System { delta } = read_delta(topic) else {
            panic!("expected a system topic delta");
        };
        topic.ok::<()>(
            wire::SUB_ACK,
            SubAck {
                topic: Topic::System,
                scope: None,
                cursor: delta.cursor,
            },
        );
        if let Some(row) = delta
            .rows
            .into_iter()
            .find(|row| row.project == "demo" && matches(row))
        {
            return row;
        }
    }
}

struct PollGuard {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl PollGuard {
    fn start(shared: &Arc<Shared>) -> Self {
        shared.refresh_config().unwrap();
        crate::system::start(shared).unwrap();
        let (ready, wait) = mpsc::channel();
        let observer = Arc::clone(shared);
        let thread = thread::spawn(move || {
            let engine = observer.engine().unwrap();
            let seq = engine.store.latest_seq().unwrap();
            let version = engine.store.data_version().unwrap();
            observer.latest_seq.store(seq, Ordering::Release);
            ready.send(()).unwrap();
            crate::poll_loop(&observer, &engine.store, seq, version);
        });
        let guard = Self {
            shared: Arc::clone(shared),
            thread: Some(thread),
        };
        wait.recv_timeout(WAIT).unwrap();
        guard
    }
}

impl Drop for PollGuard {
    fn drop(&mut self) {
        self.shared.stop();
        let result = self.thread.take().unwrap().join();
        self.shared.system.wait_stopped(WAIT);
        if !thread::panicking() {
            result.unwrap();
        }
    }
}

struct BlockedDelta {
    ready: fs::File,
    release: fs::File,
    cancel: PathBuf,
}

impl BlockedDelta {
    fn configure(root: &Path) -> Self {
        fn fifo(path: &Path) -> fs::File {
            mkfifoat(CWD, path, Mode::RUSR | Mode::WUSR).unwrap();
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .unwrap()
        }
        let config = root.join(".corral-next/corral.toml");
        let mut toml = fs::OpenOptions::new().append(true).open(config).unwrap();
        writeln!(toml, "\n[extensions.{EXTENSION}]\nfacts = 'sh ../facts.sh'").unwrap();
        fs::write(
            root.join("facts.sh"),
            r#"set -eu
case "$(cat)" in
    *'"query":"delta"'*)
        exec 3<>../facts-release 4<>../facts-ready
        printf '1' >&4
        if [ ! -f ../facts-cancel ]; then read -r signal <&3; fi
        cat <<'ROWS'
{"type":"change","change":"changed","fact":"definition","path":"app/src/feature/mod.rs","name":"feature::value","line":1}
{"type":"done","provided":["changes"],"skipped":{},"omitted":{}}
ROWS
        ;;
    *)
        if [ -f ../overflow-facts ]; then
            cat ../overflow-facts
        else
            printf '%s\n' '{"type":"done","provided":[],"skipped":{},"omitted":{}}'
        fi
        ;;
esac
"#,
        )
        .unwrap();
        Self {
            ready: fifo(&root.join("facts-ready")),
            release: fifo(&root.join("facts-release")),
            cancel: root.join("facts-cancel"),
        }
    }

    fn wait_until_entered(&mut self) {
        let mut ready = [PollFd::new(&self.ready, PollFlags::IN)];
        let limit = Timespec {
            tv_sec: WAIT.as_secs().try_into().unwrap(),
            tv_nsec: 0,
        };
        assert_eq!(
            poll(&mut ready, Some(&limit)).unwrap(),
            1,
            "delta provider did not reach the barrier"
        );
        let mut byte = [0];
        self.ready.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"1");
    }

    fn release(&mut self) {
        self.release.write_all(b"go\n").unwrap();
    }
}

impl Drop for BlockedDelta {
    fn drop(&mut self) {
        // Release an already blocked child, and let a child opening the FIFO
        // concurrently with unwind skip blocking if our descriptor is gone.
        let _ = fs::File::create(&self.cancel);
        let _ = self.release.write_all(b"go\n");
    }
}
