use std::path::PathBuf;

use anyhow::{Result, bail};
use corral_core::{Pin, PinRef, PinTarget, pin_position, resolve_pin};
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

pub fn list(ctx: &Ctx, json: bool) -> Result<()> {
    let pins = ctx.store.resolved_pins()?;
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
    let pin = match pin_position(&pins, query) {
        Ok(i) => pins[i].clone(),
        Err(e) if query.trim().chars().all(|c| c.is_ascii_digit()) => return Err(e),
        Err(_) => Pin {
            target: PinRef::parse(query)?.to_string(),
            label: None,
            follow: None,
        },
    };
    let tasks = ctx.store.list()?;
    let path: PathBuf = match resolve_pin(&pin, &tasks, ctx.store.dir()) {
        PinTarget::Task { path, .. } | PinTarget::Artifact { path, .. } => path,
        PinTarget::Broken(reason) => bail!("{} is broken: {reason}", pin.target),
    };
    let opened = corral_ops::open_path(ctx, &path, true)?;
    println!("opened {} with {}", path.display(), opened.command());
    Ok(())
}
