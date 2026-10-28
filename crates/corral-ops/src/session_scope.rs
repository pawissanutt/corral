//! A store's pane IDs are meaningful only on their launch server.

use std::path::PathBuf;

use anyhow::{Result, bail};
use corral_core::{Board, HerdrRef, HerdrSessionBinding, Run, SessionView, Store, Task, TaskHead};

use crate::Ctx;

fn run_refs<'a>(run: &'a Run, refs: &mut Vec<&'a HerdrRef>) {
    refs.extend(run.herdr.iter());
    for child in &run.review_runs {
        run_refs(child, refs);
    }
}

/// Fail closed for mixed ownership, including legacy refs. This check is read-only
/// and deliberately silent: plugin events can invoke foreign hooks many times a second.
/// Within one pass the bounded summary is checked once per selected socket.
pub(crate) fn owns_inventory(ctx: &Ctx) -> Result<bool> {
    let view = ctx.store.session_view()?;
    owns_summary(ctx, &view)
}

/// Check the snapshots actually being consumed, not a task list read before IPC.
/// The binding comes from the current pass's single read.
pub(crate) fn owns_tasks(ctx: &Ctx, store: &Store, tasks: &[Task]) -> Result<bool> {
    let view = store.session_view()?;
    if let [task] = tasks {
        // A task-local mutation must not inherit an unrelated task's foreign ownership.
        return head_owned(ctx, &view, &store.head(&task.id)?);
    }
    owns_summary(ctx, &view)
}

/// [`owns_tasks`] over the whole board, from its open heads and status counts: a store of one
/// task checks that task's head, any other store the bounded summary.
pub(crate) fn owns_board(ctx: &Ctx, store: &Store, board: &Board) -> Result<bool> {
    let view = store.session_view()?;
    let total: u64 = board.status_counts.iter().map(|c| c.n).sum();
    match board.heads.as_slice() {
        [head] if total == 1 => head_owned(ctx, &view, head),
        [] if total == 1 => {
            let sole = ["done", "dropped"].iter().find_map(|status| {
                store
                    .heads_page(
                        &corral_core::backend::HeadQuery::RecentByStatus {
                            status: (*status).to_owned(),
                        },
                        1,
                    )
                    .map(|heads| heads.into_iter().next())
                    .transpose()
            });
            match sole.transpose()? {
                Some(head) => head_owned(ctx, &view, &head),
                None => owns_summary(ctx, &view),
            }
        }
        _ => owns_summary(ctx, &view),
    }
}

/// [`owns_task`] from a head: the summary's and current containers' pane references cover
/// every pane the task recorded.
pub(crate) fn owns_head(ctx: &Ctx, head: &TaskHead) -> Result<bool> {
    let view = ctx.store.session_view()?;
    head_owned(ctx, &view, head)
}

/// A task or a head, for checks that accept either.
#[derive(Clone, Copy)]
pub(crate) enum Owner<'a> {
    Task(&'a Task),
    Head(&'a TaskHead),
}

impl<'a> From<&'a Task> for Owner<'a> {
    fn from(task: &'a Task) -> Self {
        Owner::Task(task)
    }
}

impl<'a> From<&'a TaskHead> for Owner<'a> {
    fn from(head: &'a TaskHead) -> Self {
        Owner::Head(head)
    }
}

pub(crate) fn owns(ctx: &Ctx, owner: Owner<'_>) -> Result<bool> {
    match owner {
        Owner::Task(task) => owns_task(ctx, task),
        Owner::Head(head) => owns_head(ctx, head),
    }
}

