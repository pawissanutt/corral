use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    Actor, Applied, CheckKind, CheckResult, Engine, NewTask, Op, Origin, Outcome, PipelineMode,
    Priority, Role, Run, SessionStatus, Status, Task, as_refusal,
    config::Project,
    events::{Event, EventKind},
    testing::FakeRunner,
};

struct Fixture {
    _root: tempfile::TempDir,
    repo: PathBuf,
    engine: Engine,
    runner: FakeRunner,
}

fn git_output(repo: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap()
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = git_output(repo, args);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim_end().into()
}

impl Fixture {
    fn new(gate: &[&str]) -> Self {
        let root = crate::tests::tempdir();
        let repo = root.path().join("project");
        fs::create_dir(&repo).unwrap();
        let repo = repo.canonicalize().unwrap();
        git(&repo, &["init", "-b", "main"]);
        for (key, value) in [
            ("user.name", "Corral Test"),
            ("user.email", "corral@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            git(&repo, &["config", key, value]);
        }
        fs::write(repo.join(".gitignore"), "/.worktrees/\n").unwrap();
        fs::write(repo.join("file.txt"), "base\n").unwrap();
        fs::write(repo.join("unrelated.txt"), "unchanged\n").unwrap();
        fs::write(repo.join("AGENTS.md"), "project guidance\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "base"]);
        let runner = FakeRunner::new();
        runner.set_default_status(SessionStatus::Idle);
        let mut engine = Engine::init(root.path(), None).unwrap();
        engine.actor = Actor::Owner;
        engine.runner = Box::new(runner.clone());
        engine.pipeline = PipelineMode::Inline;
        engine.config.projects.insert(
            "project".into(),
            Project {
                path: "project".into(),
                profile: Some("generic".into()),
                gate: gate.iter().map(|line| (*line).into()).collect(),
                ..Default::default()
            },
        );
        Self {
            _root: root,
            repo,
            engine,
            runner,
        }
    }

    fn project(&mut self) -> &mut Project {
        self.engine.config.projects.get_mut("project").unwrap()
    }

    fn new_task(
        &self,
        title: &str,
        status: Status,
        priority: Priority,
        depends_on: Vec<String>,
    ) -> String {
        self.apply(
            Op::New {
                task: NewTask {
                    title: title.into(),
                    body: "Goal and acceptance".into(),
                    project: Some("project".into()),
                    status,
                    priority,
                    depends_on,
                    ..Default::default()
                },
            },
            false,
        )
        .task
        .unwrap()
        .id
    }

    fn backlog(&self, title: &str) -> String {
        self.new_task(title, Status::Backlog, Priority::P2, vec![])
    }

    fn apply(&self, op: Op, force: bool) -> Applied {
        self.engine.apply(op, force).unwrap()
    }
    fn dispatch(&self, id: &str) -> Run {
        self.apply(dispatch(id), false).run.unwrap()
    }
    fn done(&self, id: &str) -> Applied {
        self.apply(
            Op::Done {
                id: id.into(),
                summary: "finished it".into(),
            },
            false,
        )
    }
    fn task(&self, id: &str) -> Task {
        self.engine.store.require_task(id).unwrap()
    }
    fn events(&self, id: &str, kind: EventKind) -> Vec<Event> {
        self.engine
            .store
            .list_events(id)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == kind)
            .collect()
    }
    fn branch_exists(&self, branch: &str) -> bool {
        git_output(
            &self.repo,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .status
        .success()
    }
    fn prompt(&self, run: &Run) -> String {
        fs::read_to_string(
            self.engine
                .state_dir
                .join("runs")
                .join(&run.task)
                .join(&run.id)
                .join("prompt.md"),
        )
        .unwrap()
    }
    fn assert_landed(&self, run: &Run, head: &str) {
        assert_eq!(git(&self.repo, &["rev-parse", "refs/heads/main"]), head);
        assert!(!run.worktree.exists());
        assert!(!self.branch_exists(&run.branch));
        assert!(
            !self
                .engine
                .state_dir
                .join("locks")
                .join(format!("{}.pipeline", run.task))
                .exists()
        );
        assert_eq!(self.task(&run.task).status, Status::Done);
        assert_eq!(
            self.engine
                .store
                .get_run(&run.task, &run.id)
                .unwrap()
                .unwrap()
                .outcome,
            Some(Outcome::Landed)
        );
    }
}

fn dispatch(id: &str) -> Op {
    Op::Dispatch {
        id: id.into(),
        profile: None,
        base: None,
    }
}
fn approve(id: &str) -> Op {
    approve_onto(id, None)
}

fn approve_onto(id: &str, onto: Option<&str>) -> Op {
    Op::Approve {
        id: id.into(),
        onto: onto.map(str::to_owned),
    }
}

#[test]
fn dispatch_done_green_gate_approve_cleans_up_without_requiring_clean_base() {
    let mut cx = Fixture::new(&["test \"$EXPECTED\" = \"project environment\" && cat file.txt"]);
    cx.project()
        .env
        .insert("EXPECTED".into(), "project environment".into());
    let id = cx.backlog("make a change");
    let run = cx.dispatch(&id);
    let prompt = cx.prompt(&run);
    let bin = crate::binary_path().unwrap();
    assert!(prompt.contains("Goal and acceptance"));
    assert!(prompt.contains("AGENTS.md"));
    assert!(prompt.contains(&format!("{}' done {id} --summary", bin.display())));
    assert!(prompt.contains(&format!("artifact add {id}")));
    assert!(
        cx.runner.starts()[0]
            .env
            .contains(&("EXPECTED".into(), "project environment".into()))
    );
    assert!(
        cx.runner.starts()[0]
            .env
            .contains(&("CORRAL_TASK".into(), id.clone()))
    );
    fs::write(run.worktree.join("file.txt"), "worker change\n").unwrap();
    fs::write(run.worktree.join("report.html"), "evidence\n").unwrap();
    let first = cx.done(&id);
    let head = first.run.unwrap().head_sha.unwrap();
    assert_eq!(first.task.unwrap().status, Status::Review);
    assert_eq!(first.seq, cx.engine.store.latest_seq().unwrap());
    assert_eq!(first.seq, cx.done(&id).seq);
    assert_eq!(cx.events(&id, EventKind::Ready).len(), 1);
    assert_eq!(cx.events(&id, EventKind::Gate).len(), 1);
    let check = cx
        .engine
        .store
        .latest_check(&id, CheckKind::Gate, &head)
        .unwrap()
        .unwrap();
    assert_eq!(check.result, CheckResult::Pass);
    assert!(
        String::from_utf8(
            cx.engine
                .show_artifact(check.log.as_deref().unwrap())
                .unwrap()
        )
        .unwrap()
        .contains("worker change")
    );
    let titles: Vec<_> = cx
        .engine
        .list_artifacts(Some(&id))
        .unwrap()
        .into_iter()
        .map(|a| a.title)
        .collect();
    assert!(titles.iter().any(|title| title == "diff.patch"));
    assert!(titles.iter().any(|title| title == "summary.md"));
    assert!(titles.iter().any(|title| title == "report.html"));
    let session = run.session.as_ref().unwrap();
    cx.runner.set_status(session, SessionStatus::Working);
    fs::write(cx.repo.join("unrelated.txt"), "local edit\n").unwrap();
    let landed = cx.apply(approve(&id), false);
    assert_eq!(landed.task.unwrap().status, Status::Done);
    cx.assert_landed(&run, &head);
    assert_eq!(cx.runner.stops(), std::slice::from_ref(session));
    assert_eq!(
        fs::read_to_string(cx.repo.join("unrelated.txt")).unwrap(),
        "local edit\n"
    );
}

#[test]
fn red_gate_sends_back_then_blocks_with_typed_owner_actions() {
    let mut cx = Fixture::new(&["test -f green", "printf 'unexpected second line' "]);
    cx.project().max_rounds = Some(1);
    let id = cx.backlog("gate red");
    let run = cx.dispatch(&id);
    fs::write(run.worktree.join("file.txt"), "round one\n").unwrap();
    let task = cx.done(&id).task.unwrap();
    assert_eq!((task.status, task.round), (Status::Running, 1));
    assert!(cx.runner.sends()[0].text.contains("test -f green"));
    let gate = &cx.engine.store.list_checks(&id).unwrap()[0];
    let log = cx
        .engine
        .show_artifact(gate.log.as_deref().unwrap())
        .unwrap();
    assert!(
        !String::from_utf8(log)
            .unwrap()
            .contains("unexpected second line")
    );
    fs::write(run.worktree.join("file.txt"), "round two\n").unwrap();
    let task = cx.done(&id).task.unwrap();
    assert_eq!((task.status, task.round), (Status::Blocked, 1));
    let next = task.blocked.unwrap().next;
    assert!(
        matches!(&next[..], [a, b, c] if matches!(&a.op, Op::Changes { id: a, .. } if a == &id)
        && matches!(&b.op, Op::Retry { id: b, .. } if b == &id) && c.force && c.op == approve(&id))
    );
    assert_eq!(cx.runner.sends().len(), 1);
    let task = cx
        .apply(
            Op::Changes {
                id,
                text: "owner repair".into(),
            },
            false,
        )
        .task
        .unwrap();
    assert_eq!((task.status, task.round), (Status::Running, 0));
    assert_eq!(cx.runner.sends()[1].text, "owner repair");
}

#[test]
fn reviewer_requests_changes_then_approves_and_auto_lands_off_base() {
    let mut cx = Fixture::new(&[]);
    cx.project().reviewer = Some("generic".into());
    cx.project().auto_land = Some(true);
    git(&cx.repo, &["switch", "-c", "local"]);
    git(
        &cx.repo,
        &["update-ref", "refs/remotes/origin/main", "refs/heads/main"],
    );
    git(
        &cx.repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    for accepts_send in [true, false] {
        let starts_before = cx.runner.starts().len();
        let sends_before = cx.runner.sends().len();
        let id = cx.backlog("reviewed change");
        let worker = cx.dispatch(&id);
        assert_eq!(worker.base, "main");
        fs::write(worker.worktree.join("file.txt"), "first draft\n").unwrap();
        let previous_head = cx.done(&id).run.unwrap().head_sha.unwrap();
        let reviewer = || {
            cx.engine
                .store
                .latest_run(&id, Role::Reviewer)
                .unwrap()
                .unwrap()
        };
        let original = reviewer();
        let session = original.session.as_ref().unwrap();
        let prompt = cx.prompt(&original);
        assert!(prompt.contains(&format!("git diff {}...HEAD", worker.base_sha)));
        let finish = format!("verdict {id} approve --notes-file");
        assert!(prompt.contains(&finish));
        if !accepts_send {
            cx.runner.refuse_send(session);
        }

        // The old worker is gone, so the send-back starts a replacement worker.
        cx.runner
            .set_status(worker.session.as_ref().unwrap(), SessionStatus::Gone);
        let verdict = |approve, notes: &str| Op::Verdict {
            id: id.clone(),
            approve,
            notes: Some(notes.into()),
        };
        let request = cx.apply(verdict(false, "revise the change"), false);
        assert_eq!(request.task.unwrap().status, Status::Running);
        let reviewing = reviewer();
        assert_eq!(request.run.unwrap().id, original.id);
        assert_eq!(reviewing.ended, None);
        assert_eq!(reviewing.head_sha.as_deref(), Some(previous_head.as_str()));
        assert!(!cx.runner.stops().contains(session));
        let worker = cx
            .engine
            .store
            .latest_run(&id, Role::Worker)
            .unwrap()
            .unwrap();
        assert!(cx.prompt(&worker).contains("revise the change"));

        fs::write(worker.worktree.join("file.txt"), "revised draft\n").unwrap();
        let head = cx.done(&id).run.unwrap().head_sha.unwrap();
        cx.engine.advance(&id).unwrap();
        cx.engine.advance(&id).unwrap();
        let starts = cx.runner.starts();
        let sends = cx.runner.sends();
        let requests = cx.events(&id, EventKind::ReviewStarted);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].detail["round"], 0);
        assert_eq!(requests[1].detail["round"], 1);
        assert_eq!(requests[1].actor, Actor::Auto);
        let active = reviewer();
        if accepts_send {
            assert_eq!(active.id, original.id);
            assert_eq!(starts.len() - starts_before, 3); // Two workers and one reviewer.
            let [send] = &sends[sends_before..] else {
                panic!("expected exactly one re-review request");
            };
            assert_eq!(&send.session, session);
            assert_eq!(send.origin, Origin::Auto);
            assert!(send.text.contains("Review round 1"));
            assert!(
                send.text
                    .contains(&format!("git diff {previous_head}..HEAD"))
            );
            assert!(send.text.contains(&finish));
        } else {
            assert_ne!(active.id, original.id);
            assert_eq!(starts.len() - starts_before, 4); // Two workers and two reviewers.
            assert_eq!(sends.len(), sends_before);
            assert!(cx.runner.stops().contains(session));
            assert!(cx.prompt(&active).contains("revise the change"));

            // Losing the reviewer after the request still yields an owner action.
            cx.runner
                .set_status(active.session.as_ref().unwrap(), SessionStatus::Gone);
            cx.apply(Op::Sync, false);
            let task = cx.task(&id);
            assert_eq!(task.status, Status::Blocked);
            let next = task.blocked.unwrap().next.remove(0);
            assert!(matches!(&next.op, Op::Review { id: task, .. } if task == &id));
            assert_eq!(
                cx.apply(next.op, next.force).task.unwrap().status,
                Status::Review
            );
            assert!(cx.prompt(&reviewer()).contains("revise the change"));
        }
        let active = reviewer();
        let landed = cx.apply(verdict(true, "looks good"), false);
        assert_eq!(landed.task.unwrap().status, Status::Done);
        assert!(landed.run.unwrap().ended.is_some());
        assert!(cx.runner.stops().contains(active.session.as_ref().unwrap()));
        assert_eq!(git(&cx.repo, &["branch", "--show-current"]), "local");
        cx.assert_landed(&worker, &head);
        assert_eq!(cx.events(&id, EventKind::Landed)[0].actor, Actor::Auto);
    }
}

#[test]
fn done_auto_lands_in_the_same_pipeline_while_worker_is_working() {
    let mut cx = Fixture::new(&["test -f file.txt"]);
    cx.project().auto_land = Some(true);
    cx.runner.set_default_status(SessionStatus::Working);
    let id = cx.backlog("auto-land a working session");
    let worker = cx.dispatch(&id);
    fs::write(worker.worktree.join("file.txt"), "auto-landed\n").unwrap();

    let landed = cx.done(&id);
    assert_eq!(landed.task.unwrap().status, Status::Done);
    cx.assert_landed(&worker, &landed.run.unwrap().head_sha.unwrap());
    assert_eq!(cx.runner.stops(), [worker.session.unwrap()]);
}

#[test]
fn review_releases_global_and_project_slots_with_worker_still_live() {
    let mut cx = Fixture::new(&[]);
    cx.engine.config.limits.running = 1;
    cx.project().running = Some(1);
    let first = cx.backlog("enter review");
    let run = cx.dispatch(&first);
    assert_eq!(cx.done(&first).task.unwrap().status, Status::Review);
    assert!(cx.runner.stops().is_empty());

    let second = cx.new_task("next ready task", Status::Ready, Priority::P2, vec![]);
    cx.project().auto_dispatch = Some(true);
    cx.apply(Op::Sync, false);
    assert_eq!(cx.task(&second).status, Status::Running);
    let live = cx.engine.store.live_runs_for_task(&first).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].session, run.session);
}

