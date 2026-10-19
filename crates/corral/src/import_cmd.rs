use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use corral_core::{Origin, Store};
use corral_ops::Ctx;

#[derive(Args)]
pub struct ImportArgs {
    /// Markdown plan with YAML frontmatter describing its child tasks
    file: PathBuf,
    /// Amend only an existing tracker's approved scope, retaining its members
    #[arg(long, requires = "expected_revision")]
    amend: Option<String>,
    /// Exact approved_scope_revision from corral show (initially zero)
    #[arg(long, requires = "amend")]
    expected_revision: Option<u64>,
    /// Preview planned IDs and dependencies without writing to .corral/
    #[arg(long, conflicts_with = "amend")]
    dry_run: bool,
    #[arg(long)]
    json: bool,
    /// Who filed this plan; PMs and agents should pass their own origin
    #[arg(long, default_value = "owner", value_parser = crate::idea_cmd::parse_origin, conflicts_with = "amend")]
    origin: Origin,
}

pub fn run(root: &Option<PathBuf>, args: ImportArgs, entry: &crate::entry::Entry) -> Result<()> {
    let root = match root {
        Some(root) => root.clone(),
        None => Store::find_root(&crate::cmds::start_dir(root)?)?,
    };
    // Ordinary opens observe config edits, creating state and locks even for a preview.
    let ctx = if args.dry_run {
        let store = Store::open_unobserved(&root)?;
        entry.apply(&store)?;
        Ctx::from_store(store)
    } else {
        entry.open(&root)?
    };
    if let Some(id) = &args.amend {
        let revision = args
            .expected_revision
            .context("scope amendment needs --expected-revision")?;
        let task = corral_ops::amend_plan_scope(&ctx, id, &args.file, revision)?;
        if args.json {
            println!(
                "{}",
                serde_json::to_string(&task).context("serializing amended Plan")?
            );
        } else {
            println!(
                "Tracker: {} {} (scope revision {})",
                task.id, task.title, task.approved_scope_revision
            );
        }
        return Ok(());
    }
    let result = corral_ops::import_plan(&ctx, &args.file, args.dry_run, args.origin)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string(&result).context("serializing imported plan")?
        );
    } else {
        if result.dry_run {
            println!("Preview only; no files written. IDs may change before import.");
        }
        println!("Tracker: {} {}", result.tracker.id, result.tracker.title);
        for task in &result.tasks {
            println!("  {} {}", task.id, task.title);
        }
        println!("Dependencies (task -> prerequisite):");
        for task in std::iter::once(&result.tracker).chain(&result.tasks) {
            for dependency in &task.depends_on {
                println!("  {} -> {dependency}", task.id);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;
    use std::time::SystemTime;

    use clap::{CommandFactory, FromArgMatches, Parser};

    use super::*;
    use crate::{Cli, Cmd};

    #[test]
    fn import_defaults_to_owner_when_options_are_omitted() {
        // Given a plan path without flags.
        let cli = Cli::try_parse_from(["corral", "import", "plan.md"]).unwrap();
        // When extracting the parsed import.
        let Some(Cmd::Import(args)) = cli.cmd else {
            panic!("expected import");
        };
        // Then the file and defaults are preserved.
        assert_eq!(args.file, PathBuf::from("plan.md"));
        assert_eq!(args.origin, Origin::Owner);
        assert!(!args.dry_run);
        assert!(!args.json);
    }

    #[test]
    fn import_parses_preview_json_and_each_origin() {
        // Given each supported filing origin.
        for (value, origin) in [
            ("owner", Origin::Owner),
            ("pm", Origin::Pm),
            ("agent", Origin::Agent),
        ] {
            // When parsing the preview flags.
            let cli = Cli::try_parse_from([
                "corral",
                "import",
                "plan.md",
                "--dry-run",
                "--json",
                "--origin",
                value,
            ])
            .unwrap();
            // Then all options reach the typed command.
            let Some(Cmd::Import(args)) = cli.cmd else {
                panic!("expected import");
            };
            assert_eq!(args.origin, origin);
            assert!(args.dry_run);
            assert!(args.json);
        }
    }

    #[test]
    fn import_rejects_missing_file_and_unknown_origin() {
        // Given malformed command lines.
        for words in [
            vec!["corral", "import"],
            vec!["corral", "import", "plan.md", "--origin", "unknown"],
        ] {
            // When parsing, then invalid inputs are refused.
            assert!(Cli::try_parse_from(words).is_err());
        }
    }

    #[test]
    fn scope_amendment_requires_exact_revision_and_refuses_creation_flags() {
        // Given the existing import command used as an explicit amendment boundary.
        let cli = Cli::try_parse_from([
            "corral",
            "import",
            "scope.md",
            "--amend",
            "T-004",
            "--expected-revision",
            "0",
        ])
        .unwrap();
        let Some(Cmd::Import(args)) = cli.cmd else {
            panic!("expected import")
        };
        assert_eq!(args.amend.as_deref(), Some("T-004"));
        assert_eq!(args.expected_revision, Some(0));
        // When its revision is missing or it mixes amendment with filing/preview options.
        for words in [
            vec!["corral", "import", "scope.md", "--amend", "T-004"],
            vec!["corral", "import", "scope.md", "--expected-revision", "0"],
            vec![
                "corral",
                "import",
                "scope.md",
                "--amend",
                "T-004",
                "--expected-revision",
                "0",
                "--dry-run",
            ],
            vec![
                "corral",
                "import",
                "scope.md",
                "--amend",
                "T-004",
                "--expected-revision",
                "0",
                "--origin",
                "agent",
            ],
        ] {
            // Then incomplete or ambiguous authorization is rejected at argument parsing.
            assert!(Cli::try_parse_from(words).is_err());
        }
    }

    #[derive(Debug, PartialEq)]
    struct Entry {
        contents: Option<Vec<u8>>,
        modified: SystemTime,
        permissions: fs::Permissions,
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
        let mut entries = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(path) = pending.pop() {
            let metadata = fs::metadata(&path).unwrap();
            if metadata.is_dir() {
                pending.extend(
                    fs::read_dir(&path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
            }
            entries.insert(
                path.strip_prefix(root).unwrap().to_path_buf(),
                Entry {
                    contents: metadata.is_file().then(|| fs::read(&path).unwrap()),
                    modified: metadata.modified().unwrap(),
                    permissions: metadata.permissions(),
                },
            );
        }
        entries
    }

    #[test]
    fn dry_run_leaves_corral_unchanged_even_with_unobserved_config_edit() {
        // Given a real store with a config change ordinary opens would observe.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let mut config = store.config.clone();
        config.limits.running += 1;
        fs::write(store.dir().join("corral.toml"), config.to_toml()).unwrap();
        let plan = dir.path().join("plan.md");
        fs::write(
            &plan,
            "---\ntitle: Preview plan\ntasks:\n  - key: first\n    title: First child\n  - key: second\n    title: Second child\n    depends: [first]\n---\nTracker specification.\n",
        )
        .unwrap();
        let before = snapshot(store.dir());
        let matches = Cli::command()
            .try_get_matches_from([
                std::ffi::OsStr::new("corral"),
                std::ffi::OsStr::new("--root"),
                dir.path().as_os_str(),
                std::ffi::OsStr::new("import"),
                plan.as_os_str(),
                std::ffi::OsStr::new("--dry-run"),
                std::ffi::OsStr::new("--json"),
            ])
            .unwrap();
        let cli = Cli::from_arg_matches(&matches).unwrap();

        // When running the actual CLI dispatch, including context opening.
        crate::cmds::run(cli, &matches).unwrap();

        // Then every directory, file, byte, modification time and permission is unchanged.
        assert_eq!(snapshot(store.dir()), before);
    }
}