fn head_owned(ctx: &Ctx, view: &SessionView, head: &TaskHead) -> Result<bool> {
    {
        let actual = ctx.herdr.socket_path()?;
        let legacy = view.binding.as_ref().map(|binding| &binding.socket);
        let owns = |socket: &str| {
            let saved = (!socket.is_empty()).then(|| std::path::Path::new(socket));
            saved.or(legacy.map(PathBuf::as_path)) == Some(actual.as_path())
        };
        let facts = head
            .current_run
            .iter()
            .map(|run| &run.facts)
            .chain(head.last_plan_run.iter().map(|run| &run.facts))
            .chain(head.last_plan.iter().map(|plan| &plan.facts))
            .chain(
                head.last_aggregate_attempt
                    .iter()
                    .map(|attempt| &attempt.facts),
            );
        Ok(legacy.is_none_or(|socket| socket == &actual)
            && head.summary.pane_refs.keys().all(|socket| owns(socket))
            && facts
                .flat_map(|facts| facts.pane_refs.keys())
                .all(|socket| owns(socket))
            && head
                .launch
                .as_ref()
                .is_none_or(|launch| launch.socket.as_ref().or(legacy) == Some(&actual))
            && view
                .pending
                .as_ref()
                .is_none_or(|socket| socket.as_ref().or(legacy) == Some(&actual)))
    }
}

fn owns_summary(ctx: &Ctx, view: &corral_core::SessionView) -> Result<bool> {
    if view.binding.is_none() && view.pending.is_none() && view.owners.is_empty() {
        return Ok(true);
    }
    view.owns(&ctx.herdr.socket_path()?)
}

fn owns_with(
    ctx: &Ctx,
    binding: Option<&HerdrSessionBinding>,
    pending: Option<&Option<PathBuf>>,
    tasks: &[Task],
) -> Result<bool> {
    let mut refs = Vec::new();
    for task in tasks {
        for run in &task.runs {
            run_refs(run, &mut refs);
        }
        refs.extend(task.plan_runs.iter().filter_map(|r| r.herdr.as_ref()));
        refs.extend(
            task.plans
                .iter()
                .flat_map(|p| &p.reviews)
                .filter_map(|r| r.herdr.as_ref()),
        );
        refs.extend(
            task.aggregate_attempts
                .iter()
                .flat_map(|a| &a.reviews)
                .filter_map(|r| r.herdr.as_ref()),
        );
    }
    let launches: Vec<_> = tasks
        .iter()
        .filter_map(|task| task.launch.as_ref())
        .collect();
    let gates: Vec<_> = tasks
        .iter()
        .flat_map(|task| &task.runs)
        .flat_map(gate_sockets)
        .collect();
    if refs.is_empty()
        && launches.is_empty()
        && gates.is_empty()
        && pending.is_none()
        && binding.is_none()
    {
        // There are no server-scoped pane IDs or reserved launches to reconcile yet.
        return Ok(true);
    }
    let actual = ctx.herdr.socket_path()?;
    let legacy = binding.map(|binding| &binding.socket);
    Ok(legacy.is_none_or(|socket| socket == &actual)
        && refs
            .into_iter()
            .all(|h| h.socket.as_ref().or(legacy) == Some(&actual))
        && launches
            .into_iter()
            .all(|launch| launch.socket.as_ref().or(legacy) == Some(&actual))
        && gates
            .into_iter()
            .all(|socket| socket.or(legacy) == Some(&actual))
        && pending.is_none_or(|socket| socket.as_ref().or(legacy) == Some(&actual)))
}

fn gate_sockets(run: &Run) -> Vec<Option<&std::path::PathBuf>> {
    let mut sockets = Vec::new();
    if let Some(gate) = &run.gate
        && gate.pane.is_some()
    {
        sockets.push(gate.socket.as_ref());
    }
    for child in &run.review_runs {
        sockets.extend(gate_sockets(child));
    }
    sockets
}

/// One task's check, using the pass's binding read; not a board scan.
pub(crate) fn owns_task(ctx: &Ctx, task: &Task) -> Result<bool> {
    let view = ctx.store.session_view()?;
    owns_with(
        ctx,
        view.binding.as_ref(),
        view.pending.as_ref(),
        std::slice::from_ref(task),
    )
}

