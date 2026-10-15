use std::path::Path;

use anyhow::{Context, Result, bail};
use corral_core::config_doc::toml_edit::{DocumentMut, Item, Value};
use corral_core::{ConfigDoc, Store};

use crate::ConfigCmd;

/// `corral config ...`. Finds the workspace without loading its config, so `validate` can report a file that
/// does not parse.
pub fn run(start: &Path, cmd: ConfigCmd) -> Result<()> {
    let root = Store::find_root(start)?;
    let path = root.join(".corral").join("corral.toml");
    match cmd {
        ConfigCmd::Path => println!("{}", path.display()),
        ConfigCmd::Validate => {
            let config = ConfigDoc::load(&path)?
                .config()
                .with_context(|| format!("{} does not match the schema", path.display()))?;
            config
                .validate(&root)
                .with_context(|| format!("checking {}", path.display()))?;
            println!("{}: ok", path.display());
        }
        ConfigCmd::Get { key } => {
            let config = ConfigDoc::load(&path)?
                .config()
                .with_context(|| format!("{} does not match the schema", path.display()))?;
            let effective = ConfigDoc::parse(&config.to_toml())?;
            match effective.get(&key)? {
                Some(item) => println!("{}", render(item)),
                None => bail!("{key} is not set"),
            }
        }
        ConfigCmd::Set { key, value } => {
            let store = Store::open(&root)?;
            store.edit_config(|doc| doc.set_str(&key, &value))?;
            let doc = ConfigDoc::load(&path)?;
            if let Some(item) = doc.get(&key)? {
                println!("{key} = {}", render(item).trim_end());
            }
        }
    }
    Ok(())
}

/// Strings print bare, other values as TOML, tables as a TOML document.
fn render(item: &Item) -> String {
    match item {
        Item::Value(Value::String(s)) => s.value().clone(),
        Item::Value(v) => {
            let mut v = v.clone();
            v.decor_mut().clear();
            v.to_string()
        }
        Item::Table(t) => DocumentMut::from(t.clone()).to_string(),
        other => other.to_string(),
    }
}
