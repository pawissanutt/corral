//! Storage roots, the section 8 containment matrix and class-scoped deletion (T-253 storage-roots).
#![cfg(unix)]

use std::collections::BTreeSet;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use corral_core::backend::{
    BackendConfig, FaultInjector, ManualClock, Operation, ScratchKind, ScratchOp, ScratchState,
    StoreBackend, fake::FakeBackend,
};
use corral_core::fs_guard::Removal;
use corral_core::scratch::{ClassDeleter, NewScratch, Reclaimed, Registration, ScratchArea};
use corral_core::storage_roots::{FIXED_CLASS_DIRS, MatrixRow, prepare_fixed_roots, workspace_id};
use corral_core::{Config, StorageCfg, StorageClass, StorageEnv, StorageLayout};
use jiff::Timestamp;
use tempfile::TempDir;

/// Name, cache_root, scratch_root, and the expected (key, colliding path, row); `None` for a
/// path that cannot be resolved.
type MatrixCase<'a> = (
    &'a str,
    Option<PathBuf>,
    Option<PathBuf>,
    Option<(&'a str, PathBuf, MatrixRow)>,
);

struct Ws {
    _dir: TempDir,
    _out: TempDir,
    w: PathBuf,
    out: PathBuf,
}

impl Ws {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let out = TempDir::new().unwrap();
        let w = std::fs::canonicalize(dir.path()).unwrap();
        let o = std::fs::canonicalize(out.path()).unwrap();
        std::fs::create_dir_all(w.join("app/.git")).unwrap();
        std::fs::create_dir_all(w.join("nest/app/.git")).unwrap();
        std::fs::create_dir_all(w.join(".corral/state")).unwrap();
        Self {
            _dir: dir,
            _out: out,
            w,
            out: o,
        }
    }

    fn config(&self, cache: Option<PathBuf>, scratch: Option<PathBuf>) -> Config {
        let mut cfg = Config::default_for(&self.w);
        let mut nested = cfg.projects["app"].clone();
        nested.path = "nest/app".into();
        cfg.projects.insert("nested".into(), nested);
        cfg.storage.cache_root = cache;
        cfg.storage.scratch_root = scratch;
        cfg
    }

    fn env(&self) -> StorageEnv {
        StorageEnv {
            xdg_cache_home: Some(self.out.join("xdg")),
            home: std::env::var_os("HOME").map(PathBuf::from),
        }
    }

    fn layout(&self, scratch: Option<PathBuf>) -> StorageLayout {
        let cfg = self.config(None, scratch);
        StorageLayout::resolve(&self.w, &cfg.storage, &cfg.projects, &self.env()).unwrap()
    }
}

fn tree(roots: &[&Path]) -> BTreeSet<PathBuf> {
    fn walk(path: &Path, out: &mut BTreeSet<PathBuf>) {
        out.insert(path.to_path_buf());
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return;
        };
        if meta.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                walk(&entry.unwrap().path(), out);
            }
        }
    }
    let mut out = BTreeSet::new();
    for root in roots {
        walk(root, &mut out);
    }
    out
}

fn storage_errors(cfg: &Config, root: &Path) -> Vec<String> {
    match cfg.validate(root) {
        Ok(()) => Vec::new(),
        Err(errors) => errors
            .0
            .into_iter()
            .filter(|e| e.starts_with("storage"))
            .collect(),
    }
}

