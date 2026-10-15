//! `corral artifact show` and the importance-ordered artifact lines of `corral show` / `corral review`.

use std::io::{IsTerminal, Write};

use anyhow::{Context, Result};
use corral_core::{Artifact, ArtifactTier};
use corral_ops::{Ctx, Preview};
use corral_tui::render;

/// Prints an artifact for reading: the complete decoded document rendered like the TUI preview (ANSI colours
/// on a terminal, plain text when piped), or the decoded stored bytes with `raw`. Compressed snapshots are
/// always decoded through `artifact_read`; purged ones print their stub.
pub(crate) fn show(ctx: &Ctx, reference: &str, raw: bool) -> Result<()> {
    let a = corral_ops::resolve_artifact(ctx, reference)?;
    if raw {
        let bytes = corral_ops::artifact_read(ctx, &a)?;
        return std::io::stdout()
            .lock()
            .write_all(&bytes)
            .context("writing artifact content");
    }
    match corral_ops::artifact_document(ctx, &a)? {
        Preview::Text(text) => {
            let width = std::env::var("COLUMNS")
                .ok()
                .and_then(|c| c.parse::<u16>().ok())
                .unwrap_or(100);
            let name = std::path::Path::new(&a.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let lines = render::render(&render::detect(&name, a.kind), &text, width);
            if std::io::stdout().is_terminal() {
                print!("{}", render::to_ansi(&lines));
            } else {
                print!("{}", render::to_plain(&lines));
            }
        }
        Preview::Image { path, note } | Preview::External { path, note } => {
            println!("{note}\n{path}");
        }
    }
    Ok(())
}

/// Artifact lines for `corral show` / `corral review`: keep and normal items in importance order, then the
/// transient ones under a `raw (N)` fold line.
pub(crate) fn print_grouped(arts: &[Artifact], with_path: bool) {
    let mut ordered = arts.to_vec();
    corral_ops::order_artifacts(&mut ordered);
    let line = |a: &Artifact| {
        let purged = if a.purged_at.is_some() {
            " [purged]"
        } else {
            ""
        };
        if with_path {
            format!(
                "artifact {} [{}] {} -> {}{purged}",
                a.run,
                a.kind.as_str(),
                a.title,
                a.path
            )
        } else {
            format!(
                "artifact {} [{}] {}{purged}",
                a.run,
                a.kind.as_str(),
                a.title
            )
        }
    };
    let (raw, shown): (Vec<_>, Vec<_>) = ordered
        .iter()
        .partition(|a| a.tier() == ArtifactTier::Transient);
    for a in shown {
        println!("{}", line(a));
    }
    if !raw.is_empty() {
        println!("raw ({}): corral artifact list --tier transient", raw.len());
        for a in raw {
            println!("  {}", line(a));
        }
    }
}
