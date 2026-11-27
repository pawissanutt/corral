//! Pi startup preferences, never the credential-bearing document.
use std::path::Path;

use anyhow::{Context, Result, ensure};
use toml_edit::{DocumentMut, Item};

use crate::pi_preferences;

const STALE: &str = "pi.toml changed; reload the startup setting before saving";
// Leave room for both values to be JSON-escaped inside the app control frame.
const MAX_APP_STARTUP_VALUE_BYTES: usize = 4096;
const INVALID_ASK: &str =
    "pi.toml startup.ask_profile must be a profile name of at most 4096 bytes";

#[derive(Debug, PartialEq, Eq)]
pub struct Preference {
    pub revision: String,
    pub model: Option<String>,
}

fn valid_model(model: &str) -> bool {
    model.len() <= 256 && qualified_model(model)
}

fn qualified_model(model: &str) -> bool {
    !model.chars().any(|c| c.is_whitespace() || c.is_control())
        && model.split_once('/').is_some_and(|(provider, id)| {
            // Keep in sync with the launch routes in corral-pi/src/auth/providers.ts.
            !id.is_empty()
                && matches!(
                    provider,
                    "openai-codex"
                        | "openai"
                        | "anthropic"
                        | "anthropic-api"
                        | "amazon-bedrock"
                        | "llama.cpp"
                )
        })
}

fn check_table(doc: &DocumentMut) -> Result<()> {
    ensure!(
        doc.get("startup").is_none_or(|item| item.is_table_like()),
        "pi.toml startup must be a table"
    );
    Ok(())
}

pub fn read(root: &Path) -> Result<Preference> {
    let (revision, doc) = pi_preferences::read(root)?;
    check_table(&doc)?;
    let model = doc
        .get("startup")
        .and_then(Item::as_table_like)
        .and_then(|table| table.get("model"))
        .map(|item| {
            item.as_str()
                .filter(|model| valid_model(model))
                .map(str::to_owned)
                .context("pi.toml startup.model is invalid; correct it on the workspace host")
        })
        .transpose()?;
    Ok(Preference { revision, model })
}

pub fn save(root: &Path, expected: &str, model: Option<String>) -> Result<Preference> {
    let revision = pi_preferences::save(root, expected, STALE, |doc| {
        check_table(doc)?;
        ensure!(
            model.as_deref().is_none_or(valid_model),
            "Enter a known Pi provider followed by / and a model ID"
        );
        pi_preferences::set_string(doc, "startup", "model", model.as_deref())
    })?;
    Ok(Preference { revision, model })
}

/// App-managed startup preferences. Terminal Chat, Build and Code keep `startup.model`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AppStartup {
    pub revision: String,
    pub ask_profile: Option<String>,
    pub build_model: Option<String>,
    pub build_inherited: bool,
    pub ask_problem: Option<&'static str>,
    pub build_problem: Option<&'static str>,
}

pub enum AppStartupChange {
    AskProfile(Option<String>),
    BuildModel(Option<String>),
}

fn startup_item<'a>(doc: &'a DocumentMut, key: &str) -> Option<&'a Item> {
    doc.get("startup")?.as_table_like()?.get(key)
}

fn valid_ask_profile_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_APP_STARTUP_VALUE_BYTES
}

pub fn check_ask_profile_name(name: &str) -> Result<()> {
    ensure!(valid_ask_profile_name(name), INVALID_ASK);
    Ok(())
}

fn ask_profile_value(doc: &DocumentMut) -> std::result::Result<Option<String>, &'static str> {
    startup_item(doc, "ask_profile")
        .map(|item| {
            item.as_str()
                .filter(|name| valid_ask_profile_name(name))
                .map(str::to_owned)
                .ok_or(INVALID_ASK)
        })
        .transpose()
}

/// Read just the Ask/Chat selection; a Build model failure must not affect chat.
pub fn read_ask_profile(root: &Path) -> Result<Option<String>> {
    let (_, doc) = pi_preferences::read(root)?;
    check_table(&doc)?;
    ask_profile_value(&doc).map_err(anyhow::Error::msg)
}