#[test]
fn defaults_load_on_fresh_and_migrated_workspaces() {
    let ws = Ws::new();
    let fresh = ws.layout(None);
    let base = ws.out.join("xdg/corral").join(workspace_id(&ws.w));
    assert_eq!(workspace_id(&ws.w).len(), 16);
    assert_eq!(fresh.cache_root(), base.join("cache"));
    assert_eq!(fresh.scratch_root(), base.join("scratch"));
    assert!(!base.exists(), "resolving creates nothing");

    let cfg = StorageCfg::default();
    assert_eq!(cfg.transcript_retention(), None);
    assert_eq!(cfg.review_scratch_warn_bytes().unwrap(), 2 << 30);
    assert_eq!(cfg.review_evidence_max_bytes().unwrap(), 64 << 20);
    assert_eq!(cfg.blob_grace().as_secs(), 24 * 3600);
    assert_eq!(cfg.view_max_age().as_secs(), 120);
    let parsed: Config = toml::from_str("[storage]\ntranscript_retention_days = 30\n").unwrap();
    assert_eq!(
        parsed.storage.transcript_retention().unwrap().as_secs(),
        30 * 86_400
    );
    assert!(!Config::default_for(&ws.w).to_toml().contains("[storage]"));

    let corral = ws.w.join(".corral");
    prepare_fixed_roots(&corral).unwrap();
    for legacy in ["tasks", "artifacts", "prompts", "lessons"] {
        std::fs::create_dir_all(corral.join(legacy)).unwrap();
    }
    std::fs::write(corral.join("events.jsonl"), "{}\n").unwrap();
    std::fs::write(corral.join("state/x.json"), "{}").unwrap();
    prepare_fixed_roots(&corral).unwrap();
    let migrated = ws.layout(None);
    assert_eq!(migrated.cache_root(), fresh.cache_root());
    assert!(storage_errors(&ws.config(None, None), &ws.w).is_empty());
    for root in FIXED_CLASS_DIRS {
        let meta = std::fs::symlink_metadata(corral.join(root)).unwrap();
        assert!(meta.is_dir() && !meta.file_type().is_symlink(), "{root}");
    }
}

#[test]
fn the_matrix_refuses_every_overlapping_root_and_creates_nothing() {
    let ws = Ws::new();
    let (w, o) = (ws.w.clone(), ws.out.clone());
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    std::fs::create_dir_all(o.join("s")).unwrap();
    symlink(w.join(".corral/state"), w.join("alias")).unwrap();
    symlink(o.join("s"), o.join("calias")).unwrap();
    let state = w.join(".corral/state");
    let some = |p: PathBuf| Some(p);
    use MatrixRow::*;
    let cases: Vec<MatrixCase> = vec![
        (
            "scratch = cache",
            some(o.join("c")),
            some(o.join("c")),
            Some(("cache_root", o.join("c"), OtherConfigurableRoot)),
        ),
        (
            "scratch inside cache",
            some(o.join("c")),
            some(o.join("c/s")),
            Some(("scratch_root", o.join("c"), OtherConfigurableRoot)),
        ),
        (
            "cache inside scratch",
            some(o.join("s/c")),
            some(o.join("s")),
            Some(("cache_root", o.join("s"), OtherConfigurableRoot)),
        ),
        (
            "scratch = .corral",
            None,
            some(w.join(".corral")),
            Some(("scratch_root", w.join(".corral"), CorralAndFixedRoots)),
        ),
        (
            "scratch = .corral/state",
            None,
            some(state.clone()),
            Some(("scratch_root", state.clone(), CorralAndFixedRoots)),
        ),
        (
            "scratch inside .corral/blobs",
            None,
            some(w.join(".corral/blobs/x")),
            Some(("scratch_root", w.join(".corral/blobs"), CorralAndFixedRoots)),
        ),
        (
            "scratch = workspace root",
            None,
            some(w.clone()),
            Some(("scratch_root", w.clone(), Workspace)),
        ),
        (
            "scratch = $HOME",
            None,
            some(home.clone()),
            Some((
                "scratch_root",
                std::fs::canonicalize(&home).unwrap(),
                HomeAndFilesystemRoot,
            )),
        ),
        (
            "scratch = /",
            None,
            some(PathBuf::from("/")),
            Some(("scratch_root", PathBuf::from("/"), HomeAndFilesystemRoot)),
        ),
        (
            "scratch inside a project",
            None,
            some(w.join("app/x")),
            Some(("scratch_root", w.join("app"), Project)),
        ),
        (
            "scratch inside .worktrees",
            None,
            some(w.join("app/.worktrees/x")),
            Some(("scratch_root", w.join("app/.worktrees"), Project)),
        ),
        (
            "scratch contains a project",
            None,
            some(w.join("nest")),
            Some(("scratch_root", w.join("nest/app"), Project)),
        ),
        (
            "scratch alias of .corral/state",
            None,
            some(w.join("alias")),
            Some(("scratch_root", state.clone(), CorralAndFixedRoots)),
        ),
        (
            "cache alias of scratch",
            some(o.join("calias")),
            some(o.join("s")),
            Some(("cache_root", o.join("s"), OtherConfigurableRoot)),
        ),
        ("a path containing ..", None, some(w.join("x/../y")), None),
    ];
    for (name, cache, scratch, expected) in cases {
        let before = tree(&[&w, &o]);
        let cfg = ws.config(cache, scratch);
        let errors = storage_errors(&cfg, &w);
        assert!(!errors.is_empty(), "{name}: accepted");
        let resolved = StorageLayout::resolve(&w, &cfg.storage, &cfg.projects, &ws.env());
        let violations = resolved.expect_err(name);
        match expected {
            Some((key, other, row)) => {
                let hit = violations
                    .iter()
                    .find(|v| v.key == key)
                    .unwrap_or_else(|| panic!("{name}: {violations:?}"));
                let (_, path, got_row, _) = hit.collision.clone().expect(name);
                assert_eq!((path.clone(), got_row), (other.clone(), row), "{name}");
                let shown = hit.to_string();
                assert!(
                    shown.contains(&hit.root.display().to_string()),
                    "{name}: {shown}"
                );
                assert!(
                    shown.contains(&other.display().to_string()),
                    "{name}: {shown}"
                );
                assert!(shown.contains(row.label()), "{name}: {shown}");
                assert!(
                    errors.iter().any(
                        |e| e.contains(row.label()) && e.contains(&other.display().to_string())
                    ),
                    "{name}: config load {errors:?}"
                );
            }
            None => assert!(
                errors.iter().any(|e| e.contains("`..`")),
                "{name}: {errors:?}"
            ),
        }
        assert_eq!(tree(&[&w, &o]), before, "{name}: created something");
    }
}

