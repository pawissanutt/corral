use super::*;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub(super) struct Fixture {
    pub root: PathBuf,
    pub ctx: Ctx,
}

impl Fixture {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "corrald-test-{}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let store = Store::init(&root).unwrap();
        let mut ctx = Ctx::from_store(store);
        // Reuse the ops fake engine, never the user's herdr executable or socket.
        let fake = root.join("fake-herdr");
        std::fs::write(
            &fake,
            include_str!("../../corral-ops/tests/reviewer_ops/herdr.sh"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        ctx.herdr = corral_herdr::Herdr::with_bin(fake, Some("corral-test".into()));
        Self { root, ctx }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(daemon_client::runtime_dir(&self.ctx.store));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(unix)]
#[test]
fn second_daemon_and_one_shot_cannot_steal_running_loop() {
    let fixture = Fixture::new();
    let ctx = fixture.ctx.clone();
    let (started, ready) = mpsc::sync_channel(0);
    let (release, blocked) = mpsc::sync_channel(0);
    let daemon = std::thread::spawn(move || {
        run_with_pass(ctx, RunOptions { once: false }, |_, _| {
            started.send(()).unwrap();
            blocked.recv().unwrap();
            None
        })
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        run(&fixture.root, RunOptions { once: false }).unwrap(),
        Exit::AlreadyRunning
    );
    assert!(run(&fixture.root, RunOptions { once: true }).is_err());
    let mut client = daemon_client::Client::connect(&fixture.ctx.store)
        .unwrap()
        .unwrap();
    client.stop(false).unwrap();
    release.send(()).unwrap();
    assert_eq!(daemon.join().unwrap().unwrap(), Exit::Stopped);
    assert!(!fixture.ctx.store.owns_loop());
}

#[test]
fn in_pass_nudges_coalesce_and_completion_requires_a_later_started_pass() {
    let shared = Shared::new();
    let mut passes = 0;
    let mut injected_pass = || {
        passes += 1;
    };
    assert!(shared.begin());
    injected_pass();
    let target = shared.request_nudge();
    assert_eq!(target, shared.request_nudge());
    assert_eq!(target, shared.request_nudge());
    shared.finish(Duration::ZERO, None);
    let ack = shared.wait_nudge(target, Some(Duration::ZERO));
    assert!(!ack.completed);
    assert_eq!(ack.pass_seq, 1);
    assert!(shared.lock().pending);
    assert!(shared.begin());
    injected_pass();
    shared.finish(Duration::ZERO, None);
    let ack = shared.wait_nudge(target, Some(Duration::ZERO));
    assert!(ack.completed);
    assert_eq!(ack.pass_seq, 2);
    assert!(!shared.lock().pending);
    assert_eq!(passes, 2);
}

#[test]
fn cli_nudge_forces_full_pass() {
    let shared = Shared::new();
    let full = PassRequest::Full(FullReason::Owner);
    shared.nudge_from("cli", None);
    let taken = shared.take_request();
    assert_eq!(taken, (true, true));
    assert_eq!(pass_request(taken, false), full);
    let taken = shared.take_request();
    assert_eq!(taken, (false, false));
    assert_eq!(pass_request(taken, false), PassRequest::Paced);
    shared.nudge_from("hook", None);
    let taken = shared.take_request();
    assert_eq!(taken, (false, true));
    assert_eq!(pass_request(taken, false), PassRequest::Nudged);
    // The `sync-full-on` switch makes any pass full, under its own reason.
    assert_eq!(
        pass_request(taken, true),
        PassRequest::Full(FullReason::Switch)
    );
}

#[test]
fn near_due_time_never_cuts_the_cpu_proportional_wait() {
    use corral_ops::loop_pacing::{QUIET_EVERY, SYNC_EVERY};
    let spent = Duration::from_millis(100);
    let next_due = Some(Timestamp::now() + jiff::SignedDuration::from_secs(1));
    let full = PassReport {
        full: Some(FullReason::Audit),
        ran: 40,
        next_due,
        ..PassReport::default()
    };
    assert_eq!(Pacing::of(Some(&full)).wait(spent), periodic_wait(spent));
    assert_eq!(periodic_wait(spent), SYNC_EVERY);
    let quiet = PassReport {
        quiet: true,
        next_due,
        ..PassReport::default()
    };
    assert_eq!(Pacing::of(Some(&quiet)).wait(spent), QUIET_EVERY);
}

#[cfg(unix)]
#[test]
fn fatal_listener_error_returns_error_after_socket_and_lease_cleanup() {
    let fixture = Fixture::new();
    let shared = Arc::new(Shared::new());
    let socket = daemon_client::socket_path(&fixture.ctx.store);
    let result = run_with_state(
        fixture.ctx.clone(),
        RunOptions { once: false },
        |_, _| {
            assert!(socket.exists());
            server::accept_error(
                &shared,
                std::io::Error::other("injected fatal listener error"),
            );
            None
        },
        Arc::clone(&shared),
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("injected fatal listener error")
    );
    assert!(shared.lock().closed);
    assert!(!socket.exists());
    assert!(!fixture.ctx.store.owns_loop());
    assert!(fixture.ctx.store.own_loop().unwrap());
    fixture.ctx.store.release_loop();
}
