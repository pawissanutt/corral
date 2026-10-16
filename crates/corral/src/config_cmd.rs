use std::path::Path;

use anyhow::{Context, Result, bail};
use corral_core::config_doc::toml_edit::{DocumentMut, Item, Value};
use corral_core::{ConfigDoc, ConfigLayer, ConfigLayers, ConfigSource, Event, EventReader, Store};

use crate::ConfigCmd;

/// `corral config ...`. Finds the workspace without loading its config, so validation and
/// event inspection work even when the current file does not parse.
pub fn run(start: &Path, cmd: ConfigCmd) -> Result<()> {
    let root = Store::find_root(start)?;
    let path = root.join(".corral").join("corral.toml");
    match cmd {
        ConfigCmd::Diff { since, json } => {
            let mut events = config_events(&root, false, since)?;
            if since.is_none() && events.len() > 1 {
                events.drain(..events.len() - 1);
            }
            print_events(&events, json)?;
        }
        ConfigCmd::History { json } => {
            print_events(&config_events(&root, true, None)?, json)?;
        }
        ConfigCmd::Path => println!("{}", path.display()),
        ConfigCmd::Validate => {
            let layers = ConfigLayers::load(&path)?;
            layers.validate(&root)?;
            if let Some(user_path) = ConfigLayers::user_path() {
                println!("{}: ok (user; optional when absent)", user_path.display());
            }
            println!("{}: ok", path.display());
        }
        ConfigCmd::Get { key, source } => {
            let layers = ConfigLayers::load(&path)?;
            let config = layers.config()?;
            let value = config.effective_value()?;
            let effective = ConfigDoc::parse(&toml::to_string(&value)?)?;
            match effective.get(&key)? {
                Some(item) => {
                    println!("{}", render(item));
                    if source {
                        let layer = layers.source(&key)?;
                        match layer {
                            ConfigLayer::Default => println!("source: default (built-in)"),
                            ConfigLayer::Mixed => {
                                println!(
                                    "source: mixed (user, workspace and/or built-in defaults)"
                                );
                                for layer in [ConfigLayer::User, ConfigLayer::Workspace] {
                                    if let Ok(path) = layers.path(layer) {
                                        println!("  {layer}: {}", path.display());
                                    }
                                }
                            }
                            ConfigLayer::User | ConfigLayer::Workspace => {
                                println!("source: {layer} ({})", layers.path(layer)?.display());
                            }
                        }
                    }
                }
                None => bail!("{key} is not set"),
            }
        }
        ConfigCmd::Set {
            key, value, user, ..
        } => {
            let store = Store::open(&root)?;
            let layer = if user {
                ConfigLayer::User
            } else {
                ConfigLayer::Workspace
            };
            store.edit_config_layer_from(layer, ConfigSource::Cli, |doc| {
                doc.set_str(&key, &value)
            })?;
            let layers = store.config_layers()?;
            let doc = layers.doc(layer)?;
            if let Some(item) = doc.get(&key)? {
                println!("{key} = {}", render(item).trim_end());
                let effective_source = layers.source(&key)?;
                if layer == ConfigLayer::User && effective_source != ConfigLayer::User {
                    println!(
                        "effective source: {effective_source} (workspace overrides user preferences)"
                    );
                }
            }
        }
    }
    Ok(())
}

fn config_events(root: &Path, history: bool, since: Option<jiff::Timestamp>) -> Result<Vec<Event>> {
    let mut reader = EventReader::from_offset(&root.join(".corral/events.jsonl"), 0)?;
    let mut events = Vec::new();
    loop {
        let before = reader.read_position();
        match reader.next_event()? {
            Some((event, _)) => {
                if (event.kind == "config_changed" || history && event.kind == "config_invalid")
                    && since.is_none_or(|since| event.ts >= since)
                {
                    events.push(event);
                }
            }
            None if reader.read_position() == before => break,
            None => {}
        }
    }
    Ok(events)
}

fn print_events(events: &[Event], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(events)?);
        return Ok(());
    }
    if events.is_empty() {
        println!("No matching config events.");
    }
    for event in events {
        let detail = &event.detail;
        let source = detail["source"]
            .as_str()
            .context("config event is missing source")?;
        let hash = detail["hash"]
            .as_str()
            .context("config event is missing hash")?;
        println!("{} {} source={source} hash={hash}", event.ts, event.kind);
        if event.kind == "config_changed" {
            let changes = detail["changes"]
                .as_array()
                .context("config_changed event is missing changes")?;
            for change in changes {
                let key = change["key"]
                    .as_str()
                    .context("config change is missing key")?;
                let applies = change["applies"]
                    .as_str()
                    .context("config change is missing applies")?;
                println!(
                    "  {key}: {} -> {} (applies={applies}, pm_relevant={})",
                    change["old"], change["new"], change["pm_relevant"]
                );
            }
        } else {
            let error = detail["error"]
                .as_str()
                .context("config_invalid event is missing error")?;
            println!("  error: {error}");
        }
        if let Some(message) = detail["message"].as_str() {
            println!("  {message}");
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
