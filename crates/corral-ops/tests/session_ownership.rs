//! T-342: each pass reads the herdr session binding once, and a second process's bind is
//! refused during inventory, seen at the launch boundary, and never hidden from a later pass.
#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use corral_core::work::{Work, count};
use corral_core::{Run, Status, Store};
use corral_herdr::Herdr;
use corral_ops::Ctx;

/// A blocked agent: sync reads its pane, under the inventory's ownership lease, and that read
/// runs process B's bind to completion before answering.
const HERDR: &str = r#"#!/bin/sh
dir=$(dirname "$0")
if [ "$1" = "--session" ]; then shift 2; fi
case "$1 $2" in
  "workspace list") printf '%s\n' '{"result":{"workspaces":[]}}' ;;
  "pane list") printf '%s\n' '{"result":{"panes":[{"pane_id":"w1:p1","workspace_id":"w1","agent_status":"blocked"}]}}' ;;
  "pane read")
    if [ -x "$dir/process-b" ]; then
      "$dir/process-b" >> "$dir/process-b.log" 2>&1
      echo "exit $?" >> "$dir/process-b.log"
    fi
    printf '%s\n' 'Need a decision' ;;
  *) printf '%s\n' '{"result":{}}' ;;
esac
"#;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Refs {
    /// Pane references carry their launch socket.
    Saved,
    /// Socket-less references, owned only through an explicit binding.
    Legacy,
}

struct Fx {
    tmp: tempfile::TempDir,
    ctx: Ctx,
    socket: PathBuf,
    task: String,
}

fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn fixture(refs: Refs) -> Fx {
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    let herdr = tmp.path().join("herdr");
    executable(&herdr, HERDR);
    let ctx = Ctx {
        store: Store::init(tmp.path()).unwrap(),
        herdr: Herdr::with_bin(herdr, Some("t342-owner".into())),
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
    };
    let socket = ctx.herdr.socket_path().unwrap();
    let task = ctx
        .store
        .create("Blocked agent", None, Status::Running, "")
        .unwrap();
    let saved = (refs == Refs::Saved).then(|| socket.clone());
    let run: Run = serde_json::from_value(serde_json::json!({
        "id":"R1", "agent":"claude", "worktree":ctx.store.root(),
        "branch":"corral/T-001", "base":"main", "base_sha":"0000000",
        "started":"2026-10-16T00:00:00Z",
        "herdr":{"workspace_id":"w1", "pane_id":"w1:p1", "socket":saved}
    }))
    .unwrap();
    ctx.store
        .update(&task.id, |task| {
            task.runs.push(run);
            Ok(())
        })
        .unwrap();
    Fx {
        tmp,
        ctx,
        socket,
        task: task.id,
    }
}

impl Fx {
    /// Process B: this test binary re-executed to bind the store from its own open.
    fn process_b(&self) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "process_b_binds_session", "--nocapture"])
            .env("T342_PROCESS_B_ROOT", self.ctx.store.root())
            .env("T342_PROCESS_B_SOCKET", &self.socket)
            .stdin(Stdio::null());
        command
    }

    fn bind_in_process_b(&self) -> Output {
        self.process_b().output().unwrap()
    }

    /// Makes the fake engine run process B from inside A's pane read.
    fn arm_process_b(&self) {
        let command = self.process_b();
        let mut script = String::from("#!/bin/sh\nexec env");
        for (name, value) in command.get_envs() {
            let value = value.unwrap().to_str().unwrap();
            script.push_str(&format!(" {}='{value}'", name.to_str().unwrap()));
        }
        script.push_str(&format!(" '{}'", command.get_program().to_str().unwrap()));
        for arg in command.get_args() {
            script.push_str(&format!(" '{}'", arg.to_str().unwrap()));
        }
        script.push_str(" </dev/null\n");
        executable(&self.tmp.path().join("process-b"), &script);
    }

    fn process_b_log(&self) -> String {
        std::fs::read_to_string(self.tmp.path().join("process-b.log")).unwrap_or_default()
    }

    fn status(&self) -> Status {
        self.ctx.store.get(&self.task).unwrap().status
    }

    fn bound_socket(&self) -> Option<PathBuf> {
        let binding = self.ctx.store.herdr_session_binding().unwrap();
        binding.map(|binding| binding.socket)
    }

    /// One read-only pass, as `corral pm next` and `corral automation status` evaluate.
    fn pass(&self) -> Ctx {
        Ctx {
            store: self.ctx.store.read_only_snapshot().unwrap(),
            ..self.ctx.clone()
        }
    }
}