#[test]
fn a_scratch_root_inside_the_workspace_outside_corral_and_projects_is_accepted() {
    let ws = Ws::new();
    let cfg = ws.config(None, Some(ws.w.join(".scratch")));
    assert_eq!(storage_errors(&cfg, &ws.w), Vec::<String>::new());
    let layout = StorageLayout::resolve(&ws.w, &cfg.storage, &cfg.projects, &ws.env()).unwrap();
    assert_eq!(layout.scratch_root(), ws.w.join(".scratch"));
    assert!(!ws.w.join(".scratch").exists());
    let relative = ws.config(None, Some(".scratch".into()));
    assert_eq!(storage_errors(&relative, &ws.w), Vec::<String>::new());
}

#[test]
fn a_symlinked_fixed_root_refuses_the_open_before_any_write() {
    for root in ["blobs", "db", "agent/sessions"] {
        let ws = Ws::new();
        let corral = ws.w.join(".corral");
        let target = ws.out.join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(corral.join(root).parent().unwrap()).unwrap();
        symlink(&target, corral.join(root)).unwrap();
        let before = tree(&[&ws.w, &ws.out]);
        let error = prepare_fixed_roots(&corral).unwrap_err().to_string();
        assert!(
            error.contains(&format!(".corral/{root}")),
            "{root}: {error}"
        );
        assert_eq!(
            tree(&[&ws.w, &ws.out]),
            before,
            "{root}: wrote before refusing"
        );
    }
}

struct Area {
    ws: Ws,
    layout: StorageLayout,
    backend: FakeBackend,
    now: Timestamp,
}

impl Area {
    fn new() -> Self {
        let ws = Ws::new();
        let layout = ws.layout(Some(ws.w.join(".scratch")));
        prepare_fixed_roots(&ws.w.join(".corral")).unwrap();
        let now = Timestamp::now();
        let backend = FakeBackend::new(
            BackendConfig::default(),
            Arc::new(ManualClock::new(now)),
            Arc::new(FaultInjector::default()),
        );
        Self {
            ws,
            layout,
            backend,
            now,
        }
    }

    fn area(&self) -> ScratchArea<'_> {
        ScratchArea::open(&self.backend, &self.layout).unwrap()
    }

    fn closed(&self, area: &ScratchArea<'_>, id: &str, path: &str) -> PathBuf {
        let dir = area
            .register(
                NewScratch {
                    id: id.into(),
                    kind: ScratchKind::ImplementationReview,
                    task: Some("T-001".into()),
                    subject: Some("T-001".into()),
                    attempt: Some("R1".into()),
                    path: path.into(),
                },
                self.now,
            )
            .unwrap();
        std::fs::write(dir.join("log.txt"), "12345").unwrap();
        area.close(id, self.now).unwrap();
        dir
    }

    fn state(&self, area: &ScratchArea<'_>, id: &str) -> ScratchState {
        area.row(id).unwrap().unwrap().state
    }
}

