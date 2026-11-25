//! Non-secret hosted tool selectors. The provider identifies the billing/auth route.
use std::path::Path;

use anyhow::{Context, Result, ensure};
use toml_edit::{DocumentMut, TableLike};

use crate::pi_preferences;

const SECTION: &str = "hosted_tools";
const STALE: &str = "pi.toml changed; reload the hosted tools settings before saving";
const WEB_SEARCH: Field = Field {
    name: "web_search",
    providers: &["openai-codex", "openai", "anthropic", "anthropic-api"],
    error: "pi.toml hosted_tools.web_search must be off or a search provider/model selector",
};
const IMAGE_GEN: Field = Field {
    name: "image_gen",
    providers: &["openai", "openai-codex"],
    error: "pi.toml hosted_tools.image_gen must be off or an image provider/model selector",
};

#[derive(Debug, PartialEq, Eq)]
pub struct Preference {
    pub revision: String,
    pub web_search: Option<String>,
    pub image_gen: Option<String>,
}

struct Field {
    name: &'static str,
    providers: &'static [&'static str],
    error: &'static str,
}

impl Field {
    fn valid(&self, selector: &str) -> bool {
        // Keep in sync with the host-side selector grammar. Syntax alone does not
        // establish entitlement or model availability.
        selector == "off"
            || (selector.len() <= 256
                && selector.split_once('/').is_some_and(|(provider, id)| {
                    self.providers.contains(&provider)
                        && id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
                        && id
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
                        && !["sk-", "sess-", "eyj"].iter().any(|prefix| {
                            id.get(..prefix.len())
                                .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
                        })
                }))
    }

    fn read(&self, table: Option<&dyn TableLike>) -> Result<Option<String>> {
        table
            .and_then(|table| table.get(self.name))
            .map(|item| {
                item.as_str()
                    .filter(|selector| self.valid(selector))
                    .map(str::to_owned)
                    .context(self.error)
            })
            .transpose()
    }
}

fn hosted_table(doc: &DocumentMut) -> Result<Option<&dyn TableLike>> {
    doc.get(SECTION)
        .map(|item| {
            let table = item
                .as_table_like()
                .context("pi.toml hosted_tools must be a table")?;
            ensure!(
                table
                    .iter()
                    .all(|(key, _)| matches!(key, "web_search" | "image_gen")),
                "pi.toml hosted_tools contains an unknown field"
            );
            Ok(table)
        })
        .transpose()
}

pub fn read(root: &Path) -> Result<Preference> {
    let (revision, doc) = pi_preferences::read(root)?;
    let table = hosted_table(&doc)?;
    Ok(Preference {
        revision,
        web_search: WEB_SEARCH.read(table)?,
        image_gen: IMAGE_GEN.read(table)?,
    })
}

