//! Event-controlled Podman and plugin-free herdr regression for T-685.
use super::*;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;

fn sweep(ctx: &Ctx) -> anyhow::Result<()> {
    crate::sandbox_cleanup::sweep(ctx, &ctx.store.list()?)
}

fn marker(ctx: &Ctx, task: &str, run: &str, value: &str) {
    let path = crate::launch_exit::receipt(ctx, task, run).with_extension("owner");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value).unwrap();
}

fn history(fx: &Fixture) -> String {
    let id = fx.task();
    fx.ctx
        .store
        .update(&id, |t| {
            t.status = Status::Done;
            let run = t.runs[0].clone();
            t.runs = (1..=256)
                .map(|n| {
                    let mut run = run.clone();
                    run.id = format!("R{n}");
                    run.ended = Some(run.started);
                    run
                })
                .collect();
            Ok(())
        })
        .unwrap();
    for n in 1..=256 {
        marker(&fx.ctx, &id, &format!("R{n}"), &format!("{n:064x}"));
    }
    marker(&fx.ctx, &id, "R1-review-1", &"a".repeat(64));
    id
}

fn fake_podman(fx: &Fixture, blocked: bool) -> std::path::PathBuf {
    let path = fx.tmp.path().join("podman");
    std::fs::write(
        &path,
        format!(
            r#"#!/usr/bin/env python3
import json, pathlib, socket, sys
root = pathlib.Path(__file__).parent
with (root / 'podman-calls').open('a') as f: f.write(' '.join(sys.argv[1:]) + '\n')
if sys.argv[1] == 'ps':
    if {blocked}:
        with socket.socket(socket.AF_UNIX) as s:
            s.settimeout(30)
            s.connect(str(root / 'podman.sock'))
            s.sendall(b'P')
            assert s.recv(1) == b'R'
    print((root / 'containers.json').read_text() if sys.argv[-1] == 'json' else '')
elif sys.argv[1] == 'rm':
    with (root / 'removed').open('a') as f: f.write(sys.argv[-1] + '\n')
"#,
            blocked = if blocked { "True" } else { "False" }
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn containers(fx: &Fixture, entries: &[(&str, String)]) {
    let values: Vec<_> = entries
        .iter()
        .map(|(id, marker)| {
            json!({
                "Id":id, "Labels":{"corral.load-sandbox":"true", "corral.run-owner":marker}
            })
        })
        .collect();
    std::fs::write(
        fx.tmp.path().join("containers.json"),
        serde_json::to_vec(&values).unwrap(),
    )
    .unwrap();
}

#[test]
fn historical_cleanup_does_not_hold_inventory_or_starve_gate_admission() {
    let fx = Fixture::new(4);
    history(&fx);
    let live = fx.task();
    let gate = fx.task();
    marker(&fx.ctx, &live, "R1", &"b".repeat(64));
    containers(
        &fx,
        &[
            ("123456789abc", format!("{:064x}", 1)),
            ("23456789abcd", "a".repeat(64)),
            ("3456789abcde", "b".repeat(64)),
            ("456789abcdef", "c".repeat(64)),
        ],
    );
    let podman = fake_podman(&fx, true);
    let listener = UnixListener::bind(fx.tmp.path().join("podman.sock")).unwrap();
    let (entered_tx, entered) = mpsc::channel();
    let acceptor = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        assert_eq!(&byte, b"P");
        entered_tx.send(stream).unwrap();
    });
    let ctx = fx.ctx.clone();
    let worker = std::thread::spawn(move || {
        crate::sandbox_cleanup::PODMAN
            .with(|slot| *slot.borrow_mut() = Some(podman.to_string_lossy().into()));
        crate::sync_at(&ctx, Timestamp::now())
    });
    let mut release = entered
        .recv_timeout(Duration::from_secs(20))
        .expect("sync did not reach Podman");
    // This is the exact exclusive guard gate/review admission needs. It must be free
    // while the query is blocked, not only after the worker finishes.
    let guard = fx.ctx.store.try_lock("session-ownership").unwrap();
    let free = guard.is_some();
    drop(guard);
    let launched = crate::gate::gate_start(&fx.ctx, &gate);
    // A concurrent hook must not queue another Podman query behind this one.
    let concurrent = sweep(&fx.ctx);
    release.write_all(b"R").unwrap();
    acceptor.join().unwrap();
    worker.join().unwrap().unwrap();
    assert!(
        free,
        "historical Podman query retained the global inventory lease"
    );
    launched.unwrap();
    concurrent.unwrap();
    let calls = std::fs::read_to_string(fx.tmp.path().join("podman-calls")).unwrap();
    assert_eq!(
        calls.lines().filter(|s| s.starts_with("ps ")).count(),
        1,
        "{calls}"
    );
    assert!(
        !calls.contains("label=corral.run-owner="),
        "per-historical-run query: {calls}"
    );
    assert_eq!(
        std::fs::read_to_string(fx.tmp.path().join("removed")).unwrap(),
        "123456789abc\n23456789abcd\n"
    );
}

#[test]
fn hundreds_of_terminal_owners_need_only_one_podman_query() {
    let fx = Fixture::new(2);
    history(&fx);
    let podman = fake_podman(&fx, false);
    containers(&fx, &[]);
    crate::sandbox_cleanup::PODMAN
        .with(|slot| *slot.borrow_mut() = Some(podman.to_string_lossy().into()));
    sweep(&fx.ctx).unwrap();
    crate::sandbox_cleanup::PODMAN.with(|slot| slot.borrow_mut().take());
    let calls = std::fs::read_to_string(fx.tmp.path().join("podman-calls")).unwrap();
    assert_eq!(
        calls.lines().count(),
        1,
        "257 historical launch owners must not cause 257 queries"
    );
}

#[test]
fn resumed_task_is_revalidated_after_blocked_container_listing() {
    let fx = Fixture::new(2);
    let id = history(&fx);
    let podman = fake_podman(&fx, true);
    containers(&fx, &[("123456789abc", format!("{:064x}", 1))]);
    let listener = UnixListener::bind(fx.tmp.path().join("podman.sock")).unwrap();
    let ctx = fx.ctx.clone();
    let worker = std::thread::spawn(move || {
        crate::sandbox_cleanup::PODMAN
            .with(|slot| *slot.borrow_mut() = Some(podman.to_string_lossy().into()));
        sweep(&ctx)
    });
    let (tx, rx) = mpsc::channel();
    let acceptor = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        tx.send(stream).unwrap();
    });
    let mut stream = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    fx.ctx
        .store
        .update(&id, |t| {
            t.status = Status::Running;
            Ok(())
        })
        .unwrap();
    stream.write_all(b"R").unwrap();
    acceptor.join().unwrap();
    worker.join().unwrap().unwrap();
    assert!(!fx.tmp.path().join("removed").exists());
}

#[test]
fn foreign_session_and_live_runs_are_not_sandbox_cleanup_authority() {
    let fx = Fixture::new(2);
    let id = history(&fx);
    let podman = fake_podman(&fx, false);
    containers(&fx, &[("123456789abc", format!("{:064x}", 1))]);
    let socket = fx.ctx.herdr.socket_path().unwrap();
    fx.ctx
        .store
        .bind_herdr_session(&socket, "fake isolated herdr")
        .unwrap();
    let mut foreign = fx.ctx.clone();
    foreign.herdr.set_session(Some("foreign".into()));
    crate::sandbox_cleanup::PODMAN
        .with(|slot| *slot.borrow_mut() = Some(podman.to_string_lossy().into()));
    sweep(&foreign).unwrap();
    assert!(!fx.tmp.path().join("podman-calls").exists());
    fx.ctx
        .store
        .update(&id, |t| {
            t.status = Status::Running;
            Ok(())
        })
        .unwrap();
    sweep(&fx.ctx).unwrap();
    crate::sandbox_cleanup::PODMAN.with(|slot| slot.borrow_mut().take());
    assert!(!fx.tmp.path().join("podman-calls").exists());
}