#[test]
fn unchanged_head_reuses_green_gate_and_rebase_conflict_sends_back() {
    let cx = Fixture::new(&["true"]);
    let id = cx.backlog("reuses green check");
    let first = cx.dispatch(&id);
    fs::write(first.worktree.join("unrelated.txt"), "task one\n").unwrap();
    let head = cx.done(&id).run.unwrap().head_sha.unwrap();
    cx.apply(approve(&id), false);
    cx.assert_landed(&first, &head);
    assert_eq!(cx.events(&id, EventKind::Gate).len(), 1);

    git(&cx.repo, &["branch", "release"]);
    let release_before = git(&cx.repo, &["rev-parse", "refs/heads/release"]);
    fs::write(cx.repo.join("main-only.txt"), "must not ship\n").unwrap();
    git(&cx.repo, &["add", "main-only.txt"]);
    git(&cx.repo, &["commit", "-m", "main-only commit"]);
    let main_before = git(&cx.repo, &["rev-parse", "refs/heads/main"]);
    let id = cx.backlog("retarget to an unchecked-out branch");
    let run = cx.dispatch(&id);
    fs::write(run.worktree.join("release-change.txt"), "ship this\n").unwrap();
    let submitted = cx.done(&id).run.unwrap().head_sha.unwrap();
    let result = cx
        .engine
        .apply(approve_onto(&id, Some("missing-release")), true)
        .unwrap_err();
    let refusal = as_refusal(&result).unwrap();
    assert!(refusal.reason.contains("missing-release"));
    assert!(
        refusal
            .next
            .contains(&crate::Next::force(approve_onto(&id, Some("release"))))
    );
    let landed = cx.apply(approve_onto(&id, Some("release")), false);
    assert_eq!(landed.task.unwrap().status, Status::Done);
    let landed = landed.run.unwrap();
    assert_eq!(landed.base, "main");
    let landed_sha = landed.head_sha.unwrap();
    assert_ne!(submitted, landed_sha);
    assert_ne!(release_before, landed_sha);
    assert_eq!(
        git(&cx.repo, &["rev-parse", "refs/heads/release"]),
        landed_sha
    );
    assert_eq!(
        git(&cx.repo, &["rev-parse", "refs/heads/main"]),
        main_before
    );
    assert_eq!(
        git(&cx.repo, &["show", "release:release-change.txt"]),
        "ship this"
    );
    assert!(
        !git_output(&cx.repo, &["cat-file", "-e", "release:main-only.txt"])
            .status
            .success()
    );
    assert_eq!(cx.events(&id, EventKind::Gate).len(), 2);
    assert_eq!(
        cx.events(&id, EventKind::Landed)[0].detail["base"],
        "release"
    );

    let id = cx.backlog("conflicting task");
    let run = cx.dispatch(&id);
    fs::write(run.worktree.join("file.txt"), "worker side\n").unwrap();
    let submitted = cx.done(&id).run.unwrap().head_sha.unwrap();
    fs::write(cx.repo.join("file.txt"), "base side\n").unwrap();
    git(&cx.repo, &["add", "file.txt"]);
    git(&cx.repo, &["commit", "-m", "competing change"]);
    let base = git(&cx.repo, &["rev-parse", "HEAD"]);
    let result = cx.apply(approve(&id), false);
    assert_eq!(result.task.unwrap().status, Status::Running);
    assert_eq!(git(&cx.repo, &["rev-parse", "HEAD"]), base);
    assert_eq!(git(&run.worktree, &["rev-parse", "HEAD"]), submitted);
    assert!(
        cx.runner.sends()[0]
            .text
            .starts_with("rebase onto main and resolve conflicts")
    );
    assert_eq!(cx.events(&id, EventKind::Gate).len(), 1);
}

