#![cfg(unix)]

use std::{
    env,
    fs::{self, File},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use corral_engine::{
    Actor, CheckKind, CheckResult, Engine, NewTask, PipelineMode, Role, Run, SessionRef, Status,
    events::{EventFilter, EventKind},
    runner::pi::{Client, Hello},
    testing::FakeRunner,
};
use jiff::Timestamp;
use serde_json::{Value, json};

struct Host(Child);

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn bun(root: &Path) -> Command {
    let mut command = Command::new("bun");
    command
        .arg("--no-env-file")
        .current_dir(root)
        .env_clear()
        .env("PATH", env::var_os("PATH").unwrap_or_default())
        .env("HOME", root)
        .env("TMPDIR", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("GIT_CEILING_DIRECTORIES", root);
    command
}

fn wait_for_host(host: &mut Host, client: &Client<'_>, log: &Path) -> Hello {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match client.hello_with_timeout(Duration::from_millis(100)) {
            Ok(hello) => return hello,
            Err(error) => {
                let exit = host.0.try_wait().unwrap();
                assert!(
                    exit.is_none() && Instant::now() < deadline,
                    "Pi host not ready ({exit:?}): {error:#}\n{}",
                    fs::read_to_string(log).unwrap_or_default(),
                );
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

#[test]
#[ignore = "requires Bun and installed packages/corral-pi dependencies"]
fn sync_observes_idle_pi_with_oversized_diagnostics() {
    let scratch = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp") // Keep the socket path short.
        .unwrap();
    let root = scratch.path().canonicalize().unwrap();
    let mut engine = Engine::init(&root, Some(1)).unwrap();
    // Placement fallback reports Working, which could not set quiet_since.
    engine.runner = Box::new(FakeRunner::new());
    engine.pipeline = PipelineMode::Off;
    let mut task = engine
        .new_task(NewTask {
            title: "Read Pi lifecycle".into(),
            ..NewTask::default()
        })
        .unwrap();
    task.status = Status::Running;
    engine.store.update_task(&task).unwrap();

    let setup = bun(&root)
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../packages/corral-pi/test/support/fixture-install.ts"),
        )
        .arg(&root)
        .output()
        .expect("start Bun to create the offline Pi fixture");
    assert!(
        setup.status.success(),
        "Pi fixture setup failed: {}",
        String::from_utf8_lossy(&setup.stderr)
    );
    let assets = PathBuf::from(String::from_utf8(setup.stdout).unwrap().trim());

    let cwd = root.join("work");
    // Real, trusted project skills with short names but long paths. The old status
    // response included every { name, path } row, even when no model turn had run.
    let skill_dir = (0..4).fold(cwd.join(".agents/skills"), |dir, _| {
        dir.join("x".repeat(200))
    });
    let skills: Vec<_> = (0..84)
        .map(|index| {
            let name = format!("fixture-{index}");
            let path = skill_dir.join(&name).join("SKILL.md");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                &path,
                format!("---\nname: {name}\ndescription: Test skill.\n---\nOffline fixture.\n"),
            )
            .unwrap();
            json!({"name": name, "path": path})
        })
        .collect();

    let dir = engine.state_dir.join("runs").join(&task.id).join("R1");
    let socket = root.join("pi.sock");
    let log_path = root.join("host.log");
    let log = File::create(&log_path).unwrap();
    let mut host = Host(
        bun(&root)
            .arg(assets.join("src/cli.ts"))
            .args(["run", "--headless", "--dir"])
            .arg(&dir)
            .arg("--socket")
            .arg(&socket)
            .current_dir(&cwd)
            .env("CORRAL_NEXT_ROOT", &root)
            .env("CORRAL_TASK", &task.id)
            .env("CORRAL_RUN", "R1")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let client = Client::new(&socket);
    let hello = wait_for_host(&mut host, &client, &log_path);
    assert_eq!(hello.session_id, format!("{}/R1", task.id));
    let context: Value = client.request("context", json!({})).unwrap();
    let loaded = context["skills"].as_array().unwrap();
    assert!(skills.iter().all(|skill| loaded.contains(&skill["name"])));
    assert!(serde_json::to_vec(&skills).unwrap().len() > 64 * 1024);

    // Only seed the durable placement; reconcile through the same socket client as sync.
    let run = Run {
        id: "R1".into(),
        task: task.id.clone(),
        role: Role::Worker,
        profile: "pi".into(),
        harness: "pi".into(),
        model: None,
        effort: None,
        worktree: cwd,
        branch: "fixture".into(),
        base: "main".into(),
        base_sha: "unused".into(),
        session: Some(SessionRef::Pi {
            socket,
            inner: Box::new(SessionRef::Process {
                pid: hello.pid,
                dir: dir.clone(),
            }),
            dir,
        }),
        started: Timestamp::now(),
        ended: None,
        outcome: None,
        summary: None,
        head_sha: None,
        quiet_since: None,
    };
    engine.store.insert_run(&run).unwrap();
    let before = engine.store.latest_seq().unwrap();
    engine.sync().unwrap();

    let errors = engine
        .store
        .events_after(
            &EventFilter {
                kinds: vec![EventKind::Error],
                ..EventFilter::default()
            },
            before,
        )
        .unwrap();
    assert!(
        errors.is_empty(),
        "sync could not read Pi lifecycle: {errors:?}"
    );
    let observed = engine.store.get_run(&task.id, &run.id).unwrap().unwrap();
    assert!(
        observed.quiet_since.is_some(),
        "sync did not observe idle Pi: {observed:?}"
    );
    assert!(observed.ended.is_none());
    assert_eq!(
        engine.store.require_task(&task.id).unwrap().status,
        Status::Running
    );

    // Seed a submitted task without Git, then exercise the transactional send-back
    // path, which reads the same worker's lifecycle before issuing the input.
    task.status = Status::Review;
    engine.store.update_task(&task).unwrap();
    engine.actor = Actor::Reviewer;
    let notes = "Use the revised wording.";
    engine.verdict(&task.id, false, Some(notes)).unwrap();
    let check = engine
        .store
        .latest_check(&task.id, CheckKind::Review, "0")
        .unwrap()
        .unwrap();
    assert_eq!(check.result, CheckResult::Fail);
    assert_eq!(check.notes.as_deref(), Some(notes));
    let task = engine.store.require_task(&task.id).unwrap();
    assert_eq!((task.status, task.round), (Status::Running, 1));
    let runs = engine.store.list_runs(&task.id).unwrap();
    assert_eq!(runs.len(), 1, "should reuse the live worker");
    assert_eq!(runs[0].session, run.session);

    let deadline = Instant::now() + Duration::from_secs(5);
    while !fs::read_to_string(&hello.session_file)
        .unwrap_or_default()
        .contains(notes)
    {
        assert!(
            Instant::now() < deadline,
            "review notes did not reach the worker transcript\n{}",
            fs::read_to_string(&log_path).unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(20));
    }
}