fn qualified_build_model(model: &str) -> bool {
    // Build cannot use local inference; Bedrock needs explicit region and profile bindings.
    // JavaScript's qualified-model grammar also treats a byte-order mark as whitespace.
    qualified_model(model)
        && !model.contains('\u{feff}')
        && !model.starts_with("llama.cpp/")
        && !model.starts_with("amazon-bedrock/")
}

fn valid_build_model(model: &str) -> bool {
    model.len() <= 256 && qualified_build_model(model)
}

fn app_values(doc: &DocumentMut) -> AppStartup {
    let build_override = startup_item(doc, "build_model");
    let build = if let Some(item) = build_override {
        item.as_str()
            .filter(|model| model.is_empty() || valid_build_model(model))
            .map(|model| (!model.is_empty()).then(|| model.to_owned()))
            .ok_or("pi.toml startup.build_model must be a known provider/model or empty")
    } else {
        startup_item(doc, "model")
            .map(|item| {
                item.as_str()
                    .filter(|model| model.len() <= MAX_APP_STARTUP_VALUE_BYTES && qualified_build_model(model))
                    .map(str::to_owned)
                    .ok_or("Build inherits an unsupported or oversized pi.toml startup.model; save a Build default separately")
            })
            .transpose()
    };
    let mut values = AppStartup {
        build_inherited: build_override.is_none(),
        ..Default::default()
    };
    match ask_profile_value(doc) {
        Ok(profile) => values.ask_profile = profile,
        Err(problem) => values.ask_problem = Some(problem),
    }
    match build {
        Ok(model) => values.build_model = model,
        Err(problem) => values.build_problem = Some(problem),
    }
    values
}

pub fn read_app(root: &Path) -> Result<AppStartup> {
    let (revision, doc) = pi_preferences::read(root)?;
    check_table(&doc)?;
    Ok(AppStartup {
        revision,
        ..app_values(&doc)
    })
}

