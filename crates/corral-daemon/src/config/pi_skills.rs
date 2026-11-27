//! Workspace skill policy and bounded, descriptor-relative Markdown editing.
use std::{
    collections::HashSet,
    fs::{File, Metadata, Permissions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use corral_engine::{Config, pi_preferences};
use corral_protocol::config::{
    PiSkillDetail, PiSkillGet, PiSkillPut, PiSkillRow, PiSkillSource, PiSkillToggle,
    PiSkillsCatalog, PiSkillsRequest,
};
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, open, openat, renameat, unlinkat};
use sha2::{Digest, Sha256};
use toml_edit::{Array, DocumentMut, Item, Table};

const LIMIT: usize = 256 * 1024;
const HEADER_LIMIT: usize = 16 * 1024;
const STALE: &str = "Skill Markdown changed; reload the skill before saving";
const DIR: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC)
    .union(OFlags::NOFOLLOW);
macro_rules! asset {
    ($name:literal, $required:literal) => {
        shipped!(
            $name,
            $required,
            concat!("assets/skills/", $name, "/SKILL.md")
        )
    };
}
macro_rules! shipped {
    ($name:literal, $required:literal, $path:expr) => {
        (
            $name,
            $required,
            $path,
            include_str!(concat!("../../../../packages/corral-pi/", $path)),
        )
    };
}
const SHIPPED: &[(&str, bool, &str, &str)] = &[
    asset!("corral-plan", true),
    asset!("corral-diagrams", true),
    shipped!(
        "corral-worker",
        true,
        "src/roles/worker/skills/corral-worker/SKILL.md"
    ),
    asset!("debugging", false),
    asset!("frontend", false),
    asset!("git-master", false),
    asset!("programming", false),
    asset!("simplify-and-cleanup", false),
    asset!("visual-qa", false),
];

struct Policy {
    disabled: Vec<String>,
    user_dirs: Vec<String>,
}
// Match JavaScript String.trim() used by Pi's workspace policy reader (including BOM).
fn policy_string(s: &str) -> bool {
    !s.is_empty()
        && !s.contains('\0')
        && s.trim_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}') == s
}

fn policy(doc: &DocumentMut) -> Result<Policy> {
    let table = doc
        .get("skills")
        .map(|item| {
            item.as_table_like()
                .context("pi.toml skills must be a table")
        })
        .transpose()?;
    let list = |key| -> Result<Vec<String>> {
        let Some(item) = table.and_then(|table| table.get(key)) else {
            return Ok(Vec::new());
        };
        item.as_array()
            .context("pi.toml skills entries must be arrays of strings")?
            .iter()
            .map(|item| {
                item.as_str()
                    .filter(|s| policy_string(s))
                    .map(str::to_owned)
                    .context("pi.toml skills entries must be nonempty strings")
            })
            .collect()
    };
    let p = Policy {
        disabled: list("disabled")?,
        user_dirs: list("user_dirs")?,
    };
    ensure!(
        p.user_dirs.len() <= 32,
        "too many configured Pi skill directories"
    );
    Ok(p)
}