#[test]
fn force_records_only_the_skipped_checks_and_stops_live_sessions() {
    let mut cx = Fixture::new(&["false"]);
    cx.engine.config.limits.running = 1;
    cx.project().running = Some(1);
    let running = cx.backlog("occupies slot");
    let run = cx.dispatch(&running);
    let dependency = cx.backlog("not done");
    let id = cx.new_task(
        "forced work",
        Status::Backlog,
        Priority::P2,
        vec![dependency],
    );
    let refused = cx.engine.apply(dispatch(&id), false).unwrap_err();
    assert!(
        as_refusal(&refused)
            .unwrap()
            .next
            .iter()
            .any(|n| n.force && n.op == dispatch(&id))
    );
    let forced = cx.apply(dispatch(&id), true).run.unwrap();
    assert_eq!(
        cx.events(&id, EventKind::Forced)[0].detail["skipped"],
        serde_json::json!(["dependencies", "limits.running", "project.running"])
    );
    fs::write(forced.worktree.join("file.txt"), "forced change\n").unwrap();
    let submitted = cx.done(&id);
    assert_eq!(submitted.task.unwrap().status, Status::Running);
    let head = submitted.run.unwrap().head_sha.unwrap();
    cx.runner
        .set_status(forced.session.as_ref().unwrap(), SessionStatus::Working);
    let refused = cx.engine.apply(approve(&id), false).unwrap_err();
    assert!(
        as_refusal(&refused)
            .unwrap()
            .next
            .iter()
            .any(|n| n.force && n.op == approve(&id))
    );
    cx.apply(approve(&id), true);
    cx.assert_landed(&forced, &head);
    assert_eq!(cx.events(&id, EventKind::Gate).len(), 1);
    assert_eq!(
        cx.events(&id, EventKind::Forced)[1].detail["skipped"],
        serde_json::json!(["status rule", "gate"])
    );
    cx.apply(
        Op::Move {
            id: running.clone(),
            status: Status::Done,
        },
        true,
    );
    assert_eq!(
        cx.runner.stops(),
        [forced.session.unwrap(), run.session.unwrap()]
    );
    assert!(run.worktree.exists());
    assert!(cx.branch_exists(&run.branch));
    assert_eq!(
        cx.events(&running, EventKind::Forced)[0].detail["skipped"],
        serde_json::json!(["status rule"])
    );
}