#[test]
fn register_close_and_reclaim_delete_only_the_registered_entry() {
    let fx = Area::new();
    let area = fx.area();
    let keep = fx.ws.out.join("keep.txt");
    std::fs::write(&keep, "outside").unwrap();
    let dir = fx.closed(&area, "s1", "review/s1");
    std::fs::create_dir(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/b.bin"), "123").unwrap();
    symlink(&keep, dir.join("link")).unwrap();
    let sibling = fx.closed(&area, "s2", "review/s2");
    assert_eq!(
        area.reclaim("s1", "gc", fx.now).unwrap(),
        Reclaimed::Deleted { bytes: 8 }
    );
    assert!(!dir.exists());
    assert!(sibling.join("log.txt").exists() && fx.ws.w.join(".scratch/review").is_dir());
    assert_eq!(std::fs::read_to_string(&keep).unwrap(), "outside");
    assert_eq!(fx.state(&area, "s1"), ScratchState::Reclaimed);
    assert!(area.reclaim("s1", "gc", fx.now).is_err(), "reclaimed twice");
}

#[test]
fn reclaim_refuses_free_paths_and_unregistered_ids() {
    let fx = Area::new();
    let area = fx.area();
    let free = area.root().join("free");
    std::fs::create_dir(&free).unwrap();
    fx.closed(&area, "s1", "review/s1");
    for id in ["free", "review/s1", "../free", "s9"] {
        assert!(area.reclaim(id, "gc", fx.now).is_err(), "{id}");
    }
    assert!(free.is_dir() && area.root().join("review/s1").is_dir());
    assert_eq!(fx.state(&area, "s1"), ScratchState::Closed);
}

#[test]
fn reclaim_refuses_an_ancestor_symlink_and_an_entry_resolving_into_blobs() {
    let fx = Area::new();
    let area = fx.area();
    let blobs = fx.ws.w.join(".corral/blobs");
    for (id, ancestor, target) in [
        ("s1", "via", fx.ws.out.join("other")),
        ("s2", "blob", blobs.clone()),
    ] {
        fx.closed(&area, id, &format!("{ancestor}/{id}"));
        std::fs::create_dir_all(target.join(id)).unwrap();
        std::fs::write(target.join(id).join("keep"), "x").unwrap();
        let real = area.root().join(ancestor);
        std::fs::rename(&real, area.root().join(format!("{ancestor}.moved"))).unwrap();
        symlink(&target, &real).unwrap();
        assert!(area.reclaim(id, "gc", fx.now).is_err(), "{id}");
        assert!(
            target.join(id).join("keep").exists(),
            "{id}: followed the symlink"
        );
        assert_eq!(fx.state(&area, id), ScratchState::Failed);
    }
    let deleter = ClassDeleter::open(&fx.layout, StorageClass::Scratch).unwrap();
    symlink(&blobs, area.root().join("blobs-link")).unwrap();
    for rel in [
        "blobs-link".to_string(),
        "blobs-link/s2".into(),
        "../../.corral/blobs".into(),
        blobs.display().to_string(),
    ] {
        let entry = Registration {
            class: StorageClass::Scratch,
            id: "forged".into(),
            rel: rel.clone().into(),
        };
        assert!(deleter.remove(&entry, &|| Ok(())).is_err(), "{rel}");
    }
    assert!(blobs.join("s2/keep").exists() && area.root().join("blobs-link").exists());
}

#[test]
fn deleters_refuse_other_classes_containers_and_class_roots() {
    let fx = Area::new();
    let area = fx.area();
    let dir = fx.closed(&area, "s1", "review/s1");
    let scratch = ClassDeleter::open(&fx.layout, StorageClass::Scratch).unwrap();
    let sessions = ClassDeleter::open(&fx.layout, StorageClass::AgentSessions).unwrap();
    let foreign = Registration {
        class: StorageClass::AgentSessions,
        id: "s1".into(),
        rel: "review/s1".into(),
    };
    assert!(scratch.remove(&foreign, &|| Ok(())).is_err());
    let own = Registration {
        class: StorageClass::Scratch,
        ..foreign.clone()
    };
    assert!(sessions.remove(&own, &|| Ok(())).is_err());
    for rel in ["", ".", "./"] {
        let root = Registration {
            rel: rel.into(),
            ..own.clone()
        };
        assert!(scratch.remove(&root, &|| Ok(())).is_err(), "{rel:?}");
    }
    let w = fx.ws.w.clone();
    for root in [
        w.clone(),
        w.join(".corral"),
        w.join(".corral/lock"),
        w.join(".corral/rollback"),
        w.join(".corral/blobs"),
        w.join("app"),
        w.join("app/.worktrees"),
        PathBuf::from("/"),
        fx.layout.cache_root().to_path_buf(),
    ] {
        assert!(
            ClassDeleter::open_at(&fx.layout, StorageClass::Scratch, &root).is_err(),
            "{}",
            root.display()
        );
    }
    assert!(dir.join("log.txt").exists());
}

#[test]
fn reclaim_refuses_an_entry_swapped_for_a_symlink() {
    let fx = Area::new();
    let area = fx.area();
    let dir = fx.closed(&area, "s1", "review/s1");
    let moved = fx.ws.out.join("moved");
    std::fs::rename(&dir, &moved).unwrap();
    symlink(&moved, &dir).unwrap();
    assert!(area.reclaim("s1", "gc", fx.now).is_err());
    assert!(moved.join("log.txt").exists());
    assert!(
        std::fs::symlink_metadata(&dir)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn reclaim_refuses_after_the_scratch_root_was_replaced() {
    let fx = Area::new();
    let area = fx.area();
    let dir = fx.closed(&area, "s1", "review/s1");
    let root = area.root().to_path_buf();
    let old = fx.ws.w.join(".scratch.old");
    std::fs::rename(&root, &old).unwrap();
    std::fs::create_dir_all(dir.clone()).unwrap();
    std::fs::write(dir.join("new.txt"), "x").unwrap();
    let error = area.reclaim("s1", "gc", fx.now).unwrap_err();
    assert!(format!("{error:#}").contains("replaced"), "{error:#}");
    assert!(old.join("review/s1/log.txt").exists() && dir.join("new.txt").exists());
}

#[test]
fn identity_and_registration_are_rechecked_immediately_before_deletion() {
    let fx = Area::new();
    let area = fx.area();
    let dir = fx.closed(&area, "s1", "review/s1");
    let deleter = ClassDeleter::open(&fx.layout, StorageClass::Scratch).unwrap();
    let entry = Registration {
        class: StorageClass::Scratch,
        id: "s1".into(),
        rel: "review/s1".into(),
    };
    let refused = || anyhow::bail!("unregistered");
    assert!(deleter.remove(&entry, &refused).is_err());
    assert!(dir.join("log.txt").exists());
    let root = area.root().to_path_buf();
    let old = fx.ws.w.join(".scratch.old");
    let swap = || -> Result<()> {
        std::fs::rename(&root, &old)?;
        std::fs::create_dir_all(root.join("review/s1"))?;
        Ok(())
    };
    let error = deleter.remove(&entry, &swap).unwrap_err();
    assert!(format!("{error:#}").contains("replaced"), "{error:#}");
    assert!(old.join("review/s1/log.txt").exists() && root.join("review/s1").is_dir());
}

#[test]
fn a_reclaim_interrupted_after_deletion_completes_on_resume() {
    let fx = Area::new();
    let area = fx.area();
    let dir = fx.closed(&area, "s1", "review/s1");
    fx.backend
        .apply(Operation::Scratch(ScratchOp::BeginReclaim {
            id: "s1".into(),
            from: ScratchState::Closed,
            owner: "gc".into(),
            at: fx.now,
        }))
        .unwrap();
    let deleter = ClassDeleter::open(&fx.layout, StorageClass::Scratch).unwrap();
    let entry = Registration {
        class: StorageClass::Scratch,
        id: "s1".into(),
        rel: "review/s1".into(),
    };
    assert_eq!(
        deleter.remove(&entry, &|| Ok(())).unwrap(),
        Removal::Removed { bytes: 5 }
    );
    assert!(!dir.exists());
    assert_eq!(
        area.reclaim("s1", "gc", fx.now).unwrap(),
        Reclaimed::AlreadyAbsent
    );
    assert_eq!(fx.state(&area, "s1"), ScratchState::Reclaimed);
}