fn owned(pass: &Ctx) -> bool {
    corral_ops::pm_health(pass).unwrap()["sync_owned"]
        .as_bool()
        .unwrap()
}

fn binding_reads() -> u64 {
    count(Work::SessionBindingRead)
}

#[test]
fn process_b_binds_session() {
    let (Some(root), Some(socket)) = (
        std::env::var_os("T342_PROCESS_B_ROOT"),
        std::env::var_os("T342_PROCESS_B_SOCKET"),
    ) else {
        return;
    };
    let store = Store::open_unobserved(Path::new(&root)).unwrap();
    if let Err(error) = store.bind_herdr_session(Path::new(&socket), "T-342 process B record") {
        println!("refused: {error:#}");
        std::process::exit(3);
    }
    println!("bound");
}

#[test]
fn bind_is_refused_while_inventory_holds_the_ownership_lease() {
    for refs in [Refs::Saved, Refs::Legacy] {
        // Given A owns the blocked pane: by its saved socket, or legacy refs plus a binding.
        let fx = fixture(refs);
        if refs == Refs::Legacy {
            fx.ctx
                .store
                .bind_herdr_session(&fx.socket, "owner launch record")
                .unwrap();
        }
        fx.arm_process_b();

        // When process B binds while A's sync pass reads the pane under its inventory lease.
        corral_ops::sync(&fx.ctx).unwrap();

        // Then B is refused for the whole pass, and A's pass still reconciles the pane.
        let log = fx.process_b_log();
        assert!(log.contains("exit 3"), "{refs:?}: {log}");
        assert!(!log.contains("exit 0"), "{refs:?}: {log}");
        assert!(
            log.contains("pane inventory is being reconciled"),
            "{refs:?}: {log}"
        );
        assert_eq!(fx.status(), Status::NeedsMe, "{refs:?}");
        // Once the pass released the lease, B's bind commits (or is already this binding).
        let after = fx.bind_in_process_b();
        assert!(after.status.success(), "{refs:?}: {after:?}");
        assert_eq!(fx.bound_socket(), Some(fx.socket.clone()), "{refs:?}");
    }
}

#[test]
fn bind_after_a_pass_started_is_kept_out_of_that_pass_and_seen_by_the_next() {
    for refs in [Refs::Saved, Refs::Legacy] {
        // Given A's read-only pass has read the binding as absent.
        let fx = fixture(refs);
        let pass = fx.pass();
        let before = binding_reads();
        assert_eq!(owned(&pass), refs == Refs::Saved, "{refs:?}");
        assert!(pass.store.session_view().unwrap().binding.is_none());
        assert_eq!(binding_reads(), before + 1, "{refs:?}");

        // When process B commits a bind after that pass started.
        let bound = fx.bind_in_process_b();
        assert!(bound.status.success(), "{refs:?}: {bound:?}");

        // Then the pass keeps its single read...
        assert_eq!(owned(&pass), refs == Refs::Saved, "{refs:?}");
        assert!(pass.store.session_view().unwrap().binding.is_none());
        assert_eq!(binding_reads(), before + 1, "{refs:?}");
        // ...and A's next pass sees it. (The board-locked launch boundary's fresh read is
        // covered by session_scope::tests.)
        let next = fx.pass();
        assert!(owned(&next), "{refs:?}");
        let view = next.store.session_view().unwrap();
        assert_eq!(view.binding.as_ref().unwrap().socket, fx.socket, "{refs:?}");
    }
}