/// New launches may bind an empty store, but may not mix existing pane owners.
pub(crate) fn check_launch(ctx: &Ctx) -> Result<()> {
    check_launch_in(ctx, &ctx.store)
}

/// Called under the board writer lock before publishing any launch claim. It reads the
/// binding afresh, never a pass's earlier read.
pub(crate) fn check_launch_in(ctx: &Ctx, store: &Store) -> Result<()> {
    let view = store.read_session_view()?;
    if !owns_summary(ctx, &view)? {
        bail!(
            "selected herdr session does not own this store's panes; unbound legacy panes require `corral bind-session --socket <launch-socket> --launch-record <evidence-file>`"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use corral_core::{Run, Status, Store};

    use super::*;

    const B_ROOT: &str = "T342_LAUNCH_B_ROOT";
    const B_SOCKET: &str = "T342_LAUNCH_B_SOCKET";

    /// Process B: this test binary re-executed, binding from its own open of the store.
    #[test]
    fn launch_boundary_process_b() {
        let (Some(root), Some(socket)) = (std::env::var_os(B_ROOT), std::env::var_os(B_SOCKET))
        else {
            return;
        };
        let store = Store::open_unobserved(Path::new(&root)).unwrap();
        if let Err(error) = store.bind_herdr_session(Path::new(&socket), "T-342 process B") {
            println!("refused: {error:#}");
            std::process::exit(3);
        }
    }

    fn bind_in_process_b(root: &Path, socket: &Path) {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "session_scope::tests::launch_boundary_process_b"])
            .env(B_ROOT, root)
            .env(B_SOCKET, socket)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }

    fn running(store: &Store, socket: Option<PathBuf>) {
        let task = store.create("Pane", None, Status::Running, "").unwrap();
        let run: Run = serde_json::from_value(serde_json::json!({
            "id":"R1", "agent":"claude", "worktree":store.root(),
            "branch":"corral/T-001", "base":"main", "base_sha":"0000000",
            "started":"2026-10-16T00:00:00Z",
            "herdr":{"workspace_id":"w1", "pane_id":"w1:p1", "socket":socket}
        }))
        .unwrap();
        store
            .update(&task.id, |task| {
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn bind_after_pass_start_is_seen_by_the_locked_launch_boundary() {
        for saved in [true, false] {
            // Given A's store, unbound: a legacy socket-less pane, plus a saved one in the
            // saved variant. A's pass read the binding as absent, so A owns nothing.
            let tmp = tempfile::tempdir().unwrap();
            let ctx = Ctx {
                store: Store::init(tmp.path()).unwrap(),
                herdr: corral_herdr::Herdr::with_bin(tmp.path().join("herdr"), Some("t342".into())),
                resource_probe: crate::resources::FixedResourceProbe::arc(),
            };
            let socket = ctx.herdr.socket_path().unwrap();
            if saved {
                running(&ctx.store, Some(socket.clone()));
            }
            running(&ctx.store, None);
            let launch = ctx.store.create("Launch", None, Status::Ready, "").unwrap();
            let pass = ctx.clone();
            drop(pass.store.try_begin_session_pass().unwrap().unwrap());
            assert!(!owns_inventory(&pass).unwrap());
            let refused = pass
                .store
                .dispatch_update(&launch.id, |store, _| check_launch_in(&pass, store));
            assert!(format!("{:#}", refused.unwrap_err()).contains("does not own"));

            // When process B binds the store to A's socket after A's pass started.
            bind_in_process_b(tmp.path(), &socket);

            // Then A's pass keeps its read, but the board-locked launch boundary reads afresh.
            assert!(pass.store.session_view().unwrap().binding.is_none());
            assert!(!owns_inventory(&pass).unwrap());
            let (_, view) = pass
                .store
                .dispatch_update(&launch.id, |store, _| {
                    check_launch_in(&pass, store)?;
                    store.read_session_view()
                })
                .unwrap();
            assert_eq!(
                view.binding.as_ref().unwrap().socket,
                socket,
                "saved={saved}"
            );
        }
    }
}
