//! `corral plugin install`: writes the herdr plugin manifest and links it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_ops::Ctx;

#[path = "plugin_config.rs"]
mod config;
pub use config::{config_path, keybindings_present, merge_keybindings};

pub use corral_herdr::MIN_HERDR_VERSION;

fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn arr(items: &[&str]) -> String {
    let parts: Vec<String> = items.iter().map(|s| q(s)).collect();
    format!("[{}]", parts.join(", "))
}

fn find_herdr() -> String {
    if let Ok(p) = std::env::var("HERDR_BIN_PATH")
        && !p.is_empty()
    {
        return p;
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join("herdr");
            if cand.is_file() {
                return cand.to_string_lossy().into_owned();
            }
        }
    }
    "herdr".to_string()
}

/// Renders the manifest for the given absolute exe, workspace root and herdr binary.
pub fn manifest(exe: &str, root: &str, herdr: &str) -> String {
    let mut m = String::new();
    m.push_str(&format!(
        "id = \"corral\"\nname = \"corral\"\nversion = \"0.1.0\"\nmin_herdr_version = \"{MIN_HERDR_VERSION}\"\n",
    ));
    m.push_str("platforms = [\"linux\", \"macos\"]\n\n");
    m.push_str(&format!(
        "[[events]]\non = \"pane.agent_status_changed\"\ncommand = {}\n\n",
        arr(&[exe, "--root", root, "hook"])
    ));
    /// (id, title, placement, width, height as text, height in cells)
    type PaneSpec<'a> = (
        &'a str,
        &'a str,
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<i64>,
    );
    let panes: [PaneSpec; 3] = [
        ("board", "corral board", "zoomed", None, None, None),
        ("capture", "capture", "popup", Some("70%"), None, Some(8)),
        (
            "artifacts",
            "artifacts",
            "popup",
            Some("85%"),
            Some("85%"),
            None,
        ),
    ];
    for (id, title, placement, width, height_s, height_n) in panes {
        m.push_str(&format!(
            "[[panes]]\nid = {}\ntitle = {}\nplacement = {}\n",
            q(id),
            q(title),
            q(placement)
        ));
        if let Some(w) = width {
            m.push_str(&format!("width = {}\n", q(w)));
        }
        if let Some(h) = height_s {
            m.push_str(&format!("height = {}\n", q(h)));
        }
        if let Some(h) = height_n {
            m.push_str(&format!("height = {h}\n"));
        }
        m.push_str(&format!(
            "command = {}\n\n",
            arr(&[exe, "--root", root, id])
        ));
    }
    for id in ["board", "capture", "artifacts"] {
        m.push_str(&format!(
            "[[actions]]\nid = {}\ntitle = {}\ncommand = {}\n\n",
            q(id),
            q(&format!("corral {id}")),
            arr(&[
                herdr,
                "plugin",
                "pane",
                "open",
                "--plugin",
                "corral",
                "--entrypoint",
                id
            ])
        ));
    }
    m.push_str(&format!(
        "[[actions]]\nid = \"pm\"\ntitle = \"corral pm\"\ncommand = {}\n",
        arr(&[exe, "--root", root, "pm"])
    ));
    m
}

pub fn keybindings() -> String {
    let mut s = String::new();
    for (key, action, desc) in [
        ("prefix+k", "board", "corral board"),
        ("prefix+n", "capture", "corral capture"),
        ("prefix+a", "artifacts", "corral artifacts"),
        ("prefix+m", "pm", "corral pm"),
    ] {
        s.push_str(&format!(
            "[[keys.command]]\nkey = \"{key}\"\ntype = \"plugin_action\"\ncommand = \"corral.{action}\"\ndescription = \"{desc}\"\n\n"
        ));
    }
    s
}

fn write_manifest(ctx: &Ctx) -> Result<(PathBuf, bool)> {
    let exe = std::env::current_exe().context("locating the corral executable")?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let root: PathBuf = ctx
        .store
        .root()
        .canonicalize()
        .unwrap_or_else(|_| ctx.store.root().to_path_buf());
    let dir: PathBuf = ctx.store.state_dir().join("herdr-plugin");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let file = dir.join("herdr-plugin.toml");
    let text = manifest(
        &exe.to_string_lossy(),
        &root.to_string_lossy(),
        &find_herdr(),
    );
    let changed = config::read_config(&file)?.as_deref() != Some(&text);
    if changed {
        std::fs::write(&file, text).with_context(|| format!("writing {}", file.display()))?;
    }
    Ok((dir, changed))
}

/// Links the plugin and merges config, returning messages without writing stdout.
///
/// Errors include conflicting keybindings with exact manual replacement instructions.
pub fn setup(ctx: &Ctx) -> Result<Vec<String>> {
    let path = config_path()?;
    setup_at(ctx, &path)
}

fn setup_at(ctx: &Ctx, path: &Path) -> Result<Vec<String>> {
    // Check conflicts before any installation side effects.
    config::merged_text(&config::read_config(path)?.unwrap_or_default(), path)?;
    let (dir, changed) = write_manifest(ctx)?;
    let mut messages = vec![format!(
        "manifest: {}",
        dir.join("herdr-plugin.toml").display()
    )];
    if changed
        || !ctx
            .herdr
            .plugin_linked(&dir.join("herdr-plugin.toml"))
            .context("checking herdr plugin link")?
    {
        messages.push(format!("linked plugin: {}", link(ctx, &dir)?));
    } else {
        messages.push("plugin already linked".to_owned());
    }
    messages.extend(config::merge_at(path)?);
    Ok(messages)
}

pub fn install(ctx: &Ctx, print_only: bool) -> Result<()> {
    if print_only {
        let (dir, _) = write_manifest(ctx)?;
        println!("wrote {}", dir.join("herdr-plugin.toml").display());
        println!("\nAdd to ~/.config/herdr/config.toml:\n\n{}", keybindings());
    } else {
        for message in setup(ctx)? {
            println!("{message}");
        }
    }
    println!("Run `corral skill install` so the agents in your panes know the corral commands.");
    Ok(())
}

fn link(ctx: &Ctx, dir: &Path) -> Result<String> {
    ctx.herdr.plugin_link(dir).context("herdr plugin link")
}