#[test]
fn absent_binding_observed_by_a_pass_is_not_reused_by_the_next_pass() {
    for refs in [Refs::Saved, Refs::Legacy] {
        // Given A's sync pass and read-only pass both observed no binding.
        let fx = fixture(refs);
        corral_ops::sync(&fx.ctx).unwrap();
        let expected = match refs {
            Refs::Saved => Status::NeedsMe,
            // Unbound legacy refs belong to no session: that sync was a no-op.
            Refs::Legacy => Status::Running,
        };
        assert_eq!(fx.status(), expected, "{refs:?}");
        let pass = fx.pass();
        assert!(pass.store.session_view().unwrap().binding.is_none());
        drop(pass);

        // When process B binds the store between A's passes.
        let bound = fx.bind_in_process_b();
        assert!(bound.status.success(), "{refs:?}: {bound:?}");

        // Then A's next passes read the binding again instead of reusing the absence.
        let before = binding_reads();
        let next = fx.pass();
        let view = next.store.session_view().unwrap();
        assert_eq!(view.binding.as_ref().unwrap().socket, fx.socket, "{refs:?}");
        assert!(owned(&next), "{refs:?}");
        assert_eq!(binding_reads(), before + 1, "{refs:?}");
        corral_ops::sync(&fx.ctx).unwrap();
        assert!(binding_reads() > before + 1, "{refs:?}");
        assert_eq!(fx.status(), Status::NeedsMe, "{refs:?}");
    }
}

#[test]
fn read_only_pass_reads_schema_binding_and_ownership_once() {
    for refs in [Refs::Saved, Refs::Legacy] {
        // Given a bound store with pane references.
        let fx = fixture(refs);
        fx.ctx
            .store
            .bind_herdr_session(&fx.socket, "owner launch record")
            .unwrap();
        let start = Work::ALL.map(count);

        // When a fresh process opens it and evaluates pm next and automation status on one
        // read-only snapshot, checking ownership twice.
        let ctx = Ctx {
            store: Store::open(fx.ctx.store.root()).unwrap(),
            ..fx.ctx.clone()
        };
        let pass = Ctx {
            store: ctx.store.read_only_snapshot().unwrap(),
            ..ctx.clone()
        };
        corral_ops::pm_workset(&pass).unwrap();
        corral_ops::automation_summary(&pass).unwrap();
        corral_ops::pm_next(&pass).unwrap();
        assert!(owned(&pass), "{refs:?}");
        assert!(owned(&pass), "{refs:?}");

        // Then open plus the snapshot read the schema, and the pass one binding and one scan.
        let delta = |work: Work| count(work) - start[work as usize];
        assert!(delta(Work::SchemaRead) <= 2, "{refs:?}");
        assert_eq!(delta(Work::SessionBindingRead), 1, "{refs:?}");
        assert_eq!(delta(Work::OwnershipScan), 1, "{refs:?}");
    }
}

/// Binding reads and ownership scans of one sync that reconciles `blocked` Running tasks.
fn sync_work(blocked: usize) -> (u64, u64) {
    let fx = fixture(Refs::Saved);
    let mut config = fx.ctx.store.config.clone();
    config.dispatch.paused = true;
    config.review.autopilot = Some(false);
    let mut store = fx.ctx.store.clone();
    store.config = config;
    store.save_config().unwrap();
    let ctx = Ctx {
        store: Store::open(store.root()).unwrap(),
        ..fx.ctx.clone()
    };
    let run = ctx.store.get(&fx.task).unwrap().runs[0].clone();
    for i in 1..blocked {
        let task = ctx
            .store
            .create(&format!("Blocked {i}"), None, Status::Running, "")
            .unwrap();
        ctx.store
            .update(&task.id, |task| {
                task.runs.push(run.clone());
                Ok(())
            })
            .unwrap();
    }
    let (reads, scans) = (binding_reads(), count(Work::OwnershipScan));
    let changes = corral_ops::sync(&ctx).unwrap();
    assert_eq!(changes.len(), blocked);
    let needs_me = ctx.store.list().unwrap();
    assert!(needs_me.iter().all(|task| task.status == Status::NeedsMe));
    (binding_reads() - reads, count(Work::OwnershipScan) - scans)
}

#[test]
fn sync_binding_reads_and_scans_do_not_grow_with_reconciliation_writes() {
    // Given one and sixteen blocked agents, each reconciled to NeedsMe with its own write.
    // When one sync runs over each board.
    let one = sync_work(1);
    let sixteen = sync_work(16);
    // Then the pass's protected writes and per-task checks reuse its single read.
    assert_eq!(one, sixteen);
    println!(
        "per sync: session_binding_reads={} ownership_scans={}",
        one.0, one.1
    );
}