struct Entry {
    row: PiSkillRow,
    backing: Backing,
}
enum Backing {
    Shipped(&'static str),
    Local(PathBuf),
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn open_dir(path: &Path) -> Result<File> {
    ensure!(path.is_absolute(), "Pi skill directory must be absolute");
    let mut dir = File::from(open("/", DIR, Mode::empty())?);
    for component in path.components() {
        match component {
            Component::RootDir => (),
            Component::Normal(part) => dir = File::from(openat(&dir, part, DIR, Mode::empty())?),
            _ => anyhow::bail!("Pi skill path contains traversal"),
        }
    }
    Ok(dir)
}

fn open_markdown(dir: &File, name: &std::ffi::OsStr) -> Result<(File, Metadata)> {
    let kind = rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)?;
    ensure!(
        FileType::from_raw_mode(kind.st_mode) == FileType::RegularFile,
        "Skill Markdown is not a regular file"
    );
    let file = File::from(openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?);
    let meta = file.metadata()?;
    ensure!(meta.is_file(), "Skill Markdown is not a regular file");
    ensure!(meta.len() <= LIMIT as u64, "Skill Markdown exceeds 256 KiB");
    Ok((file, meta))
}

fn read_markdown(dir: &File, name: &std::ffi::OsStr) -> Result<(String, Metadata)> {
    let (file, meta) = open_markdown(dir, name)?;
    let mut text = String::new();
    file.take(LIMIT as u64 + 1).read_to_string(&mut text)?;
    ensure!(text.len() <= LIMIT, "Skill Markdown exceeds 256 KiB");
    Ok((text, meta))
}

mod frontmatter;
use frontmatter::skill_name;

fn packaged_path(path: &Path) -> bool {
    SHIPPED
        .iter()
        .any(|(_, _, relative, _)| path.ends_with(relative))
}
struct Scan {
    entries: Vec<Entry>,
    paths: HashSet<PathBuf>,
    roots: HashSet<PathBuf>,
    visited: usize,
}
impl Scan {
    fn file(&mut self, dir: &File, path: PathBuf, source: PiSkillSource) -> Result<()> {
        let Some(display) = path.to_str() else {
            return Ok(());
        };
        let name = path.file_name().context("Missing skill filename")?;
        let Ok((file, meta)) = open_markdown(dir, name) else {
            return Ok(());
        };
        let mut prefix = Vec::new();
        if file
            .take(HEADER_LIMIT as u64)
            .read_to_end(&mut prefix)
            .is_err()
        {
            return Ok(());
        }
        let text = match std::str::from_utf8(&prefix) {
            Ok(text) => text,
            Err(error) => std::str::from_utf8(&prefix[..error.valid_up_to()])?,
        };
        let Some(name) = skill_name(text, &path).filter(|name| policy_string(name)) else {
            return Ok(());
        };
        if self.paths.insert(path.clone()) {
            ensure!(self.entries.len() < 256, "too many Pi skills");
            self.entries.push(Entry {
                row: PiSkillRow {
                    id: format!("local:{}", hash(display.as_bytes())),
                    name,
                    source,
                    path: display.into(),
                    editable: meta.nlink() == 1 && !packaged_path(&path),
                    required: false,
                    shadowed: false,
                    enabled: true,
                },
                backing: Backing::Local(path),
            });
        }
        Ok(())
    }

    fn directory(
        &mut self,
        dir: &File,
        path: &Path,
        source: PiSkillSource,
        depth: usize,
    ) -> Result<()> {
        ensure!(depth <= 16, "Pi skill directory nesting is too deep");
        let mut children = Vec::new();
        for child in Dir::read_from(dir)? {
            let child = child?;
            if matches!(child.file_name().to_bytes(), b"." | b"..") {
                continue;
            }
            self.visited += 1;
            ensure!(self.visited <= 4096, "too many Pi skill directory entries");
            let Ok(name) = child.file_name().to_str() else {
                continue;
            };
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            children.push((name.to_owned(), child.file_type()));
        }
        children.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        if children
            .iter()
            .any(|(name, kind)| name == "SKILL.md" && *kind == FileType::RegularFile)
        {
            return self.file(dir, path.join("SKILL.md"), source);
        }
        for (name, kind) in children {
            if kind == FileType::Directory {
                if let Ok(sub) = openat(dir, name.as_str(), DIR, Mode::empty()) {
                    self.directory(&File::from(sub), &path.join(name), source, depth + 1)?;
                }
            } else if kind == FileType::RegularFile && depth == 0 && name.ends_with(".md") {
                self.file(dir, path.join(name), source)?;
            }
        }
        Ok(())
    }