pub fn save_app(root: &Path, expected: &str, change: AppStartupChange) -> Result<AppStartup> {
    let mut saved = AppStartup::default();
    let revision = pi_preferences::save(root, expected, STALE, |doc| {
        check_table(doc)?;
        match &change {
            AppStartupChange::AskProfile(name) => {
                if let Some(name) = name {
                    check_ask_profile_name(name)?;
                }
                pi_preferences::set_string(doc, "startup", "ask_profile", name.as_deref())?;
            }
            AppStartupChange::BuildModel(model) => {
                ensure!(
                    model.as_deref().is_none_or(valid_build_model),
                    "Enter a supported Build provider and model ID (openai-codex, openai, anthropic or anthropic-api)"
                );
                // Absence inherits the terminal default; empty explicitly clears it for Build.
                pi_preferences::set_string(
                    doc,
                    "startup",
                    "build_model",
                    Some(model.as_deref().unwrap_or_default()),
                )?;
            }
        }
        saved = app_values(doc);
        Ok(())
    })?;
    Ok(AppStartup { revision, ..saved })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_preferences::FILE;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn malformed_app_startup_values_do_not_block_reading_or_saving_the_other_preference()
    -> Result<()> {
        let temp = tempfile::tempdir()?;
        let state = temp.path().join(".corral-next");
        fs::create_dir(&state)?;
        let path = state.join(FILE);
        let root = temp.path();
        fs::write(&path, "[startup]\nmodel = '' # keep\n")?;
        let before = read_app(root)?;
        assert!(before.build_problem.is_some());
        let ask = save_app(
            root,
            &before.revision,
            AppStartupChange::AskProfile(Some("chat".into())),
        )?;
        assert_eq!(ask.ask_profile.as_deref(), Some("chat"));
        assert!(ask.build_inherited && ask.build_problem.is_some());
        assert!(fs::read_to_string(&path)?.contains("model = '' # keep"));

        let oversized = "x".repeat(MAX_APP_STARTUP_VALUE_BYTES + 1);
        fs::write(
            &path,
            format!("[startup]\nmodel = 'openai/{oversized}'\nask_profile = '{oversized}'\n"),
        )?;
        let readback = read_app(root)?;
        assert!(readback.ask_problem.is_some() && readback.build_problem.is_some());
        assert!(readback.ask_profile.is_none() && readback.build_model.is_none());

        fs::write(&path, "[startup]\nask_profile = 123\nbuild_model = false\n")?;
        let before = read_app(root)?;
        assert!(before.ask_problem.is_some() && before.build_problem.is_some());
        for selector in ["llama.cpp/local", "amazon-bedrock/unbound"] {
            assert!(
                save_app(
                    root,
                    &before.revision,
                    AppStartupChange::BuildModel(Some(selector.into()))
                )
                .is_err()
            );
        }
        let build = save_app(root, &before.revision, AppStartupChange::BuildModel(None))?;
        assert!(build.build_problem.is_none() && build.ask_problem.is_some());
        assert!(fs::read_to_string(&path)?.contains("ask_profile = 123"));
        let cleared = save_app(root, &build.revision, AppStartupChange::AskProfile(None))?;
        assert!(cleared.ask_problem.is_none() && cleared.ask_profile.is_none());
        assert_eq!(cleared, read_app(root)?);
        Ok(())
    }

    #[test]
    fn concurrent_saves_do_not_lose_a_write() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir(temp.path().join(".corral-next"))?;
        let revision = read(temp.path())?.revision;
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let handles: Vec<_> = ["openai/first", "openai/second"]
                .into_iter()
                .map(|model| {
                    let barrier = &barrier;
                    let revision = &revision;
                    let root = temp.path();
                    scope.spawn(move || {
                        barrier.wait();
                        save(root, revision, Some(model.to_owned()))
                    })
                })
                .collect();
            let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
            let error = results
                .iter()
                .find_map(|r| r.as_ref().err())
                .unwrap()
                .to_string();
            assert!(
                error.contains("being saved") || error.contains(STALE),
                "{error}"
            );
            let winner = results.into_iter().find_map(Result::ok).unwrap();
            assert_eq!(read(temp.path()).unwrap(), winner);
        });
        Ok(())
    }

    #[test]
    fn startup_edits_only_the_model_and_refuses_stale_or_unsafe_writes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let state = temp.path().join(".corral-next");
        fs::create_dir(&state)?;
        let path = state.join(FILE);
        let read = || read(temp.path());
        let write = |revision: String, model: Option<&str>| {
            save(temp.path(), &revision, model.map(str::to_owned))
        };
        let missing = read()?;
        assert_eq!(missing.model, None);
        let created = write(missing.revision, Some("openai-codex/gpt-6.2-sol"))?;
        assert_eq!(created, read()?);
        fs::write(
            &path,
            "[mcp]\nother = 'keep'\n[startup]\nmodel = 'openai-codex/example' # note\nfuture = true\n",
        )?;
        assert!(write(created.revision, None).is_err());
        let current = read()?;
        assert!(write(current.revision.clone(), Some("unknown/model")).is_err());
        let saved = write(current.revision, Some("llama.cpp/local"))?;
        assert_eq!(saved, read()?);
        let text = fs::read_to_string(&path)?;
        assert!(text.contains("other = 'keep'") && text.contains("# note"));
        assert_eq!(write(saved.revision, None)?.model, None);
        assert!(fs::read_to_string(&path)?.contains("future = true"));
        fs::remove_file(&path)?;
        symlink("missing-target", &path)?;
        assert!(read().is_err());
        fs::remove_file(&path)?;
        let moved = temp.path().join("elsewhere");
        fs::rename(&state, &moved)?;
        symlink(&moved, &state)?;
        assert!(read().is_err());
        assert!(write("missing".into(), Some("openai/model")).is_err());
        assert!(!moved.join(FILE).exists());
        Ok(())
    }
}
