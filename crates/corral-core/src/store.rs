use std::path::{Path, PathBuf};

use std::fs;
use std::io::Write;
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use jiff::Timestamp;

use crate::config::Config;
use crate::config_doc::ConfigDoc;
use crate::model::{Event, Status, Task};

/// The `.corral/` directory of a workspace.
///
/// Layout:
/// ```text
/// <root>/.corral/
///   corral.toml          config
///   tasks/<id>-<slug>.md task files (YAML frontmatter between "---" lines, then markdown body)
///   events.jsonl         append-only history (one Event per line)
///   pins.toml            ordered pinned tasks/artifacts (`[[pin]]` ref, label, follow)
///   artifacts/<task>/<run>/<sha8>-<file name>   artifact snapshots
///   prompts/<task>-<run>.md                     rendered dispatch prompts
///   state/               UI state and scratch (selection, etc.)
///   lock/                lock files (`next-id.lock`, `<task>.lock`)
///   .gitignore           contains "artifacts/\nprompts/\nstate/\nlock/\n"
/// ```
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
    dir: PathBuf,
    pub config: Config,
    /// corral.toml (mtime, size) when `config` was last loaded.
    config_stamp: Option<(SystemTime, u64)>,
}

impl Store {
    fn task_files(&self) -> Result<Vec<PathBuf>> {
        let dir = self.dir.join("tasks");
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        let mut v: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "md"))
            .collect();
        v.sort();
        Ok(v)
    }

    /// Exclusive lock on `lock/<name>.lock`, held until the returned file is dropped.
    pub(crate) fn lock(&self, name: &str) -> Result<fs::File> {
        let dir = self.dir.join("lock");
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{name}.lock"));
        let f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        f.lock()
            .with_context(|| format!("locking {}", path.display()))?;
        Ok(f)
    }

    /// Like the per-task locks, but non-blocking: None while another process holds `lock/<name>.lock`.
    pub fn try_lock(&self, name: &str) -> Result<Option<fs::File>> {
        let dir = self.dir.join("lock");
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{name}.lock"));
        let f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        match f.try_lock() {
            Ok(()) => Ok(Some(f)),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(e)) => {
                Err(e).with_context(|| format!("locking {}", path.display()))
            }
        }
    }

    fn save_inner(&self, task: &Task) -> Result<Task> {
        let mut t = task.clone();
        t.updated = Timestamp::now();
        if t.path.as_os_str().is_empty() {
            t.path = self
                .dir
                .join("tasks")
                .join(format!("{}.md", file_stem(&t.id, &t.title)));
        }
        atomic_write(&t.path, &render_task(&t))?;
        Ok(t)
    }
    /// Finds the workspace: $CORRAL_ROOT if set, else walks up from `start` to the first dir
    /// containing `.corral/corral.toml`. Error message on failure: "no .corral workspace found (run `corral init`)".
    pub fn discover(start: &Path) -> Result<Store> {
        Store::open(&Store::find_root(start)?)
    }

    /// The workspace root `discover` would open, without loading its config.
    pub fn find_root(start: &Path) -> Result<PathBuf> {
        if let Some(r) = std::env::var_os("CORRAL_ROOT").filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(r));
        }
        let start =
            std::path::absolute(start).with_context(|| format!("resolving {}", start.display()))?;
        for dir in start.ancestors() {
            if dir.join(".corral").join("corral.toml").is_file() {
                return Ok(dir.to_path_buf());
            }
        }
        bail!("no .corral workspace found (run `corral init`)")
    }

    /// Creates `<root>/.corral/` with all subdirs, the .gitignore above and `Config::default_for(root)`.
    /// Idempotent: an existing corral.toml is kept as-is (loaded, not overwritten).
    pub fn init(root: &Path) -> Result<Store> {
        let dir = root.join(".corral");
        for sub in ["tasks", "artifacts", "prompts", "state", "lock"] {
            let p = dir.join(sub);
            fs::create_dir_all(&p).with_context(|| format!("creating {}", p.display()))?;
        }
        let events = dir.join("events.jsonl");
        if !events.exists() {
            fs::write(&events, "").with_context(|| format!("creating {}", events.display()))?;
        }
        let gi = dir.join(".gitignore");
        if !gi.exists() {
            fs::write(&gi, "artifacts/\nprompts/\nstate/\nlock/\n")
                .with_context(|| format!("writing {}", gi.display()))?;
        }
        let toml = dir.join("corral.toml");
        if !toml.exists() {
            atomic_write(&toml, &Config::default_for(root).to_toml())?;
        }
        Store::open(root)
    }

    /// Opens an existing store at `root` (no walking).
    pub fn open(root: &Path) -> Result<Store> {
        // Absolute root: prompt files and worktrees are handed to shells running in other directories.
        let root = &root
            .canonicalize()
            .with_context(|| format!("resolving {}", root.display()))?;
        let dir = root.join(".corral");
        let path = dir.join("corral.toml");
        let config_stamp = file_stamp(&path);
        let config = Config::load(&path)?;
        Ok(Store {
            root: root.to_path_buf(),
            dir,
            config,
            config_stamp,
        })
    }

    /// Workspace root (the dir containing `.corral/`).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `.corral/` dir.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `<dir>/corral.toml`.
    pub fn config_path(&self) -> PathBuf {
        self.dir.join("corral.toml")
    }

    /// Writes the current `config` back to corral.toml through the validated save path: only changed keys
    /// are rewritten, so comments and formatting elsewhere survive (see [`ConfigDoc::sync_from`]).
    pub fn save_config(&self) -> Result<()> {
        self.edit_config(|doc| doc.sync_from(&self.config))
            .map(|_| ())
    }

    /// The one write path for corral.toml: under `lock/config.lock`, re-reads the file, applies `f`, checks
    /// the result against the schema and [`Config::validate`], then writes it atomically (temp + rename).
    /// Nothing is written when `f`, parsing or validation fails; validation errors list every problem.
    /// Returns the saved config (`self.config` is not updated; see [`Store::reload_config_if_changed`]).
    pub fn edit_config<F>(&self, f: F) -> Result<Config>
    where
        F: FnOnce(&mut ConfigDoc) -> Result<()>,
    {
        let _lock = self.lock("config")?;
        let path = self.config_path();
        let mut doc = ConfigDoc::load(&path)?;
        f(&mut doc)?;
        let config = doc
            .config()
            .with_context(|| format!("refusing to save {}", path.display()))?;
        config
            .validate(&self.root)
            .with_context(|| format!("refusing to save {}", path.display()))?;
        atomic_write(&path, &doc.to_string())?;
        Ok(config)
    }

    /// Reloads `config` when corral.toml changed (mtime or size) since it was last loaded. Returns whether it
    /// reloaded. A file that fails to load keeps the previous config and is not retried until it changes again.
    pub fn reload_config_if_changed(&mut self) -> Result<bool> {
        let path = self.config_path();
        let stamp = file_stamp(&path);
        if stamp == self.config_stamp {
            return Ok(false);
        }
        self.config_stamp = stamp;
        self.config = Config::load(&path)?;
        Ok(true)
    }

    /// Sets `[review] autopilot` through [`Store::edit_config`] (validated, comments kept) and keeps the saved
    /// config.
    pub fn set_autopilot(&mut self, on: bool) -> Result<()> {
        let value = if on { "true" } else { "false" };
        self.config = self.edit_config(|doc| doc.set_str("review.autopilot", value))?;
        self.config_stamp = file_stamp(&self.config_path());
        Ok(())
    }

    /// All tasks (including done/dropped), sorted by numeric id. Unparseable files are skipped with
    /// a line on stderr naming the file, never a hard error.
    pub fn list(&self) -> Result<Vec<Task>> {
        let mut tasks = Vec::new();
        for path in self.task_files()? {
            let parsed = fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))
                .and_then(|text| parse_task(&text, &path));
            match parsed {
                Ok(t) => tasks.push(t),
                Err(e) => eprintln!("corral: skipping {}: {e:#}", path.display()),
            }
        }
        tasks.sort_by_key(|t| (id_number(&t.id), t.id.clone()));
        Ok(tasks)
    }

    /// Accepts "T-042", "t-42", "42". Error "task T-042 not found" when missing.
    pub fn get(&self, id: &str) -> Result<Task> {
        let id = Self::normalize_id(id)?;
        let n = id_number(&id);
        for path in self.task_files()? {
            if file_id_number(&path) == Some(n) {
                let text = fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                return parse_task(&text, &path);
            }
        }
        bail!("task {id} not found")
    }

    /// Normalizes an id: "42" / "t-42" / "T-042" -> "T-042".
    pub fn normalize_id(id: &str) -> Result<String> {
        let t = id.trim();
        let digits = t
            .strip_prefix("T-")
            .or_else(|| t.strip_prefix("t-"))
            .unwrap_or(t);
        match digits.parse::<u64>() {
            Ok(n) if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) => {
                Ok(format!("T-{n:03}"))
            }
            _ => bail!("invalid task id {id:?}"),
        }
    }

    /// Allocates the next id under `lock/next-id.lock` (max existing id + 1), writes the file
    /// `tasks/<id>-<slug>.md` (slug: lowercase alnum words joined by '-', max 48 chars) with the given
    /// body, logs a "created" event, and returns the task.
    pub fn create(
        &self,
        title: &str,
        project: Option<&str>,
        status: Status,
        body: &str,
    ) -> Result<Task> {
        let _lock = self.lock("next-id")?;
        let next = self
            .task_files()?
            .iter()
            .filter_map(|p| file_id_number(p))
            .max()
            .map_or(1, |m| m + 1);
        let now = Timestamp::now();
        let task = Task {
            id: format!("T-{next:03}"),
            title: title.to_string(),
            status,
            project: project.map(str::to_string),
            kind: None,
            priority: "P2".to_string(),
            depends_on: Vec::new(),
            profile: None,
            queued: false,
            review: None,
            rounds: 0,
            created: now,
            updated: now,
            runs: Vec::new(),
            artifacts: Vec::new(),
            body: body.to_string(),
            path: PathBuf::new(),
        };
        let saved = self.save_inner(&task)?;
        self.log_event(
            Some(&saved.id),
            "created",
            serde_json::json!({ "title": title, "status": status.as_str() }),
        )?;
        Ok(saved)
    }

    /// Writes the task atomically (temp file + rename) to its existing path (or a new path from its
    /// id+title when `path` is empty), setting `updated` to now.
    pub fn save(&self, task: &Task) -> Result<()> {
        self.save_inner(task).map(|_| ())
    }

    /// Takes the per-task lock, re-reads the task from disk, applies `f`, saves, releases the lock,
    /// and returns the saved task. Use this for every read-modify-write.
    pub fn update<F>(&self, id: &str, f: F) -> Result<Task>
    where
        F: FnOnce(&mut Task) -> Result<()>,
    {
        let id = Self::normalize_id(id)?;
        let _lock = self.lock(&id)?;
        let mut task = self.get(&id)?;
        f(&mut task)?;
        self.save_inner(&task)
    }

    /// Like `update`, but `f` decides on the fresh copy whether to write: returning `false` leaves the
    /// file untouched and yields `None`. Use it when the change was computed from an earlier read and
    /// must be skipped if the task has moved on since (compare-and-swap under the per-task lock).
    pub fn try_update<F>(&self, id: &str, f: F) -> Result<Option<Task>>
    where
        F: FnOnce(&mut Task) -> Result<bool>,
    {
        let id = Self::normalize_id(id)?;
        let _lock = self.lock(&id)?;
        let mut task = self.get(&id)?;
        if !f(&mut task)? {
            return Ok(None);
        }
        self.save_inner(&task).map(Some)
    }

    /// Appends one Event line to events.jsonl (append mode, single write call).
    pub fn log_event(
        &self,
        task: Option<&str>,
        kind: &str,
        detail: serde_json::Value,
    ) -> Result<()> {
        let ev = Event {
            ts: Timestamp::now(),
            task: task.map(str::to_string),
            kind: kind.to_string(),
            detail,
        };
        let mut line = serde_json::to_string(&ev).context("serializing event")?;
        line.push('\n');
        let path = self.dir.join("events.jsonl");
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        f.write_all(line.as_bytes())
            .with_context(|| format!("appending to {}", path.display()))
    }

    /// Reads events.jsonl, newest last; malformed lines are skipped.
    pub fn events(&self) -> Result<Vec<Event>> {
        let path = self.dir.join("events.jsonl");
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(text
            .lines()
            .filter_map(|l| serde_json::from_str::<Event>(l).ok())
            .collect())
    }

    /// `<dir>/artifacts/<task>/<run>` (not created).
    pub fn artifacts_dir(&self, task: &str, run: &str) -> PathBuf {
        self.dir.join("artifacts").join(task).join(run)
    }

    /// `<dir>/prompts` (not created).
    pub fn prompts_dir(&self) -> PathBuf {
        self.dir.join("prompts")
    }

    /// `<dir>/state` (not created).
    pub fn state_dir(&self) -> PathBuf {
        self.dir.join("state")
    }

    /// Absolute path of a project's repo: config path joined to root (absolute paths kept).
    pub fn project_path(&self, project: &str) -> Result<PathBuf> {
        let p = self
            .config
            .projects
            .get(project)
            .with_context(|| format!("unknown project {project:?}"))?;
        let path = Path::new(&p.path);
        Ok(if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        })
    }
}

