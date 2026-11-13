//! Explicit maintenance/status commands. Normal queries never enter the foreground loop.
use std::fs::{self, File};
use std::io::BufReader;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::{
    Phase, Progress, Step, advance_with_cache, current, load_prefix_cache, published,
    read_manifest, request_reindex, root,
};
use crate::Ctx;

pub(crate) fn build_foreground(ctx: &Ctx, reindex: bool) -> Result<Value> {
    ensure!(
        super::super::enabled(ctx),
        "search catalog is disabled; enable search.catalog or set CORRAL_SEARCH_CATALOG=1"
    );
    let _span = corral_core::timing::span("search.catalog.build");
    let mut ctx = ctx.clone();
    if ctx.store.is_snapshot() {
        ctx.store = corral_core::Store::open_unobserved(ctx.store.root())
            .context("opening catalog maintenance store")?;
    }
    if reindex {
        request_reindex(&ctx)?;
    } else if published(&ctx)?.is_some() {
        return status(&ctx);
    }
    clear_error(&ctx)?;
    let cache = load_prefix_cache(&ctx)?;
    loop {
        ctx.reload_config()
            .context("reloading catalog maintenance config")?;
        ensure!(
            super::super::enabled(&ctx),
            "search catalog disabled; completed batches remain checkpointed"
        );
        match advance_with_cache(&ctx, Some(&cache)) {
            Ok(Step::More) => {}
            Ok(Step::Ready) => {
                clear_error(&ctx)?;
                return status(&ctx);
            }
            Ok(Step::Busy) => anyhow::bail!(
                "search catalog builder is active in another process; use --catalog-status to inspect its progress"
            ),
            Err(error) => {
                if let Err(recording) = record_error(&ctx, &error) {
                    eprintln!("could not record catalog build error: {recording:#}");
                }
                return Err(error);
            }
        }
    }
}

pub(crate) fn status(ctx: &Ctx) -> Result<Value> {
    let root = root(ctx)?;
    let enabled = super::super::enabled(ctx);
    let manifest = read_manifest(&root)?;
    let progress = match File::open(root.join("progress.json")) {
        Ok(file) => Some(
            serde_json::from_reader::<_, Progress>(BufReader::new(file))
                .context("decoding catalog progress")?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("reading catalog progress"),
    };
    let last_error = match fs::read_to_string(root.join("last-error.txt")) {
        Ok(error) => Some(error.trim_end().to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("reading catalog build diagnostic"),
    };
    let ready = enabled && published(ctx)?.is_some();
    let state = if !enabled {
        "disabled"
    } else if ready {
        "ready"
    } else if last_error.is_some() {
        "deferred"
    } else if progress.is_some() {
        "incomplete"
    } else if manifest.is_some() {
        "stale"
    } else {
        "missing"
    };
    let phase = progress.as_ref().map(|progress| match progress.phase {
        Phase::Headers(_) => "headers",
        Phase::Tasks => "tasks",
        Phase::Events => "events",
        Phase::Follow => "follow",
        Phase::Publish => "publish",
    });
    let metadata_cursor = progress
        .as_ref()
        .and_then(|progress| progress.meta.as_ref())
        .map(|pending| serde_json::to_value(&pending.scan))
        .transpose()
        .context("encoding catalog metadata cursor")?;
    let source_cursor = progress
        .as_ref()
        .and_then(|progress| progress.index.as_ref())
        .map(|pending| {
            json!({ "task": pending.id, "input": pending.input,
            "byte_offset": pending.offset, "output_bytes": pending.bytes })
        });
    let build_sequence = progress.as_ref().map(|progress| progress.manifest.after);
    let build_event_sequence = progress
        .as_ref()
        .map(|progress| progress.manifest.events_after);
    let published_sequence = manifest.as_ref().map(|manifest| manifest.after);
    let published_event_sequence = manifest.as_ref().map(|manifest| manifest.events_after);
    let published_current = match manifest.as_ref() {
        Some(manifest) => current(ctx, manifest.after)?,
        None => false,
    };
    Ok(json!({
        "enabled": enabled, "state": state, "ready": ready,
        "cache_root": root.display().to_string(),
        "progress_file": root.join("progress.json").display().to_string(),
        "manifest_file": root.join("manifest.json").display().to_string(),
        "error_file": root.join("last-error.txt").display().to_string(),
        "phase": phase, "queued_tasks": progress.as_ref().map(|progress| progress.tasks.len()),
        "metadata_cursor": metadata_cursor, "source_cursor": source_cursor,
        "indexed_sequence": build_sequence, "indexed_event_sequence": build_event_sequence,
        "published_sequence": published_sequence, "published_event_sequence": published_event_sequence,
        "published_current": published_current, "restart_requested": root.join("restart").is_file(),
        "last_error": last_error,
    }))
}

pub(crate) fn record_error(ctx: &Ctx, error: &anyhow::Error) -> Result<()> {
    let root = root(ctx)?;
    fs::create_dir_all(&root).context("creating catalog diagnostic directory")?;
    let message = format!("{error:#}").replace(['\r', '\n'], " ");
    crate::artifact_storage::replace(&root.join("last-error.txt"), message.as_bytes())
}

pub(crate) fn clear_error(ctx: &Ctx) -> Result<()> {
    match fs::remove_file(root(ctx)?.join("last-error.txt")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("clearing catalog build diagnostic"),
    }
}
