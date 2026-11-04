use anyhow::{Context, Result};
use corral_ops::Ctx;

#[derive(clap::Args)]
pub struct ReportArgs {
    /// Include dispatches started in the last N hours, days or weeks
    #[arg(long, default_value = "30d")]
    pub since: String,
    /// Only this project's tasks
    #[arg(long)]
    pub project: Option<String>,
    /// Open the pinned page through the loopback artifact server
    #[arg(long)]
    pub open: bool,
    /// Print the analytics model as JSON
    #[arg(long)]
    pub json: bool,
    /// Write only the page to this path: no usage refresh, artifact or pin
    #[arg(long, hide = true)]
    pub out: Option<std::path::PathBuf>,
}

pub fn run(ctx: &Ctx, args: ReportArgs) -> Result<()> {
    if let Some(path) = &args.out {
        let (model, page) = corral_ops::render_report_at(
            ctx,
            &args.since,
            args.project.as_deref(),
            corral_core::clock::now(),
        )?;
        std::fs::write(path, page).with_context(|| format!("writing report {}", path.display()))?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&model)?);
        }
        return Ok(());
    }
    let output = corral_ops::report(ctx, &args.since, args.project.as_deref())?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&output.model)?);
    } else if let Some(artifact_ref) = &output.artifact_ref {
        println!("Pinned report: {artifact_ref}");
    }
    if args.open
        && let Some(artifact) = &output.artifact
    {
        let opened = corral_ops::open_artifact_live(ctx, artifact)?;
        match &opened.url {
            Some(url) => eprintln!("Serving {url}"),
            None => eprintln!("Opened {}", opened.command()),
        }
    }
    Ok(())
}