    fn root(&mut self, path: &Path, source: PiSkillSource) -> Result<()> {
        if let Ok(dir) = open_dir(path) {
            let normalized: PathBuf = path.components().collect();
            if self.roots.insert(normalized.clone()) {
                self.directory(&dir, &normalized, source, 0)?;
            }
        }
        Ok(())
    }
}

fn select(root: &Path, project: Option<&str>, policy: &Policy) -> Result<Vec<Entry>> {
    let mut scan = Scan {
        entries: Vec::new(),
        paths: HashSet::new(),
        roots: HashSet::new(),
        visited: 0,
    };
    for &(name, required, relative, text) in SHIPPED {
        scan.entries.push(Entry {
            row: PiSkillRow {
                id: format!("shipped:{name}"),
                name: name.into(),
                source: if required {
                    PiSkillSource::Role
                } else {
                    PiSkillSource::Builtin
                },
                path: format!("corral-pi/{relative}"),
                enabled: true,
                editable: false,
                required,
                shadowed: false,
            },
            backing: Backing::Shipped(text),
        });
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    for dir in &policy.user_dirs {
        let path = if let Some(suffix) = dir.strip_prefix("~/") {
            let home = home
                .as_ref()
                .context("Cannot expand skills.user_dirs without home")?;
            home.join(suffix.trim_start_matches('/'))
        } else {
            PathBuf::from(dir)
        };
        ensure!(
            path.is_absolute(),
            "skills.user_dirs entries must be absolute or start with ~/"
        );
        let mut normalized = PathBuf::from("/");
        for part in path.components() {
            match part {
                Component::ParentDir => {
                    normalized.pop();
                }
                Component::Normal(name) => normalized.push(name),
                _ => (),
            }
        }
        let actual = normalized
            .canonicalize()
            .unwrap_or_else(|_| normalized.clone());
        ensure!(
            normalized != Path::new("/")
                && !home.as_ref().is_some_and(|home| {
                    home.canonicalize()
                        .unwrap_or_else(|_| home.clone())
                        .starts_with(&actual)
                }),
            "skills.user_dirs cannot include home or an ancestor of home"
        );
        scan.root(&normalized, PiSkillSource::User)?;
    }
    // In the next-generation workspace, only registry entries are trusted as project roots.
    if let Some(project) = project {
        let cfg = Config::load(&root.join(crate::STATE_DIR).join("corral.toml"))?;
        let path = cfg.effective_project(project)?.path;
        ensure!(
            !path.as_os_str().is_empty(),
            "registered project has no checkout directory"
        );
        scan.root(
            &root.join(path).join(".agents/skills"),
            PiSkillSource::Project,
        )?;
    }
    let mut names = HashSet::new();
    for entry in &mut scan.entries {
        // Project directories are resolved by the SDK in the session's actual worktree,
        // including its ignore files. Never infer a winning project file from this inventory.
        entry.row.shadowed = if entry.row.required {
            // Required procedures take precedence only in the roles that register them.
            false
        } else if entry.row.source == PiSkillSource::Project {
            names.contains(&entry.row.name)
        } else {
            !names.insert(entry.row.name.clone())
        };
        entry.row.enabled = !entry.row.shadowed
            && (entry.row.required || !policy.disabled.contains(&entry.row.name));
    }
    Ok(scan.entries)
}

fn resolve(root: &Path, project: Option<&str>, id: &str) -> Result<Entry> {
    let (_, doc) = pi_preferences::read(root)?;
    select(root, project, &policy(&doc)?)?
        .into_iter()
        .find(|entry| entry.row.id == id)
        .context("Unknown Pi skill identity")
}

// Refuse clearly rather than emitting a control frame the app cannot receive.
fn within_frame(value: &impl serde::Serialize) -> Result<()> {
    ensure!(
        serde_json::to_vec(value)?.len() <= corral_protocol::app::APP_CONTROL_MAX - 2048,
        "Pi skill data exceeds the app control frame limit"
    );
    Ok(())
}

pub(crate) fn catalog(root: &Path, req: PiSkillsRequest) -> Result<PiSkillsCatalog> {
    let (revision, doc) = pi_preferences::read(root)?;
    let skills = select(root, req.project.as_deref(), &policy(&doc)?)?
        .into_iter()
        .map(|entry| entry.row)
        .collect();
    let result = PiSkillsCatalog { skills, revision };
    within_frame(&result)?;
    Ok(result)
}

fn local_revision(text: &str, meta: &Metadata) -> String {
    hash(
        format!(
            "{}:{}:{}:{}:{}:{text}",
            meta.dev(),
            meta.ino(),
            meta.mode(),
            meta.ctime(),
            meta.ctime_nsec()
        )
        .as_bytes(),
    )
}

pub(crate) fn get(root: &Path, req: PiSkillGet) -> Result<PiSkillDetail> {
    let entry = resolve(root, req.project.as_deref(), &req.id)?;
    let (text, revision) = match entry.backing {
        Backing::Shipped(text) => (text.to_owned(), hash(text.as_bytes())),
        Backing::Local(path) => {
            let dir = open_dir(path.parent().context("Missing skill parent directory")?)?;
            let (text, meta) =
                read_markdown(&dir, path.file_name().context("Missing skill filename")?)?;
            ensure!(
                skill_name(&text, &path).as_deref() == Some(entry.row.name.as_str()),
                "{STALE}"
            );
            let revision = local_revision(&text, &meta);
            (text, revision)
        }
    };
    let result = PiSkillDetail {
        id: req.id,
        text,
        revision,
    };
    within_frame(&result)?;
    Ok(result)
}

struct Candidate<'a> {
    dir: &'a File,
    name: String,
    published: bool,
}
impl Drop for Candidate<'_> {
    fn drop(&mut self) {
        if !self.published {
            let _ = unlinkat(self.dir, self.name.as_str(), AtFlags::empty());
        }
    }
}

