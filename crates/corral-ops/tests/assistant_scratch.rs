//! scratch-worktrees (T-544 C8, C13 and the promote half of C10) through the ops API.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use corral_core::{ArtifactKind, ArtifactTier, DiskSpace, ResourceSnapshot, Status, Store};
use corral_herdr::Herdr;
use corral_ops::assistant::scratch;
use corral_ops::resources::FixedResourceProbe;
use corral_ops::{Ctx, artifact_read};
use jiff::{SignedDuration, Timestamp};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Scratch Test",
            "-c",
            "user.email=scratch@test",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    ctx: Ctx,
}

impl Fixture {
    /// A workspace whose project `corral` is the git repository at its root, with sessions
    /// A-1 and A-2, a healthy fixed resource probe and no terminal server.
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("ws")).unwrap();
        let root = temp.path().join("ws").canonicalize().unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("README.md"), "base\n").unwrap();
        std::fs::write(root.join(".gitignore"), ".corral/\n.worktrees/\n*.log\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "base"]);
        Store::init(&root).unwrap();
        let config = root.join(".corral/corral.toml");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str("\n[projects.corral]\npath = '.'\n");
        std::fs::write(&config, text).unwrap();
        let store = Store::open(&root).unwrap();
        store.create_assistant("pi", None, None, None).unwrap();
        store.create_assistant("pi", None, None, None).unwrap();
        let ctx = Ctx {
            herdr: Herdr::with_bin(PathBuf::from("/nonexistent/herdr"), None),
            ..FixedResourceProbe::ctx(store)
        };
        Self {
            _temp: temp,
            root,
            ctx,
        }
    }

    fn create(&self, session: &str, slug: &str) -> corral_core::assistant::AssistantScratch {
        scratch::create(&self.ctx, session, "corral", Some(slug), None).unwrap()
    }

    fn commit(&self, path: &Path, file: &str, body: &str) {
        std::fs::write(path.join(file), body).unwrap();
        git(path, &["add", file]);
        git(path, &["commit", "-q", "-m", &format!("add {file}")]);
    }

    fn saved(&self, session: &str, path: &str) -> (corral_core::Artifact, String) {
        let artifact = self
            .ctx
            .store
            .get_assistant(session)
            .unwrap()
            .artifacts
            .into_iter()
            .find(|a| a.path == path)
            .unwrap();
        let bytes = artifact_read(&self.ctx, &artifact).unwrap();
        (artifact, String::from_utf8(bytes).unwrap())
    }

    fn branch_exists(&self, branch: &str) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .status()
            .unwrap()
            .success()
    }

    fn record(&self, session: &str) -> Vec<u8> {
        std::fs::read(self.ctx.store.assistant_path(session).unwrap()).unwrap()
    }
}

#[test]
fn c8_scratch_lifecycle_accounting_and_idle_gc() {
    // Given A-1's scratch spike-cache with one commit beyond its base.
    let fx = Fixture::new();
    let made = fx.create("A-1", "spike-cache");
    assert_eq!(made.path, fx.root.join(".worktrees/A-1-spike-cache"));
    assert_eq!(made.branch, "corral/A-1/spike-cache");
    assert_eq!(made.kind, "spike");
    assert_eq!(made.base, git(&fx.root, &["rev-parse", "main"]));
    fx.commit(&made.path, "cache.rs", "fn cache() {}\n");
    std::fs::create_dir_all(made.path.join("node_modules/dep")).unwrap();
    std::fs::write(
        made.path.join("node_modules/dep/index.js"),
        "x".repeat(4096),
    )
    .unwrap();
    // Then storage accounting measures it, keeping dependency trees apart.
    let rows = scratch::storage(&fx.ctx).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].measured.bytes > 0, "{rows:?}");
    assert_eq!(rows[0].measured.dependency_bytes, 4096);
    // When the session ends, GC keeps it until the idle period has passed.
    fx.ctx.store.end_assistant("A-1").unwrap();
    let now = Timestamp::now();
    let early = scratch::gc(&fx.ctx, now, true).unwrap();
    assert_eq!(early.items[0].action, "skip");
    assert!(
        early.items[0].reason.starts_with("idle 0d of 7d"),
        "{early:?}"
    );
    // The dry run after the idle period lists it and changes nothing.
    let later = now.checked_add(SignedDuration::from_hours(8 * 24)).unwrap();
    let before = fx.record("A-1");
    let dry = scratch::gc(&fx.ctx, later, true).unwrap();
    assert_eq!(dry.items[0].action, "remove", "{dry:?}");
    assert!(dry.items[0].preserve);
    assert_eq!(fx.record("A-1"), before);
    assert!(made.path.is_dir() && fx.branch_exists(&made.branch));
    // The real run saves a diff artifact on A-1, then removes the worktree and branch.
    let real = scratch::gc(&fx.ctx, later, false).unwrap();
    assert_eq!(real.items[0].action, "removed", "{real:?}");
    let saved = real.items[0].artifact.clone().unwrap();
    let (artifact, text) = fx.saved("A-1", &saved);
    assert_eq!(artifact.kind, ArtifactKind::Diff);
    assert_eq!(artifact.tier_override, Some(ArtifactTier::Keep));
    assert!(text.contains("+fn cache() {}"), "{text}");
    assert!(!made.path.exists() && !fx.branch_exists(&made.branch));
    assert!(
        fx.ctx
            .store
            .get_assistant("A-1")
            .unwrap()
            .scratch
            .is_empty()
    );
}