#[test]
fn sync_reconciles_questions_and_gone_sessions_then_dispatches_priority_and_age() {
    let mut cx = Fixture::new(&[]);
    cx.engine.config.limits.running = 2;
    let running = cx.backlog("waiting worker");
    let worker = cx.dispatch(&running);
    let session = worker.session.as_ref().unwrap();
    cx.apply(Op::Sync, false);
    assert!(
        cx.engine
            .store
            .get_run(&running, &worker.id)
            .unwrap()
            .unwrap()
            .quiet_since
            .is_some()
    );
    cx.runner.set_status(
        session,
        SessionStatus::Blocked {
            excerpt: "Which option?".into(),
        },
    );
    cx.apply(Op::Sync, false);
    let blocked = cx.task(&running).blocked.unwrap();
    assert_eq!(blocked.detail.as_deref(), Some("Which option?"));
    assert!(matches!(&blocked.next[0].op, Op::Reply { id, .. } if id == &running));
    cx.apply(
        Op::Reply {
            id: running.clone(),
            text: "option one".into(),
        },
        false,
    );
    assert_eq!(cx.task(&running).status, Status::Running);

    let low = cx.new_task("older low priority", Status::Ready, Priority::P3, vec![]);
    let high_older = cx.new_task("older high priority", Status::Ready, Priority::P1, vec![]);
    let high_newer = cx.new_task("newer high priority", Status::Ready, Priority::P1, vec![]);
    cx.project().auto_dispatch = Some(true);
    cx.runner.set_status(session, SessionStatus::Gone);
    cx.apply(Op::Sync, false);
    let blocked = cx.task(&running).blocked.unwrap();
    assert!(matches!(&blocked.next[0].op, Op::Retry { id, .. } if id == &running));
    let starts = cx.runner.starts();
    let order: Vec<_> = starts
        .iter()
        .map(|launch| launch.tokens[0].1.as_str())
        .collect();
    assert_eq!(order, [&running, &high_older]);
    assert_eq!(cx.task(&low).status, Status::Ready);
    assert_eq!(cx.task(&high_newer).status, Status::Ready);
    let refusal = cx.engine.apply(dispatch(&high_newer), false).unwrap_err();
    assert!(
        as_refusal(&refusal)
            .unwrap()
            .reason
            .contains("limits.running")
    );
    assert_eq!(
        cx.events(&high_older, EventKind::Dispatched)[0].actor,
        Actor::Auto
    );
}