/// Parses a task file's text. Frontmatter is between a first line "---" and the next line "---".
pub fn parse_task(text: &str, path: &Path) -> Result<Task> {
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or("");
    if first.trim_end() != "---" {
        bail!("{}: missing frontmatter start", path.display());
    }
    let mut yaml = String::new();
    let mut consumed = first.len();
    let mut closed = false;
    for line in lines {
        consumed += line.len();
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        yaml.push_str(line);
    }
    if !closed {
        bail!("{}: unterminated frontmatter", path.display());
    }
    let mut task: Task = serde_yaml_ng::from_str(&yaml)
        .with_context(|| format!("parsing frontmatter of {}", path.display()))?;
    task.body = text[consumed..].to_string();
    task.path = path.to_path_buf();
    Ok(task)
}

/// Renders a task to file text: "---\n<yaml>---\n<body>" (body kept verbatim; ends with a newline).
pub fn render_task(task: &Task) -> String {
    let yaml = serde_yaml_ng::to_string(task).unwrap_or_default();
    let mut out = String::from("---\n");
    out.push_str(&yaml);
    if !yaml.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---\n");
    out.push_str(&task.body);
    if !task.body.is_empty() && !task.body.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn file_stem(id: &str, title: &str) -> String {
    let mut slug = String::new();
    let mut pending = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            if pending && !slug.is_empty() {
                slug.push('-');
            }
            pending = false;
            slug.push(c.to_ascii_lowercase());
        } else {
            pending = true;
        }
    }
    slug.truncate(48);
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        id.to_string()
    } else {
        format!("{id}-{slug}")
    }
}

fn id_number(id: &str) -> u64 {
    id.trim_start_matches(['T', 't', '-'])
        .parse()
        .unwrap_or(u64::MAX)
}

/// Numeric id from a task file name like `T-042-some-slug.md` or `T-042.md`.
fn file_id_number(path: &Path) -> Option<u64> {
    let stem = path.file_stem()?.to_str()?;
    let rest = stem.strip_prefix("T-")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !(rest.len() == digits.len() || rest[digits.len()..].starts_with('-')) {
        return None;
    }
    digits.parse().ok()
}

fn file_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

pub(crate) fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, content).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))
}
