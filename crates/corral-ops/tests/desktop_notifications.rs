#![cfg(target_os = "linux")]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use corral_core::{Status, Store};
use corral_herdr::Herdr;
use corral_ops::Ctx;

const NOTIFY: &str = r#"#!/bin/sh
if IFS= read -r input; then exit 90; fi
printf '%s\0' "$@" >> "$CORRAL_DESKTOP_LOG"
if [ -e "$CORRAL_DESKTOP_LOG.fail" ]; then exit 1; fi
"#;

const BLOCKED_HERDR: &str = r#"#!/bin/sh
if [ "$1" = "--session" ]; then shift 2; fi
case "$1 $2" in
  "workspace list") printf '%s\n' '{"result":{"workspaces":[]}}' ;;
  "pane list") printf '%s\n' '{"result":{"panes":[{"pane_id":"w1:p1","workspace_id":"w1","agent_status":"blocked"}]}}' ;;
  "pane read") printf '%s\n' 'Need a decision' ;;
  "notification show")
    printf '%s\n' "$3" >> "$CORRAL_DESKTOP_LOG.herdr"
    printf '%s\n' '{"result":{}}' ;;
  *) exit 1 ;;
esac
"#;

fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    herdr: PathBuf,
}

impl Fx {
    fn new(needs_me: bool, review: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let herdr = tmp.path().join("herdr");
        // Every herdr call stays at a fixture executable, never the user's server.
        executable(&herdr, "#!/bin/sh\nexit 1\n");
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.ui.notify_needs_me = needs_me;
        store.config.ui.notify_review = review;
        store.save_config().unwrap();
        let ctx = Ctx {
            store,
            herdr: Herdr::with_bin(herdr.clone(), None),
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        };
        Self {
            _tmp: tmp,
            ctx,
            herdr,
        }
    }

    fn sync(&self) {
        corral_ops::sync(&self.ctx).unwrap();
    }

    fn status(&self, id: &str, status: Status) {
        self.ctx
            .store
            .update(id, |task| {
                task.status = status;
                Ok(())
            })
            .unwrap();
    }

    fn flags(&mut self, needs_me: bool, review: bool) {
        self.ctx.store.config.ui.notify_needs_me = needs_me;
        self.ctx.store.config.ui.notify_review = review;
        self.ctx.store.save_config().unwrap();
    }

    fn restart(&mut self) {
        self.ctx.store = Store::open(self.ctx.store.root()).unwrap();
        self.ctx.herdr = Herdr::with_bin(self.herdr.clone(), None);
    }
}

fn log() -> Vec<String> {
    std::fs::read(std::env::var_os("CORRAL_DESKTOP_LOG").unwrap())
        .unwrap_or_default()
        .split(|b| *b == 0)
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
        .collect()
}

fn scenario(name: &str) {
    let tmp = tempfile::tempdir().unwrap();
    executable(&tmp.path().join("notify-send"), NOTIFY);
    // Child-local PATH avoids process-global environment mutation in parallel tests.
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "desktop_fixture_child", "--nocapture"])
        .env("CORRAL_DESKTOP_SCENARIO", name)
        .env("CORRAL_DESKTOP_LOG", tmp.path().join("notifications"))
        .env("PATH", tmp.path())
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_SESSION")
        .env_remove("CORRAL_HERDR_SESSION")
        .env_remove("HERDR_ENV")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // An inherited stdin would expose this marker to notify-send's read and fail.
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"must not reach notification\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn desktop_flags_are_independent_and_default_off() {
    scenario("flags");
}

#[test]
fn desktop_entries_survive_restart_without_repeating() {
    scenario("restart");
}

#[test]
fn desktop_disabled_entries_are_not_replayed_on_enablement() {
    scenario("disabled");
}

#[test]
fn desktop_failure_is_best_effort_without_recurring_retries() {
    scenario("failure");
}

#[test]
fn desktop_delivery_is_serialized_across_sync_callers() {
    scenario("lock");
}

#[test]
fn desktop_and_herdr_notifications_remain_independent_during_sync() {
    scenario("blocked");
}

