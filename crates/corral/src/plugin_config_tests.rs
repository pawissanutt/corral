use super::*;

fn merge_at(path: &Path) -> Result<Vec<String>> {
    super::merge_at(path, &HerdrKeys::default())
}

fn present_at(path: &Path) -> Result<bool> {
    super::present_at(path, &HerdrKeys::default())
}

fn merged_text(text: &str, path: &Path) -> Result<Option<String>> {
    super::merged_text(text, path, &HerdrKeys::default())
}

#[test]
fn missing_config_is_created_with_all_bindings_without_backup() {
    // Given a nonexistent explicit config path.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("herdr/config.toml");
    assert!(!present_at(&path).unwrap());
    // When merging.
    let messages = merge_at(&path).unwrap();
    // Then all keys exist and the diff represents a new file.
    assert!(present_at(&path).unwrap());
    assert_eq!(messages.len(), 1);
    assert!(messages[0].contains("@@ -0,0"));
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
}

#[test]
fn existing_config_keeps_comments_user_keys_and_exact_backup() {
    // Given unrelated config, an unrelated user binding and no final newline.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let original = "# keep this\n[ui]\ntheme = 'dark'\n\n[[keys.command]]\nkey = 'prefix+x'\ntype = 'shell'\ncommand = 'echo hi'";
    std::fs::write(&path, original).unwrap();
    // When merging.
    let messages = merge_at(&path).unwrap();
    // Then the original is preserved byte-for-byte and backed up.
    let merged = std::fs::read_to_string(&path).unwrap();
    assert!(merged.starts_with(original));
    assert!(present_at(&path).unwrap());
    let backup = PathBuf::from(messages[0].strip_prefix("backup: ").unwrap());
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
    assert!(
        backup
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(".bak")
    );
    assert!(messages[1].contains("-theme = 'dark'"));
    assert!(messages[1].contains("+command = \"corral.board\""));
    assert!(messages[1].contains("\\ No newline at end of file"));
}

#[test]
fn repeat_merge_does_not_write_or_add_backups() {
    // Given a config that already has the required bindings.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let original = super::super::keybindings(&HerdrKeys::default())
        .replace("corral board", "My custom description");
    std::fs::write(&path, &original).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    // When merging.
    let messages = merge_at(&path).unwrap();
    // Then descriptions, file contents and modification time are untouched.
    assert!(present_at(&path).unwrap());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    assert_eq!(messages.len(), 1);
}

#[test]
fn partial_config_only_adds_missing_bindings() {
    // Given an existing board binding.
    let original =
        "[[keys.command]]\nkey = 'prefix+k'\ntype = 'plugin_action'\ncommand = 'corral.board'\n";
    // When merging.
    let merged = merged_text(original, Path::new("/temp/config.toml"))
        .unwrap()
        .unwrap();
    // Then exactly four bindings exist, without duplicating board.
    let doc = merged.parse::<DocumentMut>().unwrap();
    assert_eq!(entries(&doc).unwrap().len(), 4);
    assert!(merged.starts_with(original));
}

#[test]
fn conflicts_preserve_user_entries_and_add_safe_alternative() {
    // Given a conflicting key, including a duplicate that follows a correct key.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let original = format!(
        "{}\n[[keys.command]]\nkey = 'prefix+k'\ntype = 'shell'\ncommand = 'mine'\n",
        super::super::keybindings(&HerdrKeys::default())
    );
    std::fs::write(&path, &original).unwrap();
    // When merging.
    let messages = merge_at(&path).unwrap();
    // Then every original entry survives, the repair is explicit and repeat merges are idempotent.
    assert!(present_at(&path).unwrap());
    assert!(messages.iter().any(|m| m.contains("prefix+K")));
    let after = std::fs::read_to_string(&path).unwrap();
    assert!(after.starts_with(&original));
    let backup = messages
        .iter()
        .find_map(|m| m.strip_prefix("backup: "))
        .unwrap();
    assert_eq!(std::fs::read_to_string(backup).unwrap(), original);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
    merge_at(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), after);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
}

