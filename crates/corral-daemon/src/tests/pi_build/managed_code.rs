use std::os::unix::fs::PermissionsExt;

use super::*;
use corral_protocol::build::{BuildMode, CODE_RESUME, CODE_START};

fn save_code(dir: &Path, workspace: &Path, socket: &Path) {
    save_record(dir, workspace, socket, true);
    let path = dir.join("session.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["trustedCode"] = json!(true);
    record["worktree"] = json!(workspace.join("repo"));
    fs::write(path, record.to_string()).unwrap();
}

#[test]
fn code_start_uses_the_checkout_and_returns_code_mode() {
    let (fixture, _) = launch_fixture_mode("code", |script| script);
    let mut owner = Client::connect_with(&fixture.unix_server(), vec![wire::BUILD_V1.into()]);
    let row: AssistantRow = owner.ok(
        CODE_START,
        json!({"project":"demo", "name":"walk", "model":MODEL}),
    );
    let build = row.build.unwrap();
    assert_eq!(build.mode, BuildMode::Code);
    assert!(build.here);
    assert_eq!(Path::new(&build.worktree), fixture.root().join("repo"));
    assert!(!fixture.root().join("repo/.worktrees").exists());
    let refused = owner
        .call(SESSION_TERMINATE, json!({"id":row.id}))
        .unwrap_err();
    assert_eq!(refused.code, rpc::ErrorCode::Refused);
}

#[test]
fn code_resume_uses_code_argv_not_managed_build_relaunch() {
    let (fixture, _) = launch_fixture_mode("code", |script| script);
    let id = "B-coldcode";
    let data = dir(&fixture, id);
    fs::create_dir_all(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    save_code(
        &data,
        fixture.root(),
        &fixture.root().join("cold-code.sock"),
    );
    let mut owner = Client::connect_with(&fixture.unix_server(), vec![wire::BUILD_V1.into()]);
    let refused = owner.call(SESSION_RELAUNCH, json!({"id":id})).unwrap_err();
    assert_eq!(refused.code, rpc::ErrorCode::Refused);
    let resumed: SessionRelaunched = owner.ok(CODE_RESUME, json!({"id":id}));
    assert_eq!(resumed.id, id);
    assert_eq!(page(&mut owner, id).host, HostPresence::Running);
}
#[test]
fn managed_exit_refuses_code_without_contacting_its_host() {
    use std::{io::ErrorKind, os::unix::net::UnixListener};

    use corral_protocol::session_host::{SessionAction, SessionActionRequest, SessionCardAnswer};

    let fixture = Fixture::new();
    let id = "B-exitcode";
    let data = dir(&fixture, id);
    fs::create_dir_all(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    let socket = fixture.root().join("exit-code.sock");
    save_code(&data, fixture.root(), &socket);
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let shared = fixture.shared();
    let answer = |card: &str| SessionCardAnswer {
        id: id.into(),
        card: card.into(),
        expected_model: None,
        answers: vec![],
        dismiss: true,
    };
    let open = crate::sessions::forward::action(
        shared,
        SessionActionRequest {
            id: id.into(),
            action: SessionAction::ExitOpen,
        },
    );
    let managed = crate::sessions::forward::card_answer(
        shared,
        "local",
        answer("exit-12345678-1234-abcd-1234-123456789abc"),
    );
    assert!(open.unwrap_err().is::<corral_types::Refusal>());
    assert!(managed.unwrap_err().is::<corral_types::Refusal>());
    assert_eq!(listener.accept().unwrap_err().kind(), ErrorKind::WouldBlock);
    drop(listener);
    fs::remove_file(&socket).unwrap();

    // The Pi host handshakes as `build`; the durable record identifies it as Code.
    let script = Script::default().reply("card_answer", |_, request, _| {
        assert_eq!(request["args"]["card"], "question-1");
        json!({"status": "delivered"})
    });
    let _host = Host::start(socket, id.into(), "build", &data, script);
    let reply =
        crate::sessions::forward::card_answer(shared, "local", answer("question-1")).unwrap();
    assert_eq!(reply.status, "delivered");
}

pub(super) fn launch_fixture_mode(
    mode: &'static str,
    on_resume: impl Fn(Script) -> Script + Send + Sync + 'static,
) -> (Fixture, Placement) {
    let placement = Placement::new(move |command, dir| {
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(command.get_program(), "corral-pi");
        assert_eq!(args[0], mode);
        if mode == "code" {
            assert!(!args.contains(&OsStr::new("--here")));
        }
        assert!(args.contains(&OsStr::new("--headless")));
        let workspace = command
            .get_envs()
            .find_map(|(key, value)| {
                (key == "CORRAL_NEXT_ROOT")
                    .then_some(value)
                    .flatten()
                    .map(Path::new)
            })
            .unwrap();
        let checkout = workspace.join("repo");
        assert_eq!(command.get_current_dir(), Some(checkout.as_path()));
        let data_root = workspace.join("pi-data");
        assert_eq!(argument(&args, "--data-dir"), Some(data_root.as_os_str()));
        assert!(Path::new(argument(&args, "--corral").unwrap()).is_absolute());
        let id = dir.file_name().unwrap();
        let resuming = dir.join("session.json").is_file();
        if resuming {
            assert_eq!(argument(&args, "--resume"), Some(id));
            assert!(argument(&args, "--id").is_none());
        } else {
            assert_eq!(argument(&args, "--id"), Some(id));
            assert_eq!(argument(&args, "--project"), Some(OsStr::new("demo")));
            assert_eq!(argument(&args, "--model"), Some(OsStr::new(MODEL)));
            assert!(argument(&args, "--resume").is_none());
        }
        let socket = PathBuf::from(argument(&args, "--socket").unwrap());
        if mode == "code" {
            save_code(dir, workspace, &socket);
        } else {
            save_record(dir, workspace, &socket, false);
        }
        fs::write(dir.join("journal.json"), json!({
            "title": "Journal title", "diff": {"commits": 1, "files": 2, "added": 3, "removed": 4},
        }).to_string()).unwrap();
        let mut script = Script::default();
        script.pages.push_back(json!({"last": 0, "next": 0, "events": [], "more": false, "state": {
            "mode": "headless", "turn": 0, "open": false, "paused": false, "queue_total": 0,
            "model": MODEL, "effort": "high", "usage": {"tokens": 0, "cost_partial": false,
                "context_tokens": 0, "context_window": 0},
            "children": {"live": 0, "waiting": 0, "total": 0}, "jobs": {"live": 0, "waiting": 0, "total": 0},
        }}));
        Some(Host::start(
            socket,
            id.to_str().unwrap().into(),
            "build",
            dir,
            if resuming { on_resume(script) } else { script },
        ))
    });
    let runner = placement.clone();
    let fixture = Fixture::with_runner(Arc::new(move || Box::new(runner.clone())));
    fs::create_dir(fixture.root().join("repo")).unwrap();
    fs::write(
        fixture.root().join(crate::STATE_DIR).join("corral.toml"),
        "[projects.demo]\npath = 'repo'\n",
    )
    .unwrap();
    (fixture, placement)
}
