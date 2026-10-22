//! Compile against the unmodified schema-2 core before running recovery_schema tests.
use std::io::{self, BufRead, Write};
use std::path::Path;

use corral_core::{Store, STORE_SCHEMA_VERSION};

fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        STORE_SCHEMA_VERSION == 2,
        "fixture requires the schema-2 core"
    );
    let root = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("missing root"))?;
    let store = Store::open_unobserved(Path::new(&root))?;
    let task = store.get("T-001")?;
    println!("schema2-ready");
    io::stdout().flush()?;
    let mut command = String::new();
    io::stdin().lock().read_line(&mut command)?;
    anyhow::ensure!(command.trim() == "save", "unexpected command");
    match store.save(&task) {
        Ok(()) => anyhow::bail!("old writer unexpectedly saved"),
        Err(_) => println!("schema2-refused"),
    }
    Ok(())
}
