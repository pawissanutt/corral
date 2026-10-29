//! Disposable QA fixture writer, using the same layout-independent hooks as Rust tests.
use std::io::{self, Read};
use std::path::Path;

use anyhow::{Context, Result, bail};
use corral_core::Store;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let root = Path::new(args.get(1).context("missing fixture root")?);
    let operation = args.get(2).context("missing fixture operation")?;
    if std::fs::read_to_string(root.join(".corral-qa-fixture"))
        .context("reading synthetic QA fixture marker")?
        != "synthetic-only\n"
    {
        bail!("refusing a store not marked as a synthetic QA fixture");
    }
    let store = Store::open(root).context("opening synthetic QA store")?;
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("reading fixture mutation")?;
    match operation.as_str() {
        "put-task" => {
            store.fixture_put_task_text("qa-fixture", &input)?;
        }
        "append-events" => store.fixture_append_events(&input)?,
        "replace-events" => store.fixture_replace_events(&input)?,
        "remove-task" => store.fixture_remove_task(&input)?,
        "corrupt-task" => {
            store.fixture_corrupt_task(&input, "qa-invalid-status")?;
        }
        "tour-seen" => {
            corral_core::records::RecordArea::new("tour-seen").save(
                &store,
                "workspace",
                &"2026-10-17T00:00:00Z",
            )?;
        }
        "tour-reset" => {
            corral_core::records::RecordArea::<String>::new("tour-seen")
                .delete(&store, "workspace")?;
        }
        "reviewed-artifacts" => {
            let paths: std::collections::BTreeSet<_> = input.lines().collect();
            corral_core::records::RecordArea::new("reviewed-artifacts")
                .save(&store, "set", &paths)?;
        }
        "cache-root" => println!("{}", store.cache_root()?.display()),
        _ => bail!("unknown fixture operation {operation}"),
    }
    Ok(())
}