pub(crate) fn put(root: &Path, req: PiSkillPut) -> Result<PiSkillDetail> {
    within_frame(&req)?;
    let entry = resolve(root, req.project.as_deref(), &req.id)?;
    ensure!(entry.row.editable, "This Pi skill is read-only");
    let Backing::Local(path) = entry.backing else {
        anyhow::bail!("This Pi skill is read-only");
    };
    ensure!(
        skill_name(&req.text, &path).as_deref() == Some(entry.row.name.as_str()),
        "Skill Markdown must keep its name and a nonempty front matter description"
    );
    let parent = path.parent().context("Missing skill parent directory")?;
    let name = path.file_name().context("Missing skill filename")?;
    let dir = open_dir(parent)?;
    dir.try_lock()
        .context("Skill directory is being saved; try again")?;
    let (old, meta) = read_markdown(&dir, name)?;
    ensure!(
        meta.nlink() == 1
            && local_revision(&old, &meta) == req.revision
            && skill_name(&old, &path).as_deref() == Some(entry.row.name.as_str()),
        "{STALE}"
    );
    let temp_name = format!(".corral-skill-{:016x}.tmp", getrandom::u64()?);
    let mut file = File::from(openat(
        &dir,
        temp_name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )?);
    let mut temp = Candidate {
        dir: &dir,
        name: temp_name,
        published: false,
    };
    file.write_all(req.text.as_bytes())?;
    file.set_permissions(Permissions::from_mode(meta.mode() & 0o777))?;
    file.sync_all()?;

    // Re-select from live policy/registry and recheck identity and bytes before
    // publication. Rename never follows a substituted link or overwrites an inode.
    let current = resolve(root, req.project.as_deref(), &req.id)?;
    ensure!(
        current.row.editable
            && matches!(current.backing, Backing::Local(ref current) if *current == path),
        "Skill is no longer editable"
    );
    let live = open_dir(parent)?.metadata()?;
    let opened = dir.metadata()?;
    ensure!(
        (live.dev(), live.ino()) == (opened.dev(), opened.ino()),
        "{STALE}"
    );
    let (current, meta) = read_markdown(&dir, name)?;
    ensure!(
        meta.nlink() == 1 && local_revision(&current, &meta) == req.revision,
        "{STALE}"
    );
    let on_disk = rustix::fs::statat(&dir, temp.name.as_str(), AtFlags::SYMLINK_NOFOLLOW)?;
    let candidate = rustix::fs::fstat(&file)?;
    ensure!(
        on_disk.st_dev == candidate.st_dev
            && on_disk.st_ino == candidate.st_ino
            && on_disk.st_nlink == 1,
        "Skill candidate changed before publication"
    );
    renameat(&dir, temp.name.as_str(), &dir, name)?;
    temp.published = true;
    dir.sync_all()?;
    let revision = local_revision(&req.text, &file.metadata()?);
    Ok(PiSkillDetail {
        id: req.id,
        text: req.text,
        revision,
    })
}

