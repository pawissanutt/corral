//! Small filesystem/contract fixtures, not live engine journeys or host load tests.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use corral_core::{Project, Run};
use serde_json::json;

use super::*;

fn git(dir: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    let status = command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    task: String,
    worktree: PathBuf,
    now: Timestamp,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let repo = root.join("app");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join(".gitignore"), "/target/\n/web/dist/\n").unwrap();
        std::fs::write(repo.join("source.rs"), "unfinished source\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "fixture"]);
        let mut store = Store::init(&root).unwrap();
        store.config.projects.insert(
            "app".into(),
            Project {
                path: "app".into(),
                dispatch: Default::default(),
                default_profile: None,
                base: None,
                review: Default::default(),
                artifact_retention: None,
                post_land: vec![],
            },
        );
        let cargo = OutputCfg {
            kind: OutputKind::CargoTarget,
            path: "target".into(),
            rebuildable: true,
            exclusive: true,
            grace_hours: 24,
            extensions: vec![],
        };
        let web = OutputCfg {
            kind: OutputKind::Generated,
            path: "web/dist".into(),
            extensions: vec!["js".into(), "css".into(), "html".into()],
            ..cargo.clone()
        };
        store.config.storage.outputs.insert(
            "app".into(),
            [("cargo".into(), cargo), ("web".into(), web)].into(),
        );
        store.config.storage.cache_root = Some(root.join(".cache"));
        store.config.storage.scratch_root = Some(root.join(".scratch"));
        store.save_config().unwrap();
        let task = store
            .create("paused goal", Some("app"), Status::Running, "")
            .unwrap()
            .id;
        let worktree = repo.join(".worktrees").join(&task);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-qb",
                &format!("corral/{task}"),
                worktree.to_str().unwrap(),
                "main",
            ],
        );
        let at = store.now();
        let run: Run = serde_json::from_value(json!({
            "id":"R1", "agent":"fake", "worktree":worktree, "branch":format!("corral/{task}"),
            "base":"main", "base_sha":"0".repeat(40), "started":at, "failures":2,
            "gate":{"result":"pass", "at":at, "sha":"0".repeat(40)},
            "review":{"verdict":"changes", "reviewer":"fixture", "at":at, "sha":"0".repeat(40)},
        }))
        .unwrap();
        store
            .update(&task, |t| {
                t.runs.push(run);
                t.status = Status::NeedsMe;
                t.capacity_started = Some(true);
                t.status_since = Some(at);
                Ok(())
            })
            .unwrap();
        let ctx = Ctx::from_store(store);
        let fx = Self {
            _tmp: tmp,
            ctx,
            task,
            worktree,
            now: at.checked_add(SignedDuration::from_hours(25)).unwrap(),
        };
        // Registration is the same before-send entry point as operating::{submit,submit_head}.
        let _task = fx.ctx.store.try_lock(&fx.task).unwrap().unwrap();
        fx.ctx
            .store
            .records_transaction(|| prepare(&fx.ctx.store, &fx.ctx.store.head(&fx.task)?))
            .unwrap();
        fx
    }

    fn owner(&self, class: &str) -> Owner {
        OWNERS
            .load(&self.ctx.store, &key(&self.task, class))
            .unwrap()
            .unwrap()
    }
    fn cfg(&self, class: &str) -> &OutputCfg {
        &self.ctx.store.config.storage.outputs["app"][class]
    }
    fn head(&self) -> TaskHead {
        self.ctx.store.head(&self.task).unwrap()
    }
    fn assess(&self, class: &str) -> OutputEntry {
        assess(
            &self.ctx,
            &self.head(),
            class,
            self.cfg(class),
            Some(&self.owner(class)),
            self.now,
            &|_, _, _| Ok(()),
        )
    }
    fn reclaim(&self, class: &str) -> OutputEntry {
        let _task = self.ctx.store.try_lock(&self.task).unwrap().unwrap();
        let owner = self.owner(class);
        let previous = self
            .ctx
            .store
            .lifecycle_row(&self.task, &owner.generation(), &target(class))
            .unwrap();
        reclaim(
            &self.ctx,
            &self.head(),
            Registered {
                class,
                cfg: self.cfg(class),
                owner: &owner,
            },
            previous.as_ref(),
            self.now,
            &super::super::lifecycle::policy_digest(&self.ctx).unwrap(),
            &|_, _, _| Ok(()),
        )
        .unwrap()
    }
    fn build(&self) {
        std::fs::write(
            self.worktree.join("target/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        std::fs::write(self.worktree.join("target/app.bin"), b"cargo").unwrap();
        std::fs::write(self.worktree.join("web/dist/app.js"), b"web").unwrap();
    }
}

#[test]
fn shared_policy_reclaims_both_types_without_touching_unfinished_work_or_lineage() {
    let fx = Fx::new();
    fx.build();
    std::fs::write(fx.worktree.join("source.rs"), "dirty unfinished source\n").unwrap();
    std::fs::create_dir(fx.worktree.join(".evidence")).unwrap();
    std::fs::write(fx.worktree.join(".evidence/proof.md"), "proof").unwrap();
    let before = fx.head();
    let cargo = fx.assess("cargo");
    assert_eq!(
        cargo.disposition,
        Disposition::Reclaimable,
        "{}",
        cargo.reason
    );
    assert_eq!(cargo.measured_bytes, Some(49)); // 44-byte tag + 5-byte artifact
    let mut report = super::super::GcReport {
        dry_run: true,
        entries: vec![],
        totals: Default::default(),
    };
    let mut containing = super::super::entry(
        "worktree",
        fx.worktree.display().to_string(),
        100,
        Disposition::Retained,
        None,
    );
    containing.task = Some(fx.task.clone());
    report.push(containing);
    report.push_output(cargo);
    report.push_output(fx.assess("web"));
    report.total();
    assert_eq!(report.totals[&super::super::Bucket::Retained], 48);
    assert_eq!(report.totals[&super::super::Bucket::Reclaimed], 52);
    assert!(fx.worktree.join("target/app.bin").exists()); // readback has no effects
    assert_eq!(fx.reclaim("cargo").reclaimed_bytes, 49);
    assert_eq!(fx.reclaim("web").reclaimed_bytes, 3);
    let readback = output_gc(&fx.ctx, true).unwrap();
    assert_eq!(readback.len(), 2);
    assert!(
        readback
            .iter()
            .all(|row| row.disposition == Disposition::Reported && row.reclaimed_bytes == 0)
    );
    assert_eq!(
        readback
            .iter()
            .map(|row| row.last_reclaimed_bytes.unwrap())
            .sum::<u64>(),
        52
    );
    println!(
        "cached output readback: {}",
        serde_json::to_string(&readback).unwrap()
    );
    assert!(!fx.worktree.join("target").exists());
    assert!(!fx.worktree.join("web/dist").exists());
    assert_eq!(
        std::fs::read_to_string(fx.worktree.join("source.rs")).unwrap(),
        "dirty unfinished source\n"
    );
    assert_eq!(
        std::fs::read_to_string(fx.worktree.join(".evidence/proof.md")).unwrap(),
        "proof"
    );
    assert!(fx.worktree.join(".git").is_file());
    assert_eq!(before, fx.head());
    // Resume recreates only the outputs, with a new epoch. No goal/run/capacity mutation.
    let old_epoch = fx.owner("cargo").epoch;
    let mut absent_old = fx.owner("cargo");
    absent_old.root_ino += 1; // A newly recreated worktree must not inherit stale root identity.
    OWNERS
        .save(&fx.ctx.store, &key(&fx.task, "cargo"), &absent_old)
        .unwrap();
    let _task = fx.ctx.store.try_lock(&fx.task).unwrap().unwrap();
    prepare(&fx.ctx.store, &fx.head()).unwrap();
    assert_ne!(old_epoch, fx.owner("cargo").epoch);
    assert!(fx.worktree.join("target").is_dir());
    assert_eq!(before, fx.head());
}

#[test]
fn preservation_and_unknown_ownership_refusals_share_one_contract() {
    let fx = Fx::new();
    fx.build();
    for (class, file) in [
        ("cargo", "source.rs"),
        ("web", "report.html"),
        ("web", "proof.md"),
        ("cargo", "credentials.json"),
    ] {
        let path = fx.owner(class).path.join(file);
        std::fs::write(&path, b"must keep").unwrap();
        let line = fx.assess(class);
        assert_eq!(line.disposition, Disposition::Deferred, "{file}");
        assert!(path.exists());
        std::fs::remove_file(path).unwrap();
    }
    let package = fx.worktree.join("target/debug/build/fixture-1234");
    let generated = package.join("out");
    std::fs::create_dir_all(&generated).unwrap();
    std::fs::write(package.join("root-output"), generated.to_str().unwrap()).unwrap();
    std::fs::write(generated.join("bindings.rs"), "// generated").unwrap();
    assert_eq!(fx.assess("cargo").disposition, Disposition::Reclaimable);
    std::fs::write(package.join("root-output"), "/unknown/source").unwrap();
    assert_eq!(fx.assess("cargo").disposition, Disposition::Deferred);
    let head = fx.head();
    let line = assess(
        &fx.ctx,
        &head,
        "web",
        fx.cfg("web"),
        None,
        fx.now,
        &|_, _, _| Ok(()),
    );
    assert_eq!(line.disposition, Disposition::Deferred);
    let mut unknown = fx.owner("web");
    unknown.ino += 1;
    assert_eq!(
        assess(
            &fx.ctx,
            &head,
            "web",
            fx.cfg("web"),
            Some(&unknown),
            fx.now,
            &|_, _, _| Ok(())
        )
        .disposition,
        Disposition::Deferred
    );
    // Tracked content under an ignored output is still source, never disposable.
    git(&fx.worktree, &["add", "-f", "web/dist/app.js"]);
    assert_eq!(fx.assess("web").disposition, Disposition::Deferred);
    git(
        &fx.worktree,
        &["reset", "-q", "HEAD", "--", "web/dist/app.js"],
    );
    // An arbitrarily named generated file becomes protected when used as evidence,
    // including capture by another task/session. No filename heuristic can prove this.
    let receiver = fx
        .ctx
        .store
        .create("evidence receiver", Some("app"), Status::Backlog, "")
        .unwrap();
    crate::add_artifact(
        &fx.ctx,
        &receiver.id,
        &fx.worktree.join("web/dist/app.js"),
        Some(corral_core::ArtifactKind::Evidence),
        Some("bundle proof"),
    )
    .unwrap();
    assert_eq!(fx.reclaim("web").disposition, Disposition::Retained);
    let consumer = protect_artifact_source(&fx.ctx, &fx.worktree.join("web/dist/app.js"))
        .unwrap()
        .unwrap();
    assert_eq!(fx.reclaim("web").disposition, Disposition::Deferred);
    drop(consumer);
    assert_eq!(fx.reclaim("web").disposition, Disposition::Retained);
    assert!(fx.worktree.join("web/dist/app.js").exists());
    let mut next_run = fx.head();
    next_run.current_run.as_mut().unwrap().id = "R2".into();
    let old_epoch = fx.owner("web").epoch;
    let held = prepare(&fx.ctx.store, &next_run).unwrap();
    assert!(protect_artifact_source(&fx.ctx, &fx.worktree.join("web/dist/app.js")).is_err());
    drop(held);
    assert_ne!(old_epoch, fx.owner("web").epoch);
    assert!(protection::protected(&fx.ctx.store, &fx.owner("web"), "web").unwrap());
    fx.ctx
        .store
        .delete_record("output-owners", &key(&fx.task, "web"))
        .unwrap();
    let before = fx.head();
    let _task = fx.ctx.store.try_lock(&fx.task).unwrap().unwrap();
    prepare(&fx.ctx.store, &before).unwrap(); // GC refusal must not become a launch refusal.
    assert!(
        OWNERS
            .load(&fx.ctx.store, &key(&fx.task, "web"))
            .unwrap()
            .is_none()
    );
    assert!(
        REGISTRATION_NOTES
            .load(&fx.ctx.store, &key(&fx.task, "web"))
            .unwrap()
            .unwrap()
            .refusal
            .is_some()
    );
    assert!(fx.worktree.join("web/dist/app.js").exists());
    let mut shared = before.clone();
    let outside = fx.worktree.parent().unwrap().join("plan-target");
    shared.current_run.as_mut().unwrap().worktree = outside.clone();
    prepare(&fx.ctx.store, &shared).unwrap();
    assert!(!outside.exists());
    assert_eq!(before, fx.head());
}

#[test]
#[cfg(unix)]
fn symlink_shared_output_and_replacement_roots_are_refused() {
    use std::os::unix::fs::symlink;
    let fx = Fx::new();
    fx.build();
    let output = fx.owner("web").path;
    let source = fx.worktree.join("source.rs");
    symlink(&source, output.join("escaped.js")).unwrap();
    assert_eq!(fx.reclaim("web").disposition, Disposition::Deferred);
    assert!(source.exists());
    std::fs::remove_file(output.join("escaped.js")).unwrap();
    std::fs::hard_link(&source, output.join("shared.js")).unwrap();
    assert_eq!(fx.assess("web").disposition, Disposition::Deferred);
    std::fs::remove_file(output.join("shared.js")).unwrap();
    let outside = fx.worktree.join("shared-dist");
    std::fs::rename(&output, &outside).unwrap();
    symlink(&outside, &output).unwrap();
    assert_eq!(fx.assess("web").disposition, Disposition::Deferred);
    assert!(outside.join("app.js").exists());
}

#[test]
fn grace_pending_consumers_and_visibility_are_not_authorized_by_inactive_state() {
    let fx = Fx::new();
    fx.build();
    let cargo_lock = std::fs::File::create(fx.worktree.join("target/.cargo-lock")).unwrap();
    cargo_lock.lock().unwrap();
    assert_eq!(fx.assess("cargo").disposition, Disposition::Deferred);
    drop(cargo_lock);
    let head = fx.head();
    let owner = fx.owner("web");
    let early = assess(
        &fx.ctx,
        &head,
        "web",
        fx.cfg("web"),
        Some(&owner),
        owner.created,
        &|_, _, _| panic!("no process walk during grace"),
    );
    assert_eq!(early.disposition, Disposition::Deferred);
    assert_eq!(
        early.next_check_at,
        Some(
            owner
                .created
                .checked_add(SignedDuration::from_hours(24))
                .unwrap()
        )
    );
    let mut pending_head = head.clone();
    pending_head.task.queued = true;
    assert!(pending(&pending_head).is_err());
    pending_head.task.queued = false;
    pending_head.current_run.as_mut().unwrap().gate = Some(
        serde_json::from_value(json!({"result":"running", "at":fx.now, "sha":"0".repeat(40)}))
            .unwrap(),
    );
    assert!(pending(&pending_head).is_err());
    pending_head.current_run.as_mut().unwrap().gate = None;
    pending_head.current_run.as_mut().unwrap().review = Some(
        serde_json::from_value(json!({"reviewer":"fake", "at":fx.now, "sha":"0".repeat(40)}))
            .unwrap(),
    );
    assert!(pending(&pending_head).is_err());
    pending_head.current_run.as_mut().unwrap().review = None;
    pending_head.current_run.as_mut().unwrap().agent_host = Some(Box::new(serde_json::from_value(json!({
        "version":1, "binding":{"role":"worker", "store":"fixture", "project":"app", "generation":1, "health":"healthy"}
    })).unwrap()));
    assert!(pending(&pending_head).is_err());
    for reason in [
        "active Cargo build",
        "active generated executable",
        "active LSP",
        "incomplete process visibility",
        "unknown owner",
    ] {
        let line = assess(
            &fx.ctx,
            &head,
            "web",
            fx.cfg("web"),
            Some(&owner),
            fx.now,
            &|_, _, _| anyhow::bail!(reason),
        );
        assert_eq!(line.disposition, Disposition::Deferred);
        assert_eq!(line.measured_bytes, None);
    }
    // Real observation never guesses an engine or worker identity for this fixture.
    assert_eq!(
        assess(
            &fx.ctx,
            &head,
            "web",
            fx.cfg("web"),
            Some(&owner),
            fx.now,
            &consumers
        )
        .disposition,
        Disposition::Deferred
    );
}

#[test]
fn exclusion_rechecks_resume_and_interrupted_cleanup_never_invents_freed_bytes() {
    let fx = Fx::new();
    fx.build();
    let owner = fx.owner("web");
    let lock = fx.ctx.store.try_lock(&fx.task).unwrap().unwrap();
    let second = Store::open(fx.ctx.store.root()).unwrap();
    assert!(second.try_lock(&fx.task).unwrap().is_none()); // state writer/launch cannot enter
    let digest = super::super::lifecycle::policy_digest(&fx.ctx).unwrap();
    assert!(lifecycle::settle(&fx.ctx, &fx.task, "web", None, &digest, fx.now).unwrap());
    assert!(owner.path.join("app.js").exists());
    let deferred = fx
        .ctx
        .store
        .lifecycle_row(&fx.task, &owner.generation(), &target("web"))
        .unwrap()
        .unwrap();
    assert_eq!(deferred.outcome, LifecycleOutcome::Deferred);
    assert!(deferred.next_check_at.unwrap() > fx.now);
    drop(lock);
    let lock = second.try_lock(&fx.task).unwrap().unwrap();
    assert!(fx.ctx.store.try_lock(&fx.task).unwrap().is_none()); // cleanup cannot enter launch
    drop(lock);
    // A resume that wins before cleanup is reread and prevents the effect.
    fx.ctx
        .store
        .update_head(&fx.task, |t| {
            t.status = Status::Running;
            Ok(())
        })
        .unwrap();
    assert_eq!(fx.reclaim("web").disposition, Disposition::Deferred);
    assert!(owner.path.join("app.js").exists());
    fx.ctx
        .store
        .update_head(&fx.task, |t| {
            t.status = Status::NeedsMe;
            Ok(())
        })
        .unwrap();
    // Replay a deletion which finished before settlement. It does not claim pre-crash bytes.
    let line = fx.assess("web");
    let digest = super::super::lifecycle::policy_digest(&fx.ctx).unwrap();
    let previous = fx
        .ctx
        .store
        .lifecycle_row(&fx.task, &owner.generation(), &target("web"))
        .unwrap();
    save(
        &fx.ctx,
        &owner,
        "web",
        previous.as_ref(),
        &line,
        &digest,
        fx.now,
    )
    .unwrap();
    DirHandle::open(&fx.worktree)
        .unwrap()
        .remove_entry(Path::new("web/dist"), &|| Ok(()))
        .unwrap();
    let replay = fx.reclaim("web");
    assert_eq!(replay.disposition, Disposition::Reclaimed);
    assert_eq!(replay.reclaimed_bytes, 0);
    assert!(fx.worktree.join("source.rs").exists());
    // A malformed on-disk policy must not silently reuse the old effective config.
    let config = fx.ctx.store.root().join(".corral/corral.toml");
    let saved = std::fs::read(&config).unwrap();
    let lock = fx.ctx.store.try_lock(&fx.task).unwrap().unwrap();
    let owner = fx.owner("cargo");
    let line = reclaim(
        &fx.ctx,
        &fx.head(),
        Registered {
            class: "cargo",
            cfg: fx.cfg("cargo"),
            owner: &owner,
        },
        None,
        fx.now,
        &digest,
        &|_, _, _| {
            std::fs::write(&config, "invalid [ configuration").unwrap();
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(line.disposition, Disposition::Failed);
    assert!(owner.path.join("app.bin").exists());
    std::fs::write(config, saved).unwrap();
    drop(lock);
}

#[test]
fn due_decisions_skip_filesystem_and_process_work_until_retry_time() {
    let fx = Fx::new();
    fx.build();
    let digest = super::super::lifecycle::policy_digest(&fx.ctx).unwrap();
    let owner = fx.owner("web");
    let mut line = fx.assess("web");
    line.disposition = Disposition::Deferred;
    line.reason = "consumer unavailable".into();
    line.next_check_at = Some(fx.now.checked_add(SignedDuration::from_hours(1)).unwrap());
    save(&fx.ctx, &owner, "web", None, &line, &digest, fx.now).unwrap();
    // An unchanged, not-yet-due row must not reach Git/files/process checks, even on a task change.
    std::fs::remove_file(fx.worktree.join(".git")).unwrap();
    for size in [1, 2] {
        fx.ctx
            .store
            .create("terminal history", Some("app"), Status::Done, "")
            .unwrap();
        let _count = corral_core::work::HotScope::enter();
        let before = corral_core::work::snapshot();
        let git = corral_git::work::invocations();
        assert!(lifecycle::settle(&fx.ctx, &fx.task, "web", None, &digest, fx.now).unwrap());
        let work = corral_core::work::snapshot().since(&before);
        use corral_core::work::Counter;
        assert_eq!(
            work.total(Counter::TaskHeadDecodes),
            1,
            "history size {size}"
        );
        for counter in [
            Counter::HistoryRowDecodes,
            Counter::NestedRowDecodes,
            Counter::FullTaskHydrations,
            Counter::FileStats,
            Counter::Processes,
        ] {
            assert_eq!(work.total(counter), 0, "history size {size}, {counter:?}");
        }
        assert_eq!(corral_git::work::invocations() - git, 0);
    }
    let after = fx
        .ctx
        .store
        .lifecycle_row(&fx.task, &owner.generation(), &target("web"))
        .unwrap()
        .unwrap();
    assert_eq!(after.reason.as_deref(), Some("consumer unavailable"));
    assert_eq!(after.next_check_at, line.next_check_at);
}
