use anyhow::{Context, Result, ensure};
use clap::Args;
use corral_core::{ArtifactKind, ArtifactTier, Store};
use corral_ops::{ContentSearch, Ctx, search_content};

#[derive(Args)]
pub struct SearchArgs {
    /// Shared query language: quoted phrases, negation, test:, error:, verdict:
    #[arg(required_unless_present = "reindex")]
    query: Option<String>,
    /// Content scope; omitted preserves the task-result format
    #[arg(long = "in", value_parser = ["artifacts", "specs", "reviews", "all"])]
    scope: Option<String>,
    #[arg(long)]
    task: Option<String>,
    #[arg(long)]
    kind: Option<String>,
    #[arg(long, value_parser = ["keep", "normal", "transient"])]
    tier: Option<String>,
    /// Updated/added within Nh, Nd, Nw, or after a date or RFC3339 timestamp
    #[arg(long)]
    since: Option<String>,
    #[arg(long)]
    project: Option<String>,
    /// Maximum hits, capped at 100
    #[arg(long, default_value = "20", value_parser = clap::value_parser!(u32).range(0..=100))]
    limit: u32,
    /// Source lines before and after the matching line, capped at 20
    #[arg(long, default_value = "0", value_parser = clap::value_parser!(u32).range(0..=20))]
    context: u32,
    /// Rebuild the derived index from canonical stored content
    #[arg(long)]
    reindex: bool,
    #[arg(long)]
    json: bool,
}

pub fn run(ctx: &Ctx, args: SearchArgs) -> Result<()> {
    let query = args.query.as_deref().unwrap_or("");
    ensure!(query.len() <= 4096, "search query exceeds 4096 bytes");
    let options = ContentSearch {
        scope: args.scope.clone().unwrap_or_else(|| "all".into()),
        task: args.task.as_deref().map(Store::normalize_id).transpose()?,
        kind: args
            .kind
            .as_deref()
            .map(|k| ArtifactKind::parse(k).with_context(|| format!("unknown artifact kind '{k}'")))
            .transpose()?,
        tier: args
            .tier
            .as_deref()
            .map(|t| ArtifactTier::parse(t).context("unknown artifact tier"))
            .transpose()?,
        since: args.since,
        project: args.project,
        limit: usize::try_from(args.limit).context("limit overflow")?,
        context: usize::try_from(args.context).context("context overflow")?,
        reindex: args.reindex,
    };
    let content_mode = args.scope.is_some()
        || options.task.is_some()
        || options.kind.is_some()
        || options.tier.is_some()
        || options.since.is_some()
        || options.project.is_some()
        || options.context != 0
        || args.reindex;
    if content_mode {
        let hits = search_content(ctx, query, &options)?;
        if args.json {
            println!("{}", serde_json::to_string(&hits)?);
        } else {
            for hit in hits {
                println!(
                    "{} [{} {}] {}:{}-{}\n{}",
                    hit.reference,
                    hit.kind,
                    hit.tier.as_str(),
                    hit.task,
                    hit.line_start,
                    hit.line_end,
                    hit.snippet
                );
            }
        }
    } else {
        // Keep T-047's machine-consumed task shape for existing callers.
        let tasks = corral_ops::search_tasks(&ctx.store, query)?;
        if args.json {
            let rows: Vec<_> = tasks
                .iter()
                .take(options.limit)
                .map(|t| crate::cmds::task_output::task_json(t, false))
                .collect();
            println!("{}", serde_json::to_string(&rows)?);
        } else {
            crate::list_cols::print(ctx, &tasks[..tasks.len().min(options.limit)], &[])?;
        }
    }
    Ok(())
}