pub(crate) fn toggle(root: &Path, req: PiSkillToggle) -> Result<PiSkillsCatalog> {
    let mut result = PiSkillsCatalog {
        skills: Vec::new(),
        revision: "0".repeat(64),
    };
    let revision = pi_preferences::save(
        root,
        &req.revision,
        "Pi skill policy changed; reload the catalog before saving",
        |doc| {
            let mut policy = policy(doc)?;
            let entries = select(root, req.project.as_deref(), &policy)?;
            let entry = entries
                .iter()
                .find(|entry| {
                    entry.row.name == req.name && !entry.row.shadowed && !entry.row.required
                })
                .context(
                    "No switchable skill with this name; role procedures are always required",
                )?;
            let unchanged = req.enabled == entry.row.enabled;
            result.skills = entries.into_iter().map(|entry| entry.row).collect();
            for row in &mut result.skills {
                if row.name == req.name && !row.shadowed && !row.required {
                    row.enabled = req.enabled;
                }
            }
            // Reserve room for the revision and result fields before committing the write.
            within_frame(&result)?;
            if unchanged {
                return Ok(());
            }
            policy.disabled.retain(|name| *name != req.name);
            if !req.enabled {
                policy.disabled.push(req.name.clone());
            }
            if doc.get("skills").is_none() {
                doc.insert("skills", Item::Table(Table::new()));
            }
            let table = doc
                .get_mut("skills")
                .and_then(Item::as_table_like_mut)
                .context("pi.toml skills must be a table")?;
            let mut value: Array = policy.disabled.iter().map(String::as_str).collect();
            if let Some(old) = table.get("disabled").and_then(Item::as_array) {
                *value.decor_mut() = old.decor().clone();
            }
            table.insert("disabled", toml_edit::value(value));
            Ok(())
        },
    )?;
    result.revision = revision;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};
    const BODY: &str = "---\nname: programming\ndescription: Test skill\n---\nOriginal\n";

    #[test]
    fn duplicate_user_root_spends_its_entry_budget_once() -> Result<()> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("SKILL.md"), BODY)?;
        let mut scan = Scan {
            entries: Vec::new(),
            paths: HashSet::new(),
            roots: HashSet::new(),
            visited: 4095,
        };
        scan.root(root.path(), PiSkillSource::User)?;
        scan.root(&root.path().join("."), PiSkillSource::User)?;
        assert_eq!(scan.visited, 4096);
        assert_eq!(scan.entries.len(), 1);
        Ok(())
    }

    #[test]
    fn catalog_save_and_toggle_use_pis_plain_scalar_name() -> Result<()> {
        const SKILL: &str =
            include_str!("../../../../packages/corral-pi/test/fixtures/skills/team/SKILL.md");
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let state = root.join(crate::STATE_DIR);
        fs::create_dir(&state)?;
        let skills = root.join("custom");
        fs::create_dir_all(skills.join("team"))?;
        let path = skills.join("team/SKILL.md");
        fs::write(&path, SKILL)?;
        const MERGE: &str =
            include_str!("../../../../packages/corral-pi/test/fixtures/skills/merge-name/SKILL.md");
        fs::create_dir(skills.join("merge-name"))?;
        fs::write(skills.join("merge-name/SKILL.md"), MERGE)?;
        fs::write(
            state.join("pi.toml"),
            format!(
                "[skills]\nuser_dirs = [{}]\n",
                toml_edit::Value::from(skills.to_str().unwrap())
            ),
        )?;
        let before = catalog(root, PiSkillsRequest { project: None })?;
        assert!(before.skills.iter().any(|r| r.name == "merge-name"));
        assert!(!before.skills.iter().any(|r| r.name == "<<"));
        let row = before.skills.iter().find(|r| r.name == "0b101").unwrap();
        let detail = get(
            root,
            PiSkillGet {
                project: None,
                id: row.id.clone(),
            },
        )?;
        let put_req = |text: String| PiSkillPut {
            project: None,
            id: row.id.clone(),
            revision: detail.revision.clone(),
            text,
        };
        assert!(put(root, put_req(SKILL.replace("name: 0b101", "name: team"))).is_err());
        let duplicate = SKILL.replace(
            "description:",
            "metadata: {version: 1, version: 2}\ndescription:",
        );
        let aliases = SKILL.replace(
            "description:",
            &format!(
                "label: &label shared\nmetadata: [{}]\ndescription:",
                ["*label"; 100].join(",")
            ),
        );
        for invalid in [duplicate, aliases, format!("\u{feff}\u{feff}{SKILL}")] {
            assert!(put(root, put_req(invalid)).is_err());
            assert_eq!(fs::read_to_string(&path)?, SKILL);
        }
        put(root, put_req(format!("{SKILL}\nUpdated.")))?;
        toggle(
            root,
            PiSkillToggle {
                project: None,
                name: row.name.clone(),
                enabled: false,
                revision: before.revision,
            },
        )?;
        assert_eq!(policy(&pi_preferences::read(root)?.1)?.disabled, ["0b101"]);
        Ok(())
    }

    #[test]
    fn project_edit_is_revision_checked_and_refuses_links() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let state = root.join(crate::STATE_DIR);
        let path = root.join("repo/.agents/skills/nested/demo/SKILL.md");
        fs::create_dir(&state)?;
        fs::create_dir_all(path.parent().context("Missing skill parent directory")?)?;
        fs::write(
            state.join("corral.toml"),
            "[projects.demo]\npath = 'repo'\n",
        )?;
        fs::write(&path, BODY)?;
        let skill_root = root.join("repo/.agents/skills");
        fs::write(
            skill_root.join("bad.md"),
            BODY.replace("programming", "\" padded \""),
        )?;
        fs::write(skill_root.join(".ignore"), "a/\n")?;
        for folder in ["a", "z"] {
            fs::create_dir(skill_root.join(folder))?;
            fs::write(
                skill_root.join(folder).join("SKILL.md"),
                BODY.replace("programming", "local-name"),
            )?;
        }
        let project = || Some("demo".into());
        let entries = catalog(root, PiSkillsRequest { project: project() })?.skills;
        assert!(!entries.iter().any(|e| e.name.contains("padded")));
        let candidates: Vec<_> = entries.iter().filter(|e| e.name == "local-name").collect();
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().all(|e| e.enabled && !e.shadowed));
        let entry = entries
            .iter()
            .find(|e| e.path == path.to_str().unwrap())
            .unwrap();
        assert!(entry.shadowed && entry.editable && !entry.enabled);
        let request = || PiSkillGet {
            project: project(),
            id: entry.id.clone(),
        };
        let before = get(root, request())?;
        assert_eq!(before.text, BODY);
        assert!(
            get(
                root,
                PiSkillGet {
                    project: None,
                    ..request()
                }
            )
            .is_err()
        );
        assert!(
            get(
                root,
                PiSkillGet {
                    id: path.display().to_string(),
                    ..request()
                }
            )
            .is_err()
        );
        let change = |rev: &str| PiSkillPut {
            project: project(),
            id: entry.id.clone(),
            text: BODY.replace("Original", "Updated"),
            revision: rev.into(),
        };
        assert!(
            put(
                root,
                PiSkillPut {
                    text: BODY.replace("programming", "renamed"),
                    ..change(&before.revision)
                }
            )
            .is_err()
        );
        let saved = put(root, change(&before.revision))?;
        assert_eq!(saved.text, fs::read_to_string(&path)?);
        assert_eq!(saved.revision, get(root, request())?.revision);
        assert_eq!(
            put(root, change(&before.revision))
                .err()
                .unwrap()
                .to_string(),
            STALE
        );
        let alias = root.join("alias");
        fs::hard_link(&path, &alias)?;
        assert!(put(root, change(&saved.revision)).is_err());
        fs::remove_file(&path)?;
        symlink(&alias, &path)?;
        assert!(get(root, request()).is_err());
        assert!(put(root, change(&saved.revision)).is_err());
        assert_eq!(fs::read_to_string(alias)?, saved.text);
        Ok(())
    }

    #[test]
    fn policy_toggles_preserve_unrelated_settings_and_required_procedures() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let state = root.join(crate::STATE_DIR);
        let user = root.join("user");
        fs::create_dir(&state)?;
        fs::create_dir(&user)?;
        fs::write(
            user.join("extra.md"),
            BODY.replace("programming", "Team-Notes"),
        )?;
        let path = state.join("pi.toml");
        let unrelated = "[hosted_tools]\nweb_search = 'off'\n";
        fs::write(
            &path,
            format!(
                "{unrelated}[skills]\nuser_dirs = [{}]\ndisabled = ['corral-worker']\n",
                toml_edit::Value::from(user.to_str().unwrap())
            ),
        )?;
        let before = catalog(root, PiSkillsRequest { project: None })?;
        let request = |name: &str, revision: &str| PiSkillToggle {
            project: None,
            name: name.into(),
            enabled: false,
            revision: revision.into(),
        };
        let shipped = get(
            root,
            PiSkillGet {
                project: None,
                id: "shipped:programming".into(),
            },
        )?;
        assert!(shipped.text.starts_with("---"));
        assert!(
            put(
                root,
                PiSkillPut {
                    project: None,
                    id: shipped.id,
                    text: BODY.into(),
                    revision: shipped.revision
                }
            )
            .is_err()
        );
        assert!(
            before
                .skills
                .iter()
                .any(|s| s.name == "Team-Notes" && s.editable)
        );
        assert!(
            before
                .skills
                .iter()
                .any(|s| s.name == "corral-worker" && s.enabled && s.required)
        );
        assert!(toggle(root, request("corral-worker", &before.revision)).is_err());
        let after = toggle(root, request("programming", &before.revision))?;
        assert!(
            after
                .skills
                .iter()
                .any(|s| s.name == "programming" && !s.enabled)
        );
        assert!(toggle(root, request("Team-Notes", &before.revision)).is_err());
        assert!(toggle(root, request("unknown", &after.revision)).is_err());
        let after = toggle(root, request("Team-Notes", &after.revision))?;
        assert!(
            after
                .skills
                .iter()
                .any(|s| s.name == "Team-Notes" && !s.enabled)
        );
        fs::write(
            user.join("worker.md"),
            BODY.replace("programming", "corral-worker"),
        )?;
        let mut enable = request("corral-worker", &after.revision);
        enable.enabled = true;
        let enabled = toggle(root, enable)?;
        let disabled = toggle(root, request("corral-worker", &enabled.revision))?;
        assert!(
            disabled
                .skills
                .iter()
                .any(|s| s.name == "corral-worker" && !s.required && !s.enabled)
        );
        assert!(
            disabled
                .skills
                .iter()
                .any(|s| s.name == "corral-worker" && s.required && s.enabled)
        );
        assert!(fs::read_to_string(&path)?.starts_with(unrelated));
        for invalid in ["relative", "/", "~/"] {
            let invalid_doc = format!("[skills]\nuser_dirs = ['{invalid}']\n");
            fs::write(&path, &invalid_doc)?;
            assert!(catalog(root, PiSkillsRequest { project: None }).is_err());
            let (revision, _) = pi_preferences::read(root)?;
            assert!(toggle(root, request("frontend", &revision)).is_err());
            assert_eq!(fs::read_to_string(&path)?, invalid_doc);
        }
        Ok(())
    }
}
