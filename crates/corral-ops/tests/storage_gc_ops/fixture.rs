//! A workspace with one Git project and configured roots inside the workspace, plus helpers that
//! make finished tasks with real worktrees.

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};

use corral_core::config::Project;
use corral_core::{Run, Status, Store};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use serde_json::json;
use sha2::{Digest, Sha256};

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::support::command("git", dir)
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
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub struct Fx {
    pub _tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub app: PathBuf,
    pub ctx: Ctx,
}

pub fn context(store: Store, root: &Path) -> Ctx {
    Ctx {
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        store,
        herdr: Herdr::with_bin(root.join("absent-herdr"), None),
    }
}

impl Fx {
    pub fn new() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let app = root.join("app");
        std::fs::create_dir(&app).unwrap();
        git(&app, &["init", "-q", "-b", "main"]);
        std::fs::write(app.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(app.join("README.md"), "hello\n").unwrap();
        git(&app, &["add", "-A"]);
        git(&app, &["commit", "-q", "-m", "init"]);
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
        store.config.storage.cache_root = Some(root.join(".cache"));
        store.config.storage.scratch_root = Some(root.join(".scratch"));
        store.save_config().unwrap();
        let ctx = context(store, &root);
        Fx {
            _tmp: tmp,
            root,
            app,
            ctx,
        }
    }

    /// A fresh handle on the same store.
    pub fn reopen(&self) -> Ctx {
        context(Store::open(&self.root).unwrap(), &self.root)
    }

    pub fn task(&self, status: Status) -> String {
        let task = self
            .ctx
            .store
            .create("work", Some("app"), status, "")
            .unwrap();
        task.id
    }

    /// A real worktree of `app` for `task`'s run `run`, on branch `corral/<task>-<run>`.
    pub fn worktree(&self, task: &str, run: &str) -> PathBuf {
        let path = self.app.join(".worktrees").join(task);
        let branch = format!("corral/{task}-{run}");
        git(
            &self.app,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                &path.to_string_lossy(),
                "main",
            ],
        );
        path
    }

    /// Appends run `run` with `worktree` to `task` and sets its status.
    pub fn start(&self, task: &str, run: &str, worktree: &Path, status: Status) {
        let run: Run = serde_json::from_value(json!({
            "id": run,
            "agent": "fake",
            "worktree": worktree,
            "branch": format!("corral/{task}-{run}"),
            "base": "main",
            "base_sha": "0".repeat(40),
            "started": corral_core::clock::now(),
        }))
        .unwrap();
        self.ctx
            .store
            .update(task, |t| {
                t.runs.push(run);
                t.status = status;
                Ok(())
            })
            .unwrap();
    }

    pub fn set_status(&self, task: &str, status: Status) {
        self.ctx
            .store
            .update(task, |t| {
                t.status = status;
                Ok(())
            })
            .unwrap();
    }

    /// A finished task with a clean worktree holding ignored build output.
    pub fn finished(&self, status: Status) -> (String, PathBuf) {
        let id = self.task(Status::Running);
        let path = self.worktree(&id, "R1");
        let target = path.join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(
            target.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        std::fs::write(target.join("out.bin"), vec![7u8; 4096]).unwrap();
        self.start(&id, "R1", &path, status);
        (id, path)
    }

    pub fn branch_exists(&self, branch: &str) -> bool {
        !git(&self.app, &["branch", "--list", branch])
            .trim()
            .is_empty()
    }

    pub fn events(&self, kind: &str) -> Vec<corral_core::Event> {
        self.ctx.store.events_of(None, &[kind]).unwrap()
    }
}

/// Every path below `root` with its type and content digest; nothing is followed.
pub fn tree_digest(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path.strip_prefix(root).unwrap().display().to_string();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            let what = if meta.is_dir() {
                pending.push(path.clone());
                "dir".to_owned()
            } else if meta.is_symlink() {
                format!("link:{}", std::fs::read_link(&path).unwrap().display())
            } else {
                format!("{:x}", Sha256::digest(std::fs::read(&path).unwrap()))
            };
            out.push((rel, what));
        }
    }
    out.sort();
    out
}

/// A process whose working directory is `dir`, named `name`, until dropped.
pub struct Resident(pub Child);

impl Resident {
    pub fn start(dir: &Path, name: Option<&str>) -> Resident {
        let mut command = crate::support::command("python3", dir);
        command
            .args([
                "-c",
                "import signal; print('ready', flush=True); signal.pause()",
            ])
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped());
        if let Some(name) = name {
            command.arg0(name);
        }
        let mut child = command.spawn().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "ready");
        Resident(child)
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
        }
        self.0.wait().unwrap();
    }
}
