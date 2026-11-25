//! The Pi startup model preference, never the credential-bearing document.
use std::path::Path;

use anyhow::{Context, Result, ensure};
use toml_edit::{DocumentMut, Item};

use crate::pi_preferences;

const STALE: &str = "pi.toml changed; reload the startup setting before saving";

#[derive(Debug, PartialEq, Eq)]
pub struct Preference {
    pub revision: String,
    pub model: Option<String>,
}

fn valid_model(model: &str) -> bool {
    model.len() <= 256
        && !model.chars().any(|c| c.is_whitespace() || c.is_control())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi_preferences::FILE;
    use std::{fs, os::unix::fs::symlink};

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
