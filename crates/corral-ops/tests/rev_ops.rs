use std::path::{Path, PathBuf};
use std::process::Command;

use corral_core::{ArtifactKind, Run, RunMode, RunOutcome, Status, Store, Task};
use corral_ops::{
    ApproveStrategy, Ctx, Preview, add_artifact, all_artifacts, approve, artifact_preview, discard,
    done, infer_project, move_task, new_task, review_info, slots,
};

struct Ws {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    app: PathBuf,
    ctx: Ctx,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_file(repo: &Path, name: &str, text: &str, msg: &str) {
    std::fs::write(repo.join(name), text).unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", msg]);
}

fn workspace() -> Ws {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let app = root.join("app");
    std::fs::create_dir_all(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    git(&app, &["config", "user.name", "Corral Test"]);
    git(&app, &["config", "user.email", "corral@example.invalid"]);
    git(&app, &["config", "commit.gpgsign", "false"]);
    commit_file(&app, "README.md", "hello\n", "init");
    let store = Store::init(&root).unwrap();
    assert!(
        store.config.projects.contains_key("app"),
        "default config should detect app"
    );
    Ws {
        _tmp: tmp,
        root,
        app,
        ctx: Ctx::from_store(store),
    }
}

fn running_task(ws: &Ws, title: &str) -> Task {
    let task = new_task(
        &ws.ctx,
        title,
        Some("app"),
        &ws.root,
        Status::Ready,
        "spec\n",
    )
    .unwrap();
    let worktree = ws.app.join(".worktrees").join(&task.id);
    let branch = format!("corral/{}", task.id);
    corral_git::worktree_add(&ws.app, &worktree, &branch, "main").unwrap();
    let base_sha = corral_git::rev_sha(&ws.app, "main").unwrap();
    ws.ctx
        .store
        .update(&task.id, |t| {
            t.runs.push(Run {
                id: "R1".into(),
                agent: "generic".into(),
                harness: None,
                model: None,
                effort: None,
                mode: RunMode::Interactive,
                worktree: worktree.clone(),
                branch: branch.clone(),
                base: "main".into(),
                base_sha: base_sha.clone(),
                herdr: None,
                started: jiff::Timestamp::now(),
                ended: None,
                outcome: None,
                summary: None,
                blocked_excerpt: None,
                agent_state: None,
                seen_working: false,
            });
            t.status = Status::Running;
            Ok(())
        })
        .unwrap()
}

fn worktree(task: &Task) -> PathBuf {
    task.current_run().unwrap().worktree.clone()
}

fn finish(ws: &Ws, title: &str) -> Task {
    let task = running_task(ws, title);
    let wt = worktree(&task);
    std::fs::write(wt.join("feature.txt"), "hello feature\n").unwrap();
    done(&ws.ctx, &task.id, "Added feature.txt").unwrap()
}

#[test]
fn new_task_infers_project_from_cwd() {
    let ws = workspace();
    let sub = ws.app.join("src");
    std::fs::create_dir_all(&sub).unwrap();
    assert_eq!(infer_project(&ws.ctx, &sub).as_deref(), Some("app"));
    assert_eq!(infer_project(&ws.ctx, &ws.root), None);

    let t = new_task(&ws.ctx, "From app", None, &sub, Status::Backlog, "").unwrap();
    assert_eq!(t.project.as_deref(), Some("app"));
    let none = new_task(&ws.ctx, "From root", None, &ws.root, Status::Idea, "").unwrap();
    assert_eq!(none.project, None);

    let running = running_task(&ws, "Run");
    let wt = worktree(&running);
    assert_eq!(infer_project(&ws.ctx, &wt).as_deref(), Some("app"));
    let in_wt = new_task(&ws.ctx, "From worktree", None, &wt, Status::Backlog, "").unwrap();
    assert_eq!(in_wt.project.as_deref(), Some("app"));

    let err = new_task(&ws.ctx, "Bad", Some("nope"), &ws.root, Status::Backlog, "").unwrap_err();
    assert!(
        format!("{err:#}").contains("app"),
        "error should list known keys: {err:#}"
    );
}

#[test]
fn move_task_allowed_refused_forced() {
    let ws = workspace();
    let t = new_task(&ws.ctx, "Move me", Some("app"), &ws.root, Status::Idea, "").unwrap();
    assert_eq!(
        move_task(&ws.ctx, &t.id, Status::Backlog, false)
            .unwrap()
            .status,
        Status::Backlog
    );
    assert_eq!(
        move_task(&ws.ctx, &t.id, Status::Ready, false)
            .unwrap()
            .status,
        Status::Ready
    );
    let err = move_task(&ws.ctx, &t.id, Status::Running, false).unwrap_err();
    assert!(format!("{err:#}").contains("only dispatch"), "{err:#}");
    assert_eq!(ws.ctx.store.get(&t.id).unwrap().status, Status::Ready);
    assert_eq!(
        move_task(&ws.ctx, &t.id, Status::Review, true)
            .unwrap()
            .status,
        Status::Review
    );
    assert!(move_task(&ws.ctx, &t.id, Status::Done, false).is_err());
    assert!(move_task(&ws.ctx, &t.id, Status::Backlog, false).is_err());
    assert_eq!(
        move_task(&ws.ctx, &t.id, Status::Dropped, false)
            .unwrap()
            .status,
        Status::Dropped
    );
    let moved = ws
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "moved")
        .count();
    assert_eq!(moved, 4);
}

#[test]
fn slots_count_running_and_needs_me() {
    let ws = workspace();
    assert_eq!(slots(&ws.ctx).unwrap(), (0, 3));
    running_task(&ws, "One");
    let two = running_task(&ws, "Two");
    move_task(&ws.ctx, &two.id, Status::NeedsMe, true).unwrap();
    new_task(&ws.ctx, "Idle", None, &ws.root, Status::Ready, "").unwrap();
    assert_eq!(slots(&ws.ctx).unwrap(), (2, 3));
}

#[test]
fn done_snapshots_diff_summary_and_scanned_report() {
    let ws = workspace();
    let task = running_task(&ws, "Do work");
    let wt = worktree(&task);
    std::fs::write(wt.join("feature.txt"), "line one\nline two\n").unwrap();
    std::fs::create_dir_all(wt.join("docs/out")).unwrap();
    std::fs::write(wt.join("docs/out/report.md"), "# Report\nall good\n").unwrap();

    let t = done(&ws.ctx, &task.id, "Implemented the feature").unwrap();
    assert_eq!(t.status, Status::Review);
    let run = t.current_run().unwrap();
    assert_eq!(run.outcome, Some(RunOutcome::Done));
    assert_eq!(run.summary.as_deref(), Some("Implemented the feature"));
    assert!(run.ended.is_some());
    assert!(corral_git::is_clean(&wt).unwrap(), "done commits the work");

    let diff = t
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Diff)
        .unwrap();
    assert!(
        diff.title.starts_with("Final diff +4 \u{2212}0"),
        "{}",
        diff.title
    );
    let summary = t
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Summary)
        .unwrap();
    assert_eq!(summary.title, "Done summary");
    let report = t.artifacts.iter().find(|a| a.title == "report.md").unwrap();
    assert_eq!(report.kind, ArtifactKind::Report);
    assert_eq!(report.run, "R1");
    assert!(report.path.starts_with(&format!(
        "artifacts/{}/R1/{}-report.md",
        t.id,
        &report.sha256[..8]
    )));
    assert!(ws.ctx.store.dir().join(&report.path).is_file());
    assert_eq!(t.artifacts.len(), 3, "feature.txt does not match any glob");

    let diff_text = std::fs::read_to_string(ws.ctx.store.dir().join(&diff.path)).unwrap();
    assert!(diff_text.contains("+line one"));
    assert!(
        ws.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "done")
    );
    assert!(
        done(&ws.ctx, &task.id, "again").is_err(),
        "done only from Running/NeedsMe"
    );
}