pub fn save(
    root: &Path,
    expected: &str,
    web_search: Option<String>,
    image_gen: Option<String>,
) -> Result<Preference> {
    let revision = pi_preferences::save(root, expected, STALE, |doc| {
        hosted_table(doc)?;
        for (field, value) in [(&WEB_SEARCH, &web_search), (&IMAGE_GEN, &image_gen)] {
            ensure!(
                value.as_deref().is_none_or(|value| field.valid(value)),
                "{}",
                field.error
            );
        }
        pi_preferences::set_string(doc, SECTION, WEB_SEARCH.name, web_search.as_deref())?;
        pi_preferences::set_string(doc, SECTION, IMAGE_GEN.name, image_gen.as_deref())
    })?;
    Ok(Preference {
        revision,
        web_search,
        image_gen,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{pi_preferences::FILE, pi_startup};
    use std::{fs, os::unix::fs::PermissionsExt, sync::Barrier};

    #[test]
    fn missing_and_existing_document_keep_null_off_and_other_settings_distinct() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let state = root.join(".corral-next");
        fs::create_dir(&state)?;
        let path = state.join(FILE);
        let missing = read(root)?;
        assert_eq!(missing.revision, "missing");
        assert_eq!(
            (missing.web_search.as_deref(), missing.image_gen.as_deref()),
            (None, None)
        );
        assert_eq!(save(root, &missing.revision, None, None)?, missing);
        assert!(!path.exists());
        let created = save(root, &missing.revision, Some("off".into()), None)?;
        assert_eq!(created, read(root)?);
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);

        let unrelated = "# private config\n[startup]\nmodel = 'openai/chat'\n[mcp]\nkey = 'secret-example' # preserve\n";
        fs::write(
            &path,
            format!("{unrelated}\n[hosted_tools]\nweb_search = 'off' # retain this note\n"),
        )?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640))?;
        let saved = save(
            root,
            &read(root)?.revision,
            Some("anthropic-api/search-a.1".into()),
            Some("openai/image".into()),
        )?;
        assert_eq!(saved, read(root)?);
        let text = fs::read_to_string(&path)?;
        assert!(text.starts_with(unrelated));
        assert!(text.contains("# retain this note"));
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o640);
        assert_eq!(save(root, &saved.revision, None, None)?.web_search, None);
        assert_eq!(read(root)?.image_gen, None);
        assert!(fs::read_to_string(&path)?.starts_with(unrelated));
        Ok(())
    }

    #[test]
    fn rejects_bad_selectors_and_malformed_documents_without_leaking_content() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let state = root.join(".corral-next");
        fs::create_dir(&state)?;
        let path = state.join(FILE);
        let revision = read(root)?.revision;
        for selector in [
            "",
            "openai/",
            "openai/one/two",
            "openai/model id",
            "openai/model\t",
            "openai/model\0",
            "openai/mödél",
            "openai/-model",
            "openai/sk-private",
            "openai/SESS-private",
            "openai/EyjPrivate",
            "openai/https://private.example",
            "https://private.example",
            "amazon-bedrock/model",
        ] {
            let error =
                save(root, &revision, Some(selector.into()), Some("off".into())).unwrap_err();
            assert_eq!(error.to_string(), WEB_SEARCH.error);
            assert!(!path.exists());
        }
        let limit = format!("openai/{}", "x".repeat(249));
        assert!(WEB_SEARCH.valid(&limit));
        assert!(!WEB_SEARCH.valid(&(limit + "x")));
        for provider in WEB_SEARCH.providers {
            assert!(WEB_SEARCH.valid(&format!("{provider}/a_B-1.2")));
        }
        assert_eq!(
            save(
                root,
                &revision,
                Some("openai-codex/search".into()),
                Some("anthropic/image".into())
            )
            .unwrap_err()
            .to_string(),
            IMAGE_GEN.error
        );
        assert!(!path.exists());
        for (text, error) in [
            ("secret-example = [", "pi.toml contains invalid TOML"),
            (
                "hosted_tools = 'secret-example'",
                "pi.toml hosted_tools must be a table",
            ),
            (
                "[hosted_tools]\nsecret-example = true",
                "pi.toml hosted_tools contains an unknown field",
            ),
            (
                "[hosted_tools]\nweb_search = 'openai/sk-secret-example'",
                WEB_SEARCH.error,
            ),
            ("[hosted_tools]\nimage_gen = 123", IMAGE_GEN.error),
        ] {
            fs::write(&path, text)?;
            assert_eq!(read(root).unwrap_err().to_string(), error);
            assert_eq!(fs::read_to_string(&path)?, text);
        }
        Ok(())
    }

    #[test]
    fn startup_and_hosted_writers_share_lock_and_whole_file_revision() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::create_dir(root.join(".corral-next"))?;
        let before = read(root)?.revision;
        assert_eq!(pi_startup::read(root)?.revision, before);
        let barrier = Barrier::new(2);
        std::thread::scope(|scope| -> Result<()> {
            let hosted = scope.spawn(|| {
                barrier.wait();
                save(root, &before, Some("off".into()), None)
            });
            let startup = scope.spawn(|| {
                barrier.wait();
                pi_startup::save(root, &before, Some("openai/chat".into()))
            });
            let hosted = hosted.join().unwrap();
            let startup = startup.join().unwrap();
            assert_ne!(hosted.is_ok(), startup.is_ok());
            let error = hosted.err().or_else(|| startup.err()).unwrap().to_string();
            assert!(
                error.contains("being saved") || error.contains("pi.toml changed"),
                "{error}"
            );
            Ok(())
        })?;
        let after = read(root)?.revision;
        assert_ne!(after, before);
        assert_eq!(pi_startup::read(root)?.revision, after);
        assert_eq!(
            save(root, &before, None, None).unwrap_err().to_string(),
            STALE
        );
        assert_eq!(
            pi_startup::save(root, &before, None)
                .unwrap_err()
                .to_string(),
            "pi.toml changed; reload the startup setting before saving"
        );
        assert_eq!(read(root)?.revision, after);
        Ok(())
    }
}
