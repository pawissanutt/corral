//! Operator memory CLI. Bound agents use authenticated operations instead of shell authority.
use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use corral_core::memory::{MemoryEntry, MemoryRole, MemoryScope, MemorySelector};
use corral_ops::{
    Ctx,
    memory::{MemoryActor, authorize},
};
use std::{io::Read, path::PathBuf};

#[derive(Args)]
pub struct MemoryArgs {
    /// workspace, project, or role
    #[arg(long, global = true, default_value = "workspace", value_parser = ["workspace", "project", "role"])]
    scope: String,
    /// Registered project key (project scope) or pm/assistant/worker (role scope)
    #[arg(long, global = true)]
    key: Option<String>,
    #[command(subcommand)]
    cmd: MemoryCmd,
}
#[derive(Subcommand)]
enum MemoryCmd {
    List {
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Read {
        id: String,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 4096)]
        limit: usize,
    },
    /// Full MemoryEntry JSON replacement; author and time are stamped by Corral
    Put {
        id: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long, required_unless_present = "create", conflicts_with = "create")]
        expected_hash: Option<String>,
        #[arg(long)]
        create: bool,
    },
    Remove {
        id: String,
        #[arg(long)]
        expected_hash: String,
    },
}
pub fn run(ctx: &Ctx, entry: &crate::entry::Entry, args: MemoryArgs) -> Result<()> {
    ensure!(
        entry.decision_author(&ctx.store)? == "owner"
            && [
                "CORRAL_ASSISTANT",
                "CORRAL_PM",
                "CORRAL_AGENT_BOOTSTRAP",
                "CORRAL_TASK"
            ]
            .iter()
            .all(|key| std::env::var_os(key).is_none()),
        "bound agents must use authenticated memory operations"
    );
    let scope = match (args.scope.as_str(), args.key) {
        ("workspace", None) => MemoryScope::Workspace,
        ("project", Some(key)) => MemoryScope::Project(key),
        ("role", Some(key)) => MemoryScope::Role(match key.as_str() {
            "pm" => MemoryRole::Pm,
            "assistant" => MemoryRole::Assistant,
            "worker" => MemoryRole::Worker,
            _ => anyhow::bail!("role must be pm, assistant or worker"),
        }),
        _ => anyhow::bail!("workspace has no --key; project and role require --key"),
    };
    let write = matches!(args.cmd, MemoryCmd::Put { .. } | MemoryCmd::Remove { .. });
    authorize(&ctx.store, &MemoryActor::Operator, &scope, write)?;
    let selector = |id| MemorySelector {
        scope: scope.clone(),
        id,
    };
    let result = match args.cmd {
        MemoryCmd::List { offset, limit } => {
            serde_json::to_value(ctx.store.memory_list(&scope, offset, limit)?)?
        }
        MemoryCmd::Read { id, offset, limit } => {
            serde_json::to_value(ctx.store.memory_read(&selector(id), offset, limit)?)?
        }
        MemoryCmd::Put {
            id,
            file,
            expected_hash,
            ..
        } => {
            ensure!(
                std::fs::symlink_metadata(&file)
                    .with_context(|| format!("inspecting {}", file.display()))?
                    .is_file(),
                "memory input must be a regular JSON file, not a link or special file"
            );
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::fs::OpenOptionsExt;
                // Do not block if a regular input is replaced by a FIFO before open.
                options.custom_flags(0x800 | 0x20000); // O_NONBLOCK | O_NOFOLLOW
            }
            let input = options
                .open(&file)
                .with_context(|| format!("opening {}", file.display()))?;
            ensure!(
                input.metadata()?.is_file(),
                "memory input must be a regular JSON file"
            );
            let mut bytes = Vec::new();
            input
                .take(65537)
                .read_to_end(&mut bytes)
                .context("reading bounded memory JSON")?;
            ensure!(bytes.len() <= 65536, "memory JSON input exceeds 64 KiB");
            let value: MemoryEntry =
                serde_json::from_slice(&bytes).context("parsing MemoryEntry JSON")?;
            serde_json::to_value(ctx.store.memory_put(
                &selector(id),
                value,
                expected_hash.as_deref(),
                "owner",
                jiff::Timestamp::now(),
            )?)?
        }
        MemoryCmd::Remove { id, expected_hash } => {
            ctx.store.memory_remove(&selector(id), &expected_hash)?;
            serde_json::json!({"removed": true})
        }
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