#[test]
fn add_artifact_dedupes_and_refuses_large_files() {
    let ws = workspace();
    let task = new_task(&ws.ctx, "Arts", None, &ws.root, Status::Backlog, "").unwrap();
    let file = ws.root.join("notes.md");
    std::fs::write(&file, "notes\n").unwrap();
    let a = add_artifact(&ws.ctx, &task.id, &file, None, None).unwrap();
    assert_eq!(a.run, "R0");
    assert_eq!(a.title, "notes.md");
    let b = add_artifact(
        &ws.ctx,
        &task.id,
        &file,
        Some(ArtifactKind::Plan),
        Some("other"),
    )
    .unwrap();
    assert_eq!(a, b);
    assert_eq!(ws.ctx.store.get(&task.id).unwrap().artifacts.len(), 1);

    assert!(add_artifact(&ws.ctx, &task.id, &ws.root.join("missing.md"), None, None).is_err());
    assert!(add_artifact(&ws.ctx, &task.id, &ws.app, None, None).is_err());

    let mut small = ws.ctx.clone();
    small.store.config.artifacts.max_bytes = 4;
    let err = add_artifact(&small, &task.id, &file, None, None).unwrap_err();
    assert!(format!("{err:#}").contains("max_bytes"), "{err:#}");

    let rows = all_artifacts(&ws.ctx).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].task_id, task.id);
}

