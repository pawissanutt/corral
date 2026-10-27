use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use corral_core::{Pin, PinRef, PinTarget, pin_position};
use corral_ops::Ctx;
use serde_json::{Value, json};

pub fn pin(ctx: &Ctx, target: &str, label: Option<&str>, follow_latest: bool) -> Result<()> {
    let (n, pin) = ctx.store.pin(target, label, follow_latest)?;
    let follow = if pin.follow.is_some() {
        " (follows latest)"
    } else {
        ""
    };
    println!("pinned #{n} {}{follow}", pin.target);
    Ok(())
}

pub fn unpin(ctx: &Ctx, query: &str) -> Result<()> {
    let pin = ctx.store.unpin(query)?;
    println!("unpinned {}", pin.target);
    Ok(())
}

fn pin_json(n: usize, pin: &Pin, target: &PinTarget) -> Value {
    let mut v = json!({
        "index": n,
        "ref": pin.target,
        "label": pin.label,
        "follow": pin.follow,
    });
    let extra = match target {
        PinTarget::Task {
            id,
            title,
            status,
            path,
        } => json!({
            "state": "ok", "kind": "task", "task": id, "title": title,
            "status": status.as_str(), "path": path,
        }),
        PinTarget::Artifact {
            task,
            artifact,
            path,
            followed,
        } => json!({
            "state": "ok", "kind": "artifact", "task": task, "title": artifact.title,
            "resolved": PinRef::of_artifact(task, artifact).to_string(), "followed": followed,
            "path": path, "artifact": artifact,
        }),
        PinTarget::Broken(reason) => json!({ "state": "broken", "reason": reason }),
    };
    if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
        o.extend(e);
    }
    v
}

/// Whether an artifact's bytes exist: its legacy snapshot, or else its live blob file.
fn content_held(ctx: &Ctx, artifact: &corral_core::Artifact) -> Result<bool> {
    if ctx.store.dir().join(&artifact.path).is_file() {
        return Ok(true);
    }
    let Ok(hash) = corral_core::blob_ref::BlobHash::parse(&artifact.sha256) else {
        return Ok(false);
    };
    let live = ctx
        .store
        .blob_row(&hash)?
        .is_some_and(|row| row.state == corral_core::backend::BlobState::Live);
    Ok(live && ctx.store.blob_store()?.path_of(&hash).is_file())
}

/// A pin the core resolver calls broken because its legacy snapshot is gone still resolves
/// while the artifact's blob holds its content; `number` is its 1-based pin position, if any.
fn blob_backed(
    ctx: &Ctx,
    number: Option<usize>,
    pin: &Pin,
    target: PinTarget,
) -> Result<PinTarget> {
    let PinTarget::Broken(_) = &target else {
        return Ok(target);
    };
    let Ok(PinRef::Artifact { task, .. }) = PinRef::parse(&pin.target) else {
        return Ok(target);
    };
    let reference = number.map_or_else(|| pin.target.clone(), |n| n.to_string());
    let (Ok(artifact), Ok(anchor)) = (
        corral_ops::resolve_artifact(ctx, &reference),
        corral_ops::resolve_artifact(ctx, &pin.target),
    ) else {
        return Ok(target);
    };
    if !content_held(ctx, &artifact)? {
        return Ok(target);
    }
    Ok(PinTarget::Artifact {
        task,
        path: ctx.store.dir().join(&artifact.path),
        followed: artifact.path != anchor.path,
        artifact,
    })
}

pub fn list(ctx: &Ctx, json: bool) -> Result<()> {
    let pins = ctx
        .store
        .resolved_pins()?
        .into_iter()
        .enumerate()
        .map(|(i, (pin, target))| {
            let target = blob_backed(ctx, Some(i + 1), &pin, target)?;
            Ok((pin, target))
        })
        .collect::<Result<Vec<_>>>()?;
    if json {
        let v: Vec<Value> = pins
            .iter()
            .enumerate()
            .map(|(i, (p, t))| pin_json(i + 1, p, t))
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    for (i, (p, t)) in pins.iter().enumerate() {
        let (name, detail) = match t {
            PinTarget::Task { title, status, .. } => {
                (title.clone(), format!("[{}]", status.label()))
            }
            PinTarget::Artifact {
                task,
                artifact,
                followed,
                ..
            } => {
                let mut d = format!("[{}]", artifact.kind.as_str());
                if *followed {
                    d.push_str(&format!(" latest: {}", PinRef::of_artifact(task, artifact)));
                }
                (artifact.title.clone(), d)
            }
            PinTarget::Broken(reason) => (String::from("-"), format!("BROKEN: {reason}")),
        };
        let name = p.label.clone().unwrap_or(name);
        let follow = if p.follow.is_some() { " (follow)" } else { "" };
        println!("{:>2}  {name}  {}{follow}  {detail}", i + 1, p.target);
    }
    Ok(())
}

pub fn open(ctx: &Ctx, query: &str) -> Result<()> {
    let pins = ctx.store.pins()?;
    let (number, pin) = match pin_position(&pins, query) {
        Ok(i) => (Some(i + 1), pins[i].clone()),
        Err(e) if query.trim().chars().all(|c| c.is_ascii_digit()) => return Err(e),
        Err(_) => (
            None,
            Pin {
                target: PinRef::parse(query)?.to_string(),
                label: None,
                follow: None,
            },
        ),
    };
    let path: PathBuf = match PinRef::parse(&pin.target)? {
        // Artifacts resolve from the pinned task's artifact rows and open as their decoded copy
        // under the cache root.
        PinRef::Artifact { .. } => {
            let reference = number.map_or_else(|| pin.target.clone(), |n| n.to_string());
            let artifact = corral_ops::resolve_artifact(ctx, &reference)
                .with_context(|| format!("{} is broken", pin.target))?;
            if artifact.purged_at.is_none() && !content_held(ctx, &artifact)? {
                bail!(
                    "{} is broken: snapshot {} is missing",
                    pin.target,
                    artifact.path
                );
            }
            corral_ops::artifact_path(ctx, &artifact)?
        }
        // The store keeps no task files: open a read-only Markdown rendering of the task.
        PinRef::Task(task) => {
            let task = ctx.store.get(&task)?;
            let dir = ctx.store.state_dir().join("open");
            std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            let path = dir.join(format!("{}.md", task.id));
            std::fs::write(&path, corral_core::Store::render_task_markdown(&task))
                .with_context(|| format!("writing {}", path.display()))?;
            path
        }
    };
    let opened = corral_ops::open_path(ctx, &path, true)?;
    println!("opened {} with {}", path.display(), opened.command());
    Ok(())
}
