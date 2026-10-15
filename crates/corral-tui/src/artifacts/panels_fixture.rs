use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use corral_core::{ArtifactKind, Run, RunMode, RunOutcome, Status, Store};
use corral_ops::Ctx;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::View;

static NEXT: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct Fixture {
    pub root: PathBuf,
    pub ctx: Ctx,
    pub id: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(dir: &Path, args: &[&str]) {
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
}

pub(crate) fn fixture() -> Fixture {
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "corral-tui-panels-{}-{nanos}-{n}",
        std::process::id()
    ));
    let app = root.join("app");
    std::fs::create_dir_all(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    git(&app, &["config", "user.name", "corral test"]);
    git(&app, &["config", "user.email", "corral@example.invalid"]);
    std::fs::write(app.join("README.md"), "# app\n").unwrap();
    git(&app, &["add", "-A"]);
    git(&app, &["commit", "-q", "-m", "init"]);

    let mut store = Store::init(&root).unwrap();
    store.config.herdr.session = Some("corral-test".into());
    let ctx = Ctx::from_store(store);
    let task = ctx
        .store
        .create(
            "Witness shard cache",
            Some("app"),
            Status::Ready,
            "## Goal\nCache shard reads per character.\n",
        )
        .unwrap();
    let id = task.id.clone();

    let base_sha = corral_git::rev_sha(&app, "main").unwrap();
    let branch = format!("corral/{id}");
    let worktree = app.join(".worktrees").join(&id);
    corral_git::worktree_add(&app, &worktree, &branch, "main").unwrap();
    std::fs::write(
        worktree.join("shard_cache.rs"),
        "pub struct ShardCache;\n\nimpl ShardCache {}\n",
    )
    .unwrap();
    assert!(corral_git::commit_all(&worktree, "add shard cache").unwrap());

    let now = "2026-01-01T12:00:00Z".parse().unwrap();
    let run = Run {
        id: "R1".into(),
        agent: "omo".into(),
        harness: None,
        model: None,
        effort: None,
        mode: RunMode::Interactive,
        worktree: worktree.clone(),
        branch,
        base: "main".into(),
        base_sha,
        herdr: None,
        started: now,
        ended: Some(now),
        outcome: Some(RunOutcome::Done),
        summary: Some("Added the shard cache.\n✓ 3/3 cache tests".into()),
        blocked_excerpt: None,
        agent_state: None,
        seen_working: true,
        gate: None,
        idle_since: None,
        idle_no_report: false,
        review: None,
        usage: None,
        usage_note: None,
        autopilot: None,
        stuck: None,
        pending_reply: None,
        herdr_idle_since: None,
    };
    ctx.store
        .update(&id, |t| {
            t.status = Status::Review;
            t.runs.push(run);
            Ok(())
        })
        .unwrap();

    let report = root.join("qa-report.md");
    std::fs::write(&report, "# QA checklist\n\nall green\n").unwrap();
    corral_ops::add_artifact(
        &ctx,
        &id,
        &report,
        Some(ArtifactKind::Report),
        Some("QA checklist"),
    )
    .unwrap();
    Fixture { root, ctx, id }
}

pub(crate) fn render(view: &mut dyn View, ctx: &Ctx) -> String {
    let (w, h) = (160u16, 45u16);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        let area = f.area();
        view.draw(f, area, ctx, &mut crate::HitMap::default());
    })
    .unwrap();
    let buf = term.backend().buffer();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf.cell((x, y)).map_or(" ", |c| c.symbol()));
        }
        out.push('\n');
    }
    out
}

pub(crate) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
