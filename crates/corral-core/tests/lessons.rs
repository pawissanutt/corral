use corral_core::*;
use jiff::Timestamp;
use tempfile::tempdir;

fn at(days: i64) -> Timestamp {
    Timestamp::from_second(days * 86400).unwrap()
}

fn lesson(id: &str) -> Lesson {
    Lesson {
        id: id.into(),
        rule: "Keep `state` atomic.\n\n```rust\nwrite()?;\n```\n".into(),
        why: "\nAvoid partial updates.\n".into(),
        evidence: vec![LessonEvidence {
            task: "T-001".into(),
            ts: at(1),
            source: "review".into(),
        }],
        scope: LessonScope::default(),
        confidence: 0.8,
        pinned: false,
        contradicted: false,
        last_seen: at(1),
    }
}

#[test]
fn storage_round_trips_and_rebuilds_scopes_from_owner_edits() {
    // Given a saved lesson and a hand-edited rule and scope.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let original = lesson("atomic");
    store
        .save_lessons("app", std::slice::from_ref(&original))
        .unwrap();
    assert_eq!(
        store.lessons("app").unwrap(),
        std::slice::from_ref(&original)
    );
    let path = store.dir().join("lessons/app.md");
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Keep `state` atomic.", "Keep `disk` atomic.")
        .replacen("files: []", "files:\n      - src/**", 1);
    std::fs::write(&path, &text).unwrap();
    std::fs::write(store.dir().join("lessons/app.index.json"), "{}").unwrap();

    // When the index is rebuilt.
    store.reindex_lessons("app").unwrap();

    // Then owner text is authoritative and unchanged by reindexing.
    let mut expected = original;
    expected.rule = expected.rule.replace("`state`", "`disk`");
    expected.scope.files = vec!["src/**".into()];
    assert_eq!(store.lessons("app").unwrap(), [expected]);
    assert_eq!(std::fs::read_to_string(path).unwrap(), text);
    let index: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(store.dir().join("lessons/app.index.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(index["version"], 1);
    assert_eq!(index["scopes"]["atomic"]["files"][0], "src/**");
}

#[test]
fn mutations_pin_delete_and_record_events() {
    // Given two stored lessons.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    store
        .save_lessons("app", &[lesson("a"), lesson("b")])
        .unwrap();

    // When one is pinned and the other deleted.
    store.pin_lesson("app", "a", true).unwrap();
    store.delete_lesson("app", "b").unwrap();

    // Then only the pinned lesson remains and mutations have events.
    let mut expected = lesson("a");
    expected.pinned = true;
    assert_eq!(store.lessons("app").unwrap(), [expected]);
    let events = store.events().unwrap();
    assert_eq!(
        events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        ["lessons_saved", "lesson_pinned", "lesson_deleted"]
    );
    assert!(store.pin_lesson("app", "missing", false).is_err());
    assert!(store.delete_lesson("app", "missing").is_err());
}

#[test]
fn stale_snapshots_are_rejected_after_pin_delete_or_owner_edit() {
    // Given each kind of change after a request snapshot.
    for change in ["pin", "delete", "text"] {
        let dir = tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        let snapshot = vec![lesson("a")];
        store.save_lessons("app", &snapshot).unwrap();
        match change {
            "pin" => store.pin_lesson("app", "a", true).unwrap(),
            "delete" => store.delete_lesson("app", "a").unwrap(),
            "text" => {
                let path = store.dir().join("lessons/app.md");
                let text = std::fs::read_to_string(&path)
                    .unwrap()
                    .replace("atomic.", "consistent.");
                std::fs::write(path, text).unwrap();
            }
            _ => unreachable!(),
        }
        let changed = store.lessons("app").unwrap();
        let events = store.events().unwrap().len();

        // When a response tries to save with that stale snapshot.
        let result = store.save_lessons_if("app", &snapshot, &[lesson("b")]);

        // Then it neither overwrites changes nor emits a saved event.
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("changed since the snapshot")
        );
        assert_eq!(store.lessons("app").unwrap(), changed);
        assert_eq!(store.events().unwrap().len(), events);
    }
}

#[test]
fn matching_snapshot_allows_compare_and_swap_save() {
    // Given an empty snapshot for a project with no file yet.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    assert!(store.lessons("app").unwrap().is_empty());

    // When the expected snapshot still matches.
    store.save_lessons_if("app", &[], &[lesson("a")]).unwrap();

    // Then it saves and emits the usual event.
    assert_eq!(store.lessons("app").unwrap(), [lesson("a")]);
    assert_eq!(
        store.events().unwrap().last().unwrap().kind,
        "lessons_saved"
    );
}

#[test]
fn concurrent_pins_preserve_both_mutations() {
    // Given two callers synchronized before mutation.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    store
        .save_lessons("app", &[lesson("a"), lesson("b")])
        .unwrap();
    let barrier = std::sync::Barrier::new(2);

    // When they pin different lessons concurrently.
    std::thread::scope(|scope| {
        for id in ["a", "b"] {
            let store = &store;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                store.pin_lesson("app", id, true).unwrap();
            });
        }
    });

    // Then neither read-modify-write loses the other.
    assert!(store.lessons("app").unwrap().iter().all(|l| l.pinned));
}