#[test]
fn dispatch_recovers_checkout_and_run_files_after_launch_error() {
    let cx = Fixture::new(&[]);
    let id = cx.backlog("launch fails");
    cx.runner
        .set_start_hook(|_| anyhow::bail!("runner refused launch"));
    let before = cx.engine.store.latest_seq().unwrap();
    assert!(cx.engine.apply(dispatch(&id), false).is_err());
    assert_eq!(cx.task(&id).status, Status::Backlog);
    assert_eq!(cx.engine.store.latest_seq().unwrap(), before);
    assert!(cx.engine.store.list_runs(&id).unwrap().is_empty());
    assert!(!cx.repo.join(".worktrees").join(&id).exists());
    assert!(!cx.branch_exists(&format!("corral/{id}")));
    assert!(!cx.engine.state_dir.join("runs").join(&id).exists());
    cx.runner.clear_start_hook();
    git(&cx.repo, &["switch", "--detach"]);
    let error = cx.engine.apply(dispatch(&id), false).unwrap_err();
    let refusal = as_refusal(&error).unwrap();
    assert!(refusal.reason.contains("--base"));
    let [next] = refusal.next.as_slice() else {
        panic!("expected dispatch with the only local branch");
    };
    assert!(!next.force);
    assert!(next.command().ends_with(" --base main"));
    assert_eq!(cx.apply(next.op.clone(), false).run.unwrap().id, "R1");
}