#[test]
fn all_artifacts_newest_first() {
    let ws = workspace();
    let t1 = new_task(&ws.ctx, "First", None, &ws.root, Status::Backlog, "").unwrap();
    let t2 = new_task(&ws.ctx, "Second", None, &ws.root, Status::Backlog, "").unwrap();
    let base = jiff::Timestamp::now();
    for (i, id) in [&t2.id, &t1.id].into_iter().enumerate() {
        let f = ws.root.join(format!("f{i}.md"));
        std::fs::write(&f, format!("{i}\n")).unwrap();
        add_artifact(&ws.ctx, id, &f, None, None).unwrap();
        let added = base + jiff::SignedDuration::from_secs(i as i64 * 60);
        ws.ctx
            .store
            .update(id, |t| {
                t.artifacts[0].added = added;
                Ok(())
            })
            .unwrap();
    }
    let rows = all_artifacts(&ws.ctx).unwrap();
    assert_eq!(
        rows.iter().map(|r| r.task_id.as_str()).collect::<Vec<_>>(),
        vec![t1.id.as_str(), t2.id.as_str()]
    );
}

#[test]
fn review_info_reports_stat_diff_and_main_ready() {
    let ws = workspace();
    let t = finish(&ws, "Review me");
    let info = review_info(&ws.ctx, &t.id).unwrap();
    assert_eq!(info.run.id, "R1");
    assert!(info.stat.files.iter().any(|f| f.path == "feature.txt"));
    assert_eq!(info.stat.added, 1);
    assert!(info.diff.contains("+hello feature"));
    assert_eq!(info.artifacts.len(), 2);
    assert!(info.main_ready);

    std::fs::write(ws.app.join("README.md"), "dirty\n").unwrap();
    assert!(!review_info(&ws.ctx, &t.id).unwrap().main_ready);

    let norun = new_task(&ws.ctx, "No run", None, &ws.root, Status::Backlog, "").unwrap();
    assert!(review_info(&ws.ctx, &norun.id).is_err());
}

