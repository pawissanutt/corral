//! Copy one exported, recorded task into a disposable store for read-only Pi history QA.
use anyhow::{Context, Result, ensure};
use corral_core::{Store, Task};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let root = PathBuf::from(args.next().context("private output directory required")?);
    let source = PathBuf::from(args.next().context("recorded task JSON required")?);
    ensure!(!root.join(".corral").exists(), "refusing an existing store");
    let bytes = std::fs::read(&source).context("read recorded task export")?;
    let mut task: Task = serde_json::from_slice(&bytes).context("parse recorded task export")?;
    let store = Store::init(&root)?;
    // The file is a new private copy; transcript locators keep their original read-only subjects.
    task.path = root.join(".corral/tasks").join(format!("{}.md", task.id));
    store.save(&task)?;
    println!("{}", task.id);
    Ok(())
}