#[test]
fn gate_spawn_failure_blocks_and_a_repaired_gate_resumes_review() {
    let mut cx = Fixture::new(&[]);
    cx.engine.pipeline = PipelineMode::Off;
    let id = cx.backlog("run a gate that cannot spawn");
    cx.dispatch(&id);
    cx.done(&id);
    cx.project().gate = vec!["invalid\0argument".into()];
    let op = Op::Gate { id: id.clone() };
    let failure = cx.engine.apply(op.clone(), false).unwrap_err();
    assert!(format!("{failure:#}").contains("run gate line"));
    let blocked = cx.task(&id).blocked.unwrap();
    assert_eq!(blocked.reason, "gate failed");
    assert!(blocked.detail.unwrap().contains("run gate line"));
    assert_eq!(blocked.next, vec![crate::Next::run(op.clone())]);
    assert_eq!(
        cx.events(&id, EventKind::Error)[0].detail["operation"],
        "gate"
    );
    cx.project().gate = vec!["true".into()];
    assert_eq!(cx.apply(op, false).task.unwrap().status, Status::Review);
}

#[test]
fn fast_forward_refused_by_git_blocks_and_can_be_retried() {
    let cx = Fixture::new(&[]);
    let id = cx.backlog("land after a conflicting local change");
    let run = cx.dispatch(&id);
    fs::write(run.worktree.join("file.txt"), "worker change\n").unwrap();
    cx.done(&id);
    let base = git(&cx.repo, &["rev-parse", "HEAD"]);
    fs::write(cx.repo.join("file.txt"), "uncommitted base change\n").unwrap();

    let denied = cx.apply(approve(&id), false).task.unwrap();
    assert_eq!(denied.status, Status::Blocked);
    let blocked = denied.blocked.unwrap();
    assert_eq!(blocked.reason, "approve failed");
    assert!(
        blocked
            .detail
            .unwrap()
            .contains("fast-forward landing target")
    );
    assert_eq!(
        blocked.next,
        vec![
            crate::Next::run(approve(&id)),
            crate::Next::force(approve(&id))
        ]
    );
    assert_eq!(
        cx.events(&id, EventKind::Error)[0].detail["operation"],
        "approve"
    );
    assert_eq!(git(&cx.repo, &["rev-parse", "HEAD"]), base);

    git(&cx.repo, &["restore", "file.txt"]);
    assert_eq!(
        cx.apply(approve(&id), false).task.unwrap().status,
        Status::Done
    );
}