#[test]
fn malformed_config_is_not_modified_or_backed_up() {
    // Given malformed TOML and incompatible key structures.
    for original in ["not valid [", "keys = 4\n", "[keys]\ncommand = 'bad'\n"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        std::fs::write(&path, original).unwrap();
        // When merging.
        assert!(merge_at(&path).is_err());
        // Then the original is intact and no backup was made.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}

#[test]
fn inline_arrays_preserve_unrelated_content_and_are_idempotent() {
    // Given both supported inline shapes.
    for original in [
        "# hello\n[keys]\ncommand = [{key = 'prefix+x', type = 'shell', command = 'mine'}]\n[ui]\ntheme = 'dark'\n",
        "# hello\nkeys = { command = [] }\n[ui]\ntheme = 'dark'\n",
        "# hello\nkeys = {}\n[ui]\ntheme = 'dark'\n",
    ] {
        // When merging.
        let path = Path::new("/temp/config.toml");
        let merged = merged_text(original, path).unwrap().unwrap();
        // Then unrelated fields survive and the next merge has nothing to do.
        assert!(merged.contains("# hello"));
        assert!(merged.contains("theme = 'dark'"));
        assert!(merged_text(&merged, path).unwrap().is_none());
        let doc = merged.parse::<DocumentMut>().unwrap();
        assert!(entries(&doc).unwrap().len() >= 4);
    }
}

#[cfg(unix)]
#[test]
fn backup_retains_restricted_permissions() {
    use std::os::unix::fs::PermissionsExt;
    // Given a private config.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    std::fs::write(&path, "# private\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    // When merging.
    let messages = merge_at(&path).unwrap();
    // Then neither the config nor backup exposes its contents.
    let backup = PathBuf::from(messages[0].strip_prefix("backup: ").unwrap());
    for file in [&path, &backup] {
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[cfg(unix)]
#[test]
fn setup_links_manifest_and_returns_diff_using_only_explicit_temp_paths() {
    use std::os::unix::fs::PermissionsExt;
    // Given a fake CLI which only accepts plugin link and never touches herdr.
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("fake-herdr");
    std::fs::write(&bin, "#!/bin/sh\n[ \"$1\" = '--json' ] && shift\n[ \"$1\" = 'plugin' ] && [ \"$2\" = 'link' ] && [ -f \"$3/herdr-plugin.toml\" ] || exit 1\nprintf '%s\\n' '{\"result\":{\"linked\":true}}'\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let ctx = corral_ops::Ctx {
        store: corral_core::Store::init(temp.path()).unwrap(),
        herdr: corral_herdr::Herdr::with_bin(bin, None),
    };
    let path = temp.path().join("herdr/config.toml");
    // When running the same setup implementation with an explicit path.
    let messages = super::super::setup_at(&ctx, &path).unwrap();
    // Then linking succeeded, the manifest exists, and diff is returned to the caller.
    assert!(
        ctx.store
            .state_dir()
            .join("herdr-plugin/herdr-plugin.toml")
            .is_file()
    );
    assert!(present_at(&path).unwrap());
    assert!(
        messages
            .iter()
            .any(|message| message.contains("\"linked\":true"))
    );
    assert!(messages.iter().any(|message| message.starts_with("--- ")));
}

#[cfg(unix)]
#[test]
fn setup_does_not_relink_when_manifest_unchanged_and_plugin_enabled() {
    use std::os::unix::fs::PermissionsExt;
    // Given an unchanged manifest and enabled plugin; any link call would fail.
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("fake-herdr");
    let plugins = serde_json::json!({"result": {"plugins": [{
        "plugin_id": "corral", "enabled": true,
        "manifest_path": temp.path().join(".corral/state/herdr-plugin/herdr-plugin.toml"),
    }]}});
    std::fs::write(&bin, format!("#!/bin/sh\n[ \"$1\" = 'plugin' ] && [ \"$2\" = 'list' ] || exit 1\nprintf '%s\\n' '{plugins}'\n")).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let ctx = corral_ops::Ctx {
        store: corral_core::Store::init(temp.path()).unwrap(),
        herdr: corral_herdr::Herdr::with_bin(bin, None),
    };
    let (dir, _) = super::super::write_manifest(&ctx).unwrap();
    let manifest = dir.join("herdr-plugin.toml");
    let modified = std::fs::metadata(&manifest).unwrap().modified().unwrap();
    let path = temp.path().join("config.toml");
    std::fs::write(&path, super::super::keybindings(&HerdrKeys::default())).unwrap();
    // When running setup.
    let messages = super::super::setup_at(&ctx, &path).unwrap();
    // Then no link, config write, backup or manifest rewrite occurred.
    assert!(
        messages
            .iter()
            .any(|message| message == "plugin already linked")
    );
    assert!(!messages.iter().any(|message| message.starts_with("--- ")));
    assert_eq!(
        std::fs::metadata(&manifest).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 3);
}

#[test]
fn setup_checks_invalid_config_before_manifest_or_link_side_effects() {
    // Given an invalid config and a herdr path that must never be executed.
    let temp = tempfile::tempdir().unwrap();
    let ctx = corral_ops::Ctx {
        store: corral_core::Store::init(temp.path()).unwrap(),
        herdr: corral_herdr::Herdr::with_bin(temp.path().join("missing-herdr"), None),
    };
    let path = temp.path().join("config.toml");
    std::fs::write(&path, "keys = 4\n").unwrap();
    // When running setup.
    let error = super::super::setup_at(&ctx, &path).unwrap_err();
    // Then invalid configuration detection precedes every plugin side effect.
    assert!(format!("{error:#}").contains("keys must be a TOML table"));
    assert!(!ctx.store.state_dir().join("herdr-plugin").exists());
}

#[cfg(unix)]
#[test]
fn setup_relinks_when_plugin_points_at_another_workspace() {
    use std::os::unix::fs::PermissionsExt;
    // Given a manifest already written here but a plugin linked to another workspace.
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("herdr");
    std::fs::write(&bin, r#"#!/bin/sh
case "$1 $2" in
  "plugin list") printf '%s\n' '{"result":{"plugins":[{"plugin_id":"corral","enabled":true,"manifest_path":"/other/workspace/herdr-plugin.toml"}]}}' ;;
  "plugin link") printf '%s\n' '{"result":{"linked":true}}' ;;
  *) exit 1 ;;
esac
"#).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let ctx = corral_ops::Ctx {
        store: corral_core::Store::init(temp.path()).unwrap(),
        herdr: corral_herdr::Herdr::with_bin(bin, None),
    };
    super::super::write_manifest(&ctx).unwrap();
    let path = temp.path().join("config.toml");
    std::fs::write(&path, super::super::keybindings(&HerdrKeys::default())).unwrap();
    // When setup is run again for this workspace.
    let messages = super::super::setup_at(&ctx, &path).unwrap();
    // Then it relinks the current manifest, rather than treating another workspace's link as ready.
    assert!(messages.iter().any(|m| m.starts_with("linked plugin:")));
}

#[test]
fn conflict_alternatives_reserve_configured_and_native_keys() {
    // Given an inline user binding, a native key, and another corral action's preferred key.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let original =
        "[keys]\ndetach = 'prefix+M'\ncommand = [{ key = 'prefix+m', command = 'mine' }]\n";
    std::fs::write(&path, original).unwrap();
    let keys = HerdrKeys {
        board: "prefix+p".into(),
        ..HerdrKeys::default()
    };
    // When repairing the conflict.
    super::merge_at(&path, &keys).unwrap();
    // Then the PM alternative skips both occupied candidates and the other configured action.
    let doc = std::fs::read_to_string(&path)
        .unwrap()
        .parse::<DocumentMut>()
        .unwrap();
    let actual = entries(&doc).unwrap();
    let pm = actual
        .iter()
        .find(|entry| entry.get("command").and_then(Item::as_str) == Some("corral.pm"))
        .unwrap();
    assert_eq!(pm.get("key").and_then(Item::as_str), Some("prefix+A"));
    assert_eq!(doc["keys"]["detach"].as_str(), Some("prefix+M"));
    assert!(
        actual
            .iter()
            .any(|entry| entry.get("command").and_then(Item::as_str) == Some("mine"))
    );
    assert!(super::present_at(&path, &keys).unwrap());
}

#[test]
fn exhausted_alternatives_leave_user_config_untouched() {
    // Given every proposed alternative is occupied.
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let mut original = "[[keys.command]]\nkey = 'prefix+m'\ncommand = 'mine'\n\n[[keys.command]]\nkey = 'prefix+p'\ncommand = 'mine'\n".to_owned();
    for letter in 'A'..='Z' {
        original.push_str(&format!(
            "\n[[keys.command]]\nkey = 'prefix+{letter}'\ncommand = 'mine'\n"
        ));
    }
    std::fs::write(&path, &original).unwrap();
    // When no free key can be proposed.
    let error = merge_at(&path).unwrap_err();
    // Then repair refuses to write or create a backup.
    assert!(error.to_string().contains("herdr.keys.pm"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
}
