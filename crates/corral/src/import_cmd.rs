use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use corral_core::{Origin, Store};
use corral_ops::Ctx;

#[derive(Args)]
pub struct ImportArgs {
    /// Markdown plan with YAML frontmatter describing its child tasks
    file: PathBuf,
    /// Preview planned IDs and dependencies without writing to .corral/
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    json: bool,
    /// Who filed this plan; PMs and agents should pass their own origin
    #[arg(long, default_value = "owner", value_parser = crate::idea_cmd::parse_origin)]
    origin: Origin,
}

pub fn run(root: &Option<PathBuf>, args: ImportArgs) -> Result<()> {
    let root = match root {
        Some(root) => root.clone(),
        None => Store::find_root(&crate::cmds::start_dir(root)?)?,
    };
    // Ordinary opens observe config edits, creating state and locks even for a preview.
    let ctx = Ctx::from_store(Store::open_unobserved(&root)?);
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

    use clap::Parser;

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
        let cli = Cli::try_parse_from([
            std::ffi::OsStr::new("corral"),
            std::ffi::OsStr::new("--root"),
            dir.path().as_os_str(),
            std::ffi::OsStr::new("import"),
            plan.as_os_str(),
            std::ffi::OsStr::new("--dry-run"),
            std::ffi::OsStr::new("--json"),
        ])
        .unwrap();

        // When running the actual CLI dispatch, including context opening.
        crate::cmds::run(cli).unwrap();

        // Then every directory, file, byte, modification time and permission is unchanged.
        assert_eq!(snapshot(store.dir()), before);
    }
}
