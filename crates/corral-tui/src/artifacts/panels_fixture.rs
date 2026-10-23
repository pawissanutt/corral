use std::path::{Path, PathBuf};
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
    let mut command = crate::board::tests::fixture_command("git", dir);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("GIT_")
            || name.starts_with("CORRAL_GATE_")
            || name.starts_with("HERDR_")
        {
            command.env_remove(key);
        }
    }
    let out = command.arg("-C").arg(dir).args(args).output().unwrap();
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
    let ctx = crate::board::tests::fixture_ctx(store);
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
        execution: None,
        delivery: corral_core::inbox::DeliveryMode::Legacy,
        inbox: Default::default(),
        completion: Default::default(),
        id: "R1".into(),
        progress: None,
        checkpoints: vec![],
        checkpoint_required: false,
        agent: "omo".into(),
        profile: None,
        harness: None,
        model: None,
        effort: None,
        route: None,
        vendor: None,
        failures: 0,
        switches: vec![],
        retry: None,
        mode: RunMode::Interactive,
        worktree: worktree.clone(),
        branch,
        base: "main".into(),
        base_sha,
        plan_target: None,
        herdr: None,
        started: now,
        ended: Some(now),
        outcome: Some(RunOutcome::Done),
        summary: Some("Added the shard cache.\n✓ 3/3 cache tests".into()),
        blocked_excerpt: None,
        agent_state: None,
        seen_pane: false,
        seen_working: true,
        gate: None,
        idle_since: None,
        idle_no_report: false,
        review: None,
        review_runs: Vec::new(),
        usage: None,
        usage_note: None,
        autopilot: None,
        stuck: None,
        pending_reply: None,
        obligations: Default::default(),
        herdr_idle_since: None,
        gate_rules: corral_core::GateRules::Legacy,
        promotion_origin: None,
        agent_host: None,
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
    render_at(view, ctx, 160, 45).0
}

/// Draws `view` on a `w` x `h` TestBackend; returns the screen text (one line per row) and the frame's hits.
pub(crate) fn render_at(view: &mut dyn View, ctx: &Ctx, w: u16, h: u16) -> (String, crate::HitMap) {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = crate::HitMap::default();
    term.draw(|f| {
        let area = f.area();
        view.draw(f, area, ctx, &mut hits);
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
    (out, hits)
}

pub(crate) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