#[test]
fn desktop_fixture_child() {
    let Ok(name) = std::env::var("CORRAL_DESKTOP_SCENARIO") else {
        return;
    };
    match name.as_str() {
        "blocked" => {
            for enabled in [false, true] {
                // Given a running agent newly blocked in the fake herdr inventory.
                let fx = Fx::new(enabled, false);
                executable(&fx.herdr, BLOCKED_HERDR);
                let task = fx
                    .ctx
                    .store
                    .create("Blocked agent", None, Status::Running, "")
                    .unwrap();
                let run = serde_json::from_value(serde_json::json!({
                    "id":"R1", "agent":"claude", "worktree":fx.ctx.store.root(),
                    "branch":"corral/T-001", "base":"main", "base_sha":"0000000",
                    "started":"2026-10-16T00:00:00Z",
                    "herdr":{"workspace_id":"w1", "pane_id":"w1:p1",
                        "socket":fx.ctx.herdr.socket_path().unwrap()}
                }))
                .unwrap();
                fx.ctx
                    .store
                    .update(&task.id, |task| {
                        task.runs.push(run);
                        Ok(())
                    })
                    .unwrap();
                let before = log().len();
                // When sync changes status and a later tick observes it again.
                fx.sync();
                fx.sync();
                // Then desktop follows its flag while herdr still notifies once.
                assert_eq!(fx.ctx.store.get(&task.id).unwrap().status, Status::NeedsMe);
                assert_eq!(log().len() - before, 4 * usize::from(enabled));
            }
            let path = PathBuf::from(std::env::var_os("CORRAL_DESKTOP_LOG").unwrap());
            assert_eq!(
                std::fs::read_to_string(path.with_extension("herdr"))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
        }
        "flags" => {
            // Given every combination, including the default configuration.
            let defaults = corral_core::UiCfg::default();
            assert!(!defaults.notify_needs_me);
            assert!(!defaults.notify_review);
            for (needs_me, review) in [(false, false), (true, false), (false, true), (true, true)] {
                let fx = Fx::new(needs_me, review);
                let before = log().len();
                fx.ctx
                    .store
                    .create("Needs attention", None, Status::NeedsMe, "")
                    .unwrap();
                fx.ctx
                    .store
                    .create("Ready to review", None, Status::Review, "")
                    .unwrap();
                // When sync sees the entries twice, even without herdr.
                fx.sync();
                fx.sync();
                // Then each enabled status invokes exactly one desktop process.
                let entries = log();
                let entries = &entries[before..];
                assert_eq!(
                    entries.len(),
                    4 * (usize::from(needs_me) + usize::from(review))
                );
                assert_eq!(entries.iter().any(|arg| arg == "T-001: Needs me"), needs_me);
                assert_eq!(entries.iter().any(|arg| arg == "T-002: Review"), review);
            }
        }
        "restart" => {
            // Given a baseline Running task.
            let mut fx = Fx::new(true, true);
            let title = "-danger $(touch should-not-exist); 'quoted'\nsecond line";
            let task = fx
                .ctx
                .store
                .create(title, None, Status::Running, "")
                .unwrap();
            fx.sync();
            // When an external writer completes it before the next sync.
            fx.status(&task.id, Status::Review);
            fx.sync();
            fx.restart();
            fx.sync();
            // Then argv is literal and restart does not repeat the entry.
            assert_eq!(log(), ["--app-name=corral", "--", "T-001: Review", title]);
            fx.ctx
                .store
                .update(&task.id, |task| {
                    task.title = "New title".into();
                    Ok(())
                })
                .unwrap();
            fx.sync();
            assert_eq!(log().len(), 4);
            // A later observed exit and re-entry is a new notification.
            fx.status(&task.id, Status::Running);
            fx.sync();
            fx.status(&task.id, Status::Review);
            fx.sync();
            assert_eq!(log().len(), 8);
            fx.status(&task.id, Status::NeedsMe);
            fx.sync();
            assert_eq!(log().len(), 12);
        }
        "disabled" => {
            // Given both entries observed while disabled.
            let mut fx = Fx::new(false, false);
            let task = fx
                .ctx
                .store
                .create("Review", None, Status::Review, "")
                .unwrap();
            fx.sync();
            fx.status(&task.id, Status::NeedsMe);
            fx.sync();
            // When notifications are enabled without changing status.
            fx.flags(true, true);
            fx.sync();
            // Then no historical entry is replayed, but a new entry is delivered.
            assert!(log().is_empty());
            fx.status(&task.id, Status::Review);
            fx.sync();
            assert_eq!(log().len(), 4);
        }
        "failure" => {
            // Given a notification process returning a failure status.
            let fx = Fx::new(true, true);
            let (sender, messages) = std::sync::mpsc::channel();
            let _capture = corral_ops::capture_messages(sender);
            let path = PathBuf::from(std::env::var_os("CORRAL_DESKTOP_LOG").unwrap());
            std::fs::write(path.with_extension("fail"), "").unwrap();
            fx.ctx
                .store
                .create("Review", None, Status::Review, "")
                .unwrap();
            // When recurring sync attempts reconcile the same status.
            fx.sync();
            fx.sync();
            // Then sync succeeds and attempts desktop delivery only once.
            assert_eq!(log().len(), 4);
            let failures: Vec<_> = messages
                .try_iter()
                .filter(|message| message.source == "desktop_notification")
                .collect();
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].task.as_deref(), Some("T-001"));
            assert!(
                fx.ctx
                    .store
                    .state_dir()
                    .join("desktop-notifications.json")
                    .exists()
            );
        }
        "lock" => {
            // Given another caller already owning the desktop checkpoint.
            let fx = Fx::new(true, true);
            fx.ctx
                .store
                .create("Review", None, Status::Review, "")
                .unwrap();
            let lock = fx
                .ctx
                .store
                .try_lock("desktop-notifications")
                .unwrap()
                .unwrap();
            fx.sync();
            assert!(log().is_empty());
            drop(lock);
            // When the next sync acquires the checkpoint.
            fx.sync();
            fx.sync();
            // Then exactly one invocation occurs.
            assert_eq!(log().len(), 4);
        }
        other => panic!("unknown desktop fixture scenario {other}"),
    }
}