#[test]
fn approve_rebase_after_main_advanced() {
    let ws = workspace();
    let t = finish(&ws, "Rebase me");
    commit_file(&ws.app, "other.txt", "main moved\n", "main advanced");
    let msg = approve(&ws.ctx, &t.id, ApproveStrategy::Rebase, true).unwrap();
    assert!(
        msg.starts_with(&format!("rebased corral/{} onto main at ", t.id)),
        "{msg}"
    );
    assert!(msg.ends_with("worktree removed"), "{msg}");
    assert!(
        ws.app.join("feature.txt").is_file(),
        "run commit landed in the main checkout"
    );
    assert!(ws.app.join("other.txt").is_file());
    assert!(
        git(&ws.app, &["log", "--format=%s", "main"])
            .contains(&format!("{}: Rebase me (corral run R1)", t.id))
    );
    assert!(!worktree(&t).exists());
    assert_eq!(ws.ctx.store.get(&t.id).unwrap().status, Status::Done);
    let ev = ws
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "approved")
        .unwrap();
    assert_eq!(
        ev.detail["sha"].as_str().unwrap(),
        corral_git::rev_sha(&ws.app, "main").unwrap()
    );
}

#[test]
fn approve_fast_forward() {
    let ws = workspace();
    let t = finish(&ws, "FF me");
    let msg = approve(&ws.ctx, &t.id, ApproveStrategy::FastForward, true).unwrap();
    assert!(msg.starts_with("fast-forwarded main"), "{msg}");
    assert!(ws.app.join("feature.txt").is_file());
    assert!(!worktree(&t).exists());
    assert_eq!(ws.ctx.store.get(&t.id).unwrap().status, Status::Done);
    assert!(
        approve(&ws.ctx, &t.id, ApproveStrategy::FastForward, true).is_err(),
        "only from Review"
    );
}

#[test]
fn approve_keep_branch() {
    let ws = workspace();
    let t = finish(&ws, "Keep me");
    let main_before = corral_git::rev_sha(&ws.app, "main").unwrap();
    let msg = approve(&ws.ctx, &t.id, ApproveStrategy::KeepBranch, false).unwrap();
    assert!(msg.contains("kept branch"), "{msg}");
    assert!(msg.ends_with("worktree kept"), "{msg}");
    assert_eq!(corral_git::rev_sha(&ws.app, "main").unwrap(), main_before);
    assert!(!ws.app.join("feature.txt").exists());
    assert!(worktree(&t).exists());
    assert_eq!(ws.ctx.store.get(&t.id).unwrap().status, Status::Done);
}

#[test]
fn discard_drops_and_keeps_worktree() {
    let ws = workspace();
    let t = running_task(&ws, "Throw away");
    let d = discard(&ws.ctx, &t.id).unwrap();
    assert_eq!(d.status, Status::Dropped);
    assert_eq!(
        d.current_run().unwrap().outcome,
        Some(RunOutcome::Discarded)
    );
    assert!(d.current_run().unwrap().ended.is_some());
    assert!(worktree(&t).exists());
    assert!(
        ws.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "discarded")
    );
}

#[test]
fn artifact_preview_text_and_external() {
    let ws = workspace();
    let task = new_task(&ws.ctx, "Preview", None, &ws.root, Status::Backlog, "").unwrap();
    let md = ws.root.join("plan.md");
    std::fs::write(&md, "# Plan\nstep\n").unwrap();
    let png = ws.root.join("shot.png");
    std::fs::write(&png, [0x89u8, b'P', b'N', b'G', 0, 1, 2]).unwrap();
    let a = add_artifact(&ws.ctx, &task.id, &md, None, None).unwrap();
    assert_eq!(
        artifact_preview(&ws.ctx, &a).unwrap(),
        Preview::Text("# Plan\nstep\n".into())
    );
    let b = add_artifact(&ws.ctx, &task.id, &png, None, None).unwrap();
    match artifact_preview(&ws.ctx, &b).unwrap() {
        Preview::External { path, .. } => {
            assert!(Path::new(&path).is_absolute() && path.ends_with("-shot.png"))
        }
        other => panic!("expected External, got {other:?}"),
    }
}