#[test]
fn invalid_data_is_rejected_at_both_storage_boundaries() {
    // Given invalid confidence, globs and duplicate IDs.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let mut invalid = Vec::new();
    for confidence in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        let mut value = lesson("a");
        value.confidence = confidence;
        invalid.push(vec![value]);
    }
    let mut value = lesson("a");
    value.scope.files = vec!["[abc]".into()];
    invalid.push(vec![value]);
    invalid.push(vec![lesson("a"), lesson("a")]);
    for values in invalid {
        // When saving invalid input, then no authoritative file is created.
        assert!(store.save_lessons("app", &values).is_err());
        assert!(store.lessons("app").unwrap().is_empty());
    }
    store.save_lessons("app", &[lesson("a")]).unwrap();
    let path = store.dir().join("lessons/app.md");
    let good = std::fs::read_to_string(&path).unwrap();
    for edited in [
        good.replace("confidence: 0.8", "confidence: .nan"),
        good.replace("files: []", "files: ['../escape']"),
        good.replace("version: 1", "version: 2"),
        good.replace("<!-- lesson a why -->", "<!-- lesson a nope -->"),
    ] {
        // When an owner makes invalid edits, then reading fails closed.
        std::fs::write(&path, edited).unwrap();
        assert!(store.lessons("app").is_err());
    }
}

#[test]
fn unsafe_project_keys_are_rejected() {
    // Given paths that could escape or collapse the lessons namespace.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    for key in ["", ".", "..", "../escape", "/absolute", "a/b", "a\\b", "a."] {
        // When reading or writing, then neither boundary permits the path.
        assert!(store.lessons(key).is_err(), "{key:?}");
        assert!(store.save_lessons(key, &[lesson("a")]).is_err(), "{key:?}");
    }
}

#[cfg(unix)]
#[test]
fn symlink_storage_paths_are_rejected_without_touching_target() {
    // Given an external file linked as the authoritative lessons document.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    std::fs::create_dir(store.dir().join("lessons")).unwrap();
    let outside = dir.path().join("outside");
    std::fs::write(&outside, "untouched").unwrap();
    std::os::unix::fs::symlink(&outside, store.dir().join("lessons/app.md")).unwrap();

    // When storage reads or writes the linked path.
    assert!(store.lessons("app").is_err());
    assert!(store.save_lessons("app", &[lesson("a")]).is_err());

    // Then the external target is unchanged.
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "untouched");
}

#[test]
fn scopes_require_every_dimension_and_allow_alternatives() {
    // Given a fully scoped lesson and a task referencing its file.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let mut task = store
        .create(
            "Fix ATOMIC writes",
            None,
            Status::Backlog,
            "Edit `src/cache.rs`.",
        )
        .unwrap();
    task.kind = Some("bug".into());
    let mut value = lesson("a");
    value.scope = LessonScope {
        files: vec!["src/**/*.rs".into()],
        kinds: vec!["feature".into(), "bug".into()],
        profiles: vec!["careful".into()],
        harnesses: vec!["omo".into()],
        tags: vec!["atomic".into()],
    };

    // When matching all dimensions, then the lesson is included.
    let matching = |task: &Task, profile, harness, files: &[String]| {
        match_lessons(
            std::slice::from_ref(&value),
            task,
            profile,
            harness,
            files,
            5,
            at(1),
        )
    };
    assert_eq!(
        matching(&task, Some("careful"), Some("omo"), &[]),
        [value.clone()]
    );
    // Removing any required dimension excludes it.
    assert!(matching(&task, None, Some("omo"), &[]).is_empty());
    assert!(matching(&task, Some("careful"), None, &[]).is_empty());
    task.kind = None;
    assert!(matching(&task, Some("careful"), Some("omo"), &[]).is_empty());
    task.kind = Some("bug".into());
    task.title = "Fix atomically written files".into();
    assert!(matching(&task, Some("careful"), Some("omo"), &[]).is_empty());
}