#[test]
fn gc_skips_a_scratch_with_a_live_process_cwd_or_a_pending_finish() {
    // Given two idle scratch of ended A-1: one a live process uses, one marked for co-work.
    let fx = Fixture::new();
    let busy = fx.create("A-1", "busy");
    let cowork = fx.create("A-1", "finishing");
    fx.ctx
        .store
        .update_assistant("A-1", |s| {
            s.scratch
                .iter_mut()
                .find(|x| x.slug == "finishing")
                .unwrap()
                .kind = scratch::COWORK.into();
            Ok(())
        })
        .unwrap();
    fx.ctx.store.end_assistant("A-1").unwrap();
    // The child has entered the scratch directory once spawn returns (it has exec'd).
    let mut resident = Command::new("cat")
        .current_dir(&busy.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let later = Timestamp::now()
        .checked_add(SignedDuration::from_hours(30 * 24))
        .unwrap();
    // When the owner collects, then both are skipped with their reasons and kept.
    let report = scratch::gc(&fx.ctx, later, false).unwrap();
    drop(resident.stdin.take());
    resident.wait().unwrap();
    let reason = |slug: &str| {
        let item = report.items.iter().find(|i| i.slug == slug).unwrap();
        assert_eq!(item.action, "skip", "{item:?}");
        item.reason.clone()
    };
    assert!(reason("busy").starts_with("live process cwd"), "{report:?}");
    assert_eq!(reason("finishing"), "pending finish: a co-work finish");
    assert!(busy.path.is_dir() && cowork.path.is_dir());
    let kept = fx.ctx.store.get_assistant("A-1").unwrap().scratch;
    assert_eq!(kept.len(), 2);
    assert!(
        kept.iter()
            .all(|s| s.bytes.is_some() && s.measured_at == Some(later))
    );
}

#[test]
fn c13_discard_saves_committed_tracked_and_untracked_work_first() {
    // Given A-1 scratch with a commit beyond base, with only uncommitted edits, and empty.
    let mut fx = Fixture::new();
    let committed = fx.create("A-1", "committed");
    fx.commit(&committed.path, "feature.rs", "fn feature() {}\n");
    let dirty = fx.create("A-1", "dirty");
    std::fs::write(dirty.path.join("README.md"), "base\ntracked edit\n").unwrap();
    std::fs::write(dirty.path.join("notes.txt"), "untracked note\n").unwrap();
    std::fs::write(dirty.path.join("ignored.log"), "ignored\n").unwrap();
    let empty = fx.create("A-1", "empty");
    // When A-1 discards each, then every one is removed with its branch.
    let first = scratch::discard(&fx.ctx, "A-1", "committed").unwrap();
    assert_eq!(first.commits, 1);
    let (_, text) = fx.saved("A-1", first.artifact.as_deref().unwrap());
    assert!(text.contains("+fn feature() {}") && text.contains("add feature.rs"));
    let second = scratch::discard(&fx.ctx, "A-1", "dirty").unwrap();
    assert!(second.uncommitted && second.commits == 0);
    let (artifact, text) = fx.saved("A-1", second.artifact.as_deref().unwrap());
    assert!(text.contains("+tracked edit") && text.contains("+untracked note"));
    assert!(!text.contains("ignored.log"), "{text}");
    // The saved patch restores the work onto the base.
    let patch = fx.root.join("restore.diff");
    std::fs::write(&patch, &text).unwrap();
    git(
        &fx.root,
        &["apply", "--check", "--binary", patch.to_str().unwrap()],
    );
    let third = scratch::discard(&fx.ctx, "A-1", "empty").unwrap();
    assert_eq!(third.artifact, None);
    for gone in [&committed, &dirty, &empty] {
        assert!(!gone.path.exists() && !fx.branch_exists(&gone.branch));
    }
    assert!(
        fx.ctx
            .store
            .get_assistant("A-1")
            .unwrap()
            .scratch
            .is_empty()
    );
    // Both saved diffs stay protected after the later one was added.
    let artifacts = fx.ctx.store.get_assistant("A-1").unwrap().artifacts;
    assert_eq!(artifacts.len(), 2);
    assert!(
        artifacts
            .iter()
            .all(|a| a.tier_override == Some(ArtifactTier::Keep))
    );
    assert_eq!(artifact.kind, ArtifactKind::Diff);
    // When saving fails, then nothing is removed.
    let kept = fx.create("A-1", "kept");
    std::fs::write(kept.path.join("big.txt"), "x".repeat(4096)).unwrap();
    fx.ctx.store.config.artifacts.max_bytes = 64;
    let error = scratch::discard(&fx.ctx, "A-1", "kept").unwrap_err();
    assert!(
        format!("{error:#}").contains("nothing was removed"),
        "{error:#}"
    );
    assert!(kept.path.join("big.txt").is_file() && fx.branch_exists(&kept.branch));
    assert_eq!(fx.ctx.store.get_assistant("A-1").unwrap().scratch.len(), 1);
}

#[test]
fn c10_promote_files_a_backlog_task_with_the_diff_and_leaves_the_scratch() {
    // Given A-1's spike with a commit and an uncommitted edit.
    let fx = Fixture::new();
    let spike = fx.create("A-1", "spike");
    let empty = fx.create("A-1", "nothing");
    fx.commit(&spike.path, "cache.rs", "fn cache() {}\n");
    std::fs::write(spike.path.join("draft.txt"), "draft\n").unwrap();
    let head = git(&spike.path, &["rev-parse", "HEAD"]);
    let record = fx.record("A-1");
    // When it is promoted.
    let promoted =
        scratch::promote(&fx.ctx, "A-1", "spike", "Cache layer", None, "Why: speed.").unwrap();
    // Then a Backlog task carries the spike diff, and the scratch is untouched.
    let task = fx.ctx.store.get(&promoted.task).unwrap();
    assert_eq!(task.status, Status::Backlog);
    assert_eq!(task.project.as_deref(), Some("corral"));
    assert!(task.body.contains("Why: speed.") && task.body.contains("corral/A-1/spike"));
    assert_eq!(task.artifacts.len(), 1);
    assert_eq!(task.artifacts[0].kind, ArtifactKind::Diff);
    let text = String::from_utf8(artifact_read(&fx.ctx, &task.artifacts[0]).unwrap()).unwrap();
    assert!(text.contains("+fn cache() {}") && text.contains("+draft"));
    assert_eq!(git(&spike.path, &["rev-parse", "HEAD"]), head);
    assert!(spike.path.join("draft.txt").is_file());
    assert_eq!(fx.record("A-1"), record);
    // A scratch with nothing beyond its base files nothing.
    let error = scratch::promote(&fx.ctx, "A-1", "nothing", "Empty", None, "").unwrap_err();
    assert!(error.to_string().starts_with("scratch_empty"), "{error}");
    assert!(empty.path.is_dir());
    assert_eq!(fx.ctx.store.list().unwrap().len(), 1);
}

#[test]
fn creation_refuses_the_cap_low_disk_and_ended_sessions() {
    let mut fx = Fixture::new();
    fx.ctx.store.config.assistant.scratch_max = 3;
    for n in 0..3 {
        let session = if n % 2 == 0 { "A-1" } else { "A-2" };
        scratch::create(&fx.ctx, session, "corral", None, None).unwrap();
    }
    let error = scratch::create(&fx.ctx, "A-1", "corral", Some("fourth"), None).unwrap_err();
    assert!(error.to_string().starts_with("scratch_cap"), "{error}");
    assert!(!fx.root.join(".worktrees/A-1-fourth").exists());
    // Disk below resources.min_disk_free refuses before any worktree exists.
    let fresh = Fixture::new();
    let low = Ctx {
        resource_probe: std::sync::Arc::new(FixedResourceProbe(ResourceSnapshot {
            disks: vec![DiskSpace {
                path: PathBuf::from("/"),
                free_bytes: 0,
            }],
            ..FixedResourceProbe::healthy().0
        })),
        ..fresh.ctx.clone()
    };
    let error = scratch::create(&low, "A-1", "corral", Some("x"), None).unwrap_err();
    assert!(error.to_string().starts_with("disk_low"), "{error}");
    assert!(!fresh.root.join(".worktrees/A-1-x").exists());
    fresh.ctx.store.end_assistant("A-2").unwrap();
    let error = scratch::create(&fresh.ctx, "A-2", "corral", Some("y"), None).unwrap_err();
    assert!(error.to_string().starts_with("assistant_ended"), "{error}");
}

#[test]
fn orphans_list_scratch_directories_without_a_session_record() {
    let fx = Fixture::new();
    fx.create("A-1", "recorded");
    std::fs::create_dir_all(fx.root.join(".worktrees/A-9-lost")).unwrap();
    std::fs::create_dir_all(fx.root.join(".worktrees/A-2-unlisted")).unwrap();
    std::fs::create_dir_all(fx.root.join(".worktrees/T-001")).unwrap();
    let orphans = scratch::orphans(&fx.ctx.store).unwrap();
    assert_eq!(orphans.len(), 2, "{orphans:?}");
    assert!(
        orphans
            .iter()
            .any(|o| o.contains("A-9-lost") && o.contains("A-9 is missing"))
    );
    assert!(
        orphans
            .iter()
            .any(|o| o.contains("A-2-unlisted") && o.contains("not recorded by A-2"))
    );
}