#[test]
fn files_match_likely_paths_and_overlapping_write_globs() {
    // Given a scoped lesson without text path references.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let mut task = store.create("Fix", None, Status::Backlog, "").unwrap();
    let mut value = lesson("a");
    value.scope.files = vec!["src/**/cache*.rs".into()];
    let values = [value];
    let matching =
        |task: &Task, files: &[String]| match_lessons(&values, task, None, None, files, 5, at(1));

    // When candidates or write globs intersect the lesson, then it matches.
    assert_eq!(matching(&task, &["src/cache.rs".into()]).len(), 1);
    assert_eq!(
        matching(&task, &["src/nested/cache_test.rs".into()]).len(),
        1
    );
    for glob in ["src/**", "src/*test.rs", "src/nested/**/cache.rs"] {
        task.write_scope = vec![glob.into()];
        assert_eq!(matching(&task, &[]).len(), 1, "{glob}");
    }
    task.write_scope = vec!["docs/**".into()];
    assert!(matching(&task, &[]).is_empty());
}

#[test]
fn ranking_is_pinned_then_decayed_confidence_then_id_and_limited() {
    // Given old/new tied lessons, an old pin, and a contradicted pin.
    let dir = tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store.create("Fix", None, Status::Backlog, "").unwrap();
    let mut old = lesson("old");
    old.confidence = 1.0;
    let mut a = lesson("a");
    a.confidence = 0.6;
    a.last_seen = at(91);
    let mut z = a.clone();
    z.id = "z".into();
    let mut pin = lesson("pin");
    pin.pinned = true;
    pin.confidence = 0.01;
    let mut contradicted = pin.clone();
    contradicted.id = "contradicted".into();
    contradicted.contradicted = true;
    let values = [old, z, contradicted, a, pin];

    // When ranked at exactly one half-life after the old lesson.
    let matched = match_lessons(&values, &task, None, None, &[], 4, at(91));

    // Then pins win, age decays confidence, ties use ID, contradictions disappear.
    assert_eq!(
        matched.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(),
        ["pin", "a", "z", "old"]
    );
    assert_eq!(
        match_lessons(&values, &task, None, None, &[], 2, at(91)).len(),
        2
    );
    assert!(match_lessons(&values, &task, None, None, &[], 0, at(91)).is_empty());
    assert!((matched[3].confidence - 1.0).abs() < f64::EPSILON);
}

#[test]
fn config_defaults_are_opt_in_and_partial_tables_round_trip() {
    // Given an existing config without [lessons].
    let dir = tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    let mut raw: toml::Value = toml::from_str(&config.to_toml()).unwrap();
    raw.as_table_mut().unwrap().remove("lessons");

    // When deserializing an old config or a partial lessons table.
    let loaded: Config = raw.try_into().unwrap();
    let partial: LessonsCfg = toml::from_str("after_landings = 3\n").unwrap();

    // Then scheduling and profile stay unset, top defaults to five, and values persist.
    assert_eq!(loaded.lessons, LessonsCfg::default());
    assert_eq!(partial.every, None);
    assert_eq!(partial.profile, None);
    assert_eq!(partial.after_failures, None);
    assert_eq!(partial.top, 5);
    config.lessons = partial;
    let round_trip: Config = toml::from_str(&config.to_toml()).unwrap();
    assert_eq!(round_trip, config);
    config.validate(dir.path()).unwrap();
}

#[test]
fn config_reports_invalid_lessons_settings_together() {
    // Given invalid schedule, thresholds, profile and result limit.
    let dir = tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    config.lessons = LessonsCfg {
        every: Some("0d".into()),
        after_landings: Some(0),
        after_failures: Some(0),
        profile: Some("missing".into()),
        top: 21,
    };

    // When validating.
    let errors = config.validate(dir.path()).unwrap_err().0;

    // Then every field is reported, and boundary-valid settings are accepted.
    for field in [
        "every",
        "after_landings",
        "after_failures",
        "profile",
        "top",
    ] {
        assert!(
            errors
                .iter()
                .any(|error| error.contains(&format!("lessons.{field}")))
        );
    }
    config.lessons = LessonsCfg {
        every: Some("2w".into()),
        after_landings: Some(1),
        after_failures: Some(1),
        profile: Some("omo".into()),
        top: 20,
    };
    config.validate(dir.path()).unwrap();
    config.lessons.top = 0;
    assert!(config.validate(dir.path()).is_err());
}

#[test]
fn omitted_scope_and_flags_deserialize_to_defaults() {
    // Given a minimal externally serialized lesson.
    let text =
        r#"{"id":"a","rule":"r","why":"w","confidence":0.5,"last_seen":"2026-10-15T00:00:00Z"}"#;

    // When deserializing.
    let value: Lesson = serde_json::from_str(text).unwrap();

    // Then scopes, evidence and flags are unrestricted/empty/false.
    assert_eq!(value.scope, LessonScope::default());
    assert!(value.evidence.is_empty());
    assert!(!value.pinned);
    assert!(!value.contradicted);
}
