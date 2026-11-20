use corral_protocol::rows::{HarnessFact, PiChildProfile, PiConfigFacts, SettingsSnapshot};
use serde_json::json;

use super::*;
use crate::model::daemon::{Link, TopicState};
use crate::model::effect::Effect;
use crate::model::settings::State;
use crate::model::settings::editor::Edit;

fn loaded(text: &str) -> State {
    State {
        config: Editor {
            part: Part::Harnesses,
            saved: Some(text.into()),
            draft: text.into(),
            ..Editor::default()
        },
        ..State::default()
    }
}

#[test]
fn browsing_all_builtins_never_creates_an_override_in_the_draft_or_save() {
    let text = "# owner’s comment\n[limits]\nrunning = 2\n";
    let mut state = loaded(text);
    let items = entries(&[]);
    assert_eq!(
        items
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        ["pi", "claude", "omo", "omp", "generic"]
    );
    assert!(items.iter().all(|item| !item.configured));
    assert_eq!(selected(&items, None).unwrap().key, "pi");
    for item in &items {
        assert!(
            state
                .choose_entry(Part::Harnesses, item.key.clone())
                .is_empty()
        );
        assert_eq!(selected(&items, choice(&state.config)), Some(item));
    }
    assert!(!state.config.dirty());
    assert_eq!(state.config.draft, text);
    state.edit_config(Edit::Typed(
        Path::of(&["limits", "running"]),
        super::super::editor::Typed::Number,
        "4".into(),
    ));
    match state.save_config().as_slice() {
        [
            Effect::Config {
                put: Some(text), ..
            },
        ] => assert_eq!(text, "# owner’s comment\n[limits]\nrunning = 4\n"),
        other => panic!("expected one save; got {other:?}"),
    }
}

#[test]
fn custom_and_explicit_override_edit_one_draft_and_inherit_daemon_flags_when_created() {
    let text = "# workspace\n[limits]\nrunning = 2\n";
    let mut state = loaded(text);
    state.edit_config(Edit::AddEntry(Part::Harnesses, "omp".into()));
    assert!(
        !state.config.dirty(),
        "reserved names need the explicit override action"
    );
    state.edit_config(Edit::AddEntry(Part::Harnesses, " my-tool ".into()));
    let key = Path::entry("harnesses", "my-tool", "command");
    assert_eq!(state.config.added, Some(key.clone()));
    state.edit_config(Edit::Text(key, "my-tool {prompt_file}".into()));
    let feed = Feed { settings: Some(SettingsSnapshot {
        built_in_defaults: json!({ "harnesses": { "omp": { "command": "omp-new {model_args}", "model_flag": "-m", "effort_flag": "-e" } } }).as_object().cloned(),
        ..SettingsSnapshot::default()
    }), ..Feed::default() };
    let template = Template::of(&feed, "omp").unwrap();
    state.edit_config(Edit::OverrideHarness("omp".into(), template));
    let doc = document::parse(&state.config.draft).unwrap();
    let items = entries(&document::names(&doc, "harnesses"));
    assert_eq!(items.len(), 6);
    assert_eq!(items.last().unwrap().key, "my-tool");
    assert!(selected(&items, choice(&state.config)).unwrap().configured);
    for (field, expected) in [
        ("command", "omp-new {model_args}"),
        ("model_flag", "-m"),
        ("effort_flag", "-e"),
    ] {
        assert_eq!(
            document::get(&doc, &Path::entry("harnesses", "omp", field))
                .and_then(toml_edit::Item::as_str),
            Some(expected)
        );
    }
    state.edit_config(Edit::RemoveEntry(Part::Harnesses, "omp".into()));
    let doc = document::parse(&state.config.draft).unwrap();
    let items = entries(&document::names(&doc, "harnesses"));
    let selected = selected(&items, choice(&state.config)).unwrap();
    assert_eq!(selected.key, "omp");
    assert!(!selected.configured);
    assert!(state.config.draft.contains("[harnesses.my-tool]"));
    assert_eq!(state.config.saved.as_deref(), Some(text));
    state.discard_config();
    assert_eq!(state.config.draft, text);
}

#[test]
fn usage_counts_effective_profile_replacements_and_not_child_profiles() {
    let state = loaded(
        r#"
[profiles.move-me]
harness = "claude"
[profiles.demo]
harness = "pi"
model = "example/model"
effort = "high"
[profiles.incomplete]
model = "test"
"#,
    );
    let feed = Feed { settings: Some(SettingsSnapshot {
        built_in_defaults: json!({ "profiles": {
            "move-me": { "harness": "pi" }, "keep-me": { "harness": "pi" }, "incomplete": { "harness": "pi" }
        } }).as_object().cloned(),
        pi_config: Some(PiConfigFacts { path: "/workspace/.corral-next/pi.toml".into(), status: PiConfigStatus::Read, child_profiles: vec![PiChildProfile { name: "child".into(), model: "test/child".into(), effort: None }] }),
        ..SettingsSnapshot::default()
    }), link: Link::Up, ..Feed::default() };
    let details = Details::read(&state.config, &feed);
    let pi = &details.rows["pi"].profiles;
    assert_eq!(
        pi.iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["demo", "keep-me"]
    );
    assert_eq!(pi[0].model.as_deref(), Some("example/model"));
    assert!(pi[0].editable);
    assert!(!pi[1].editable);
    assert_eq!(details.rows["claude"].profiles[0].name, "move-me");
    assert_eq!(details.children.profiles[0].name, "child");
}

#[test]
fn detection_never_labels_an_unsaved_command_or_retained_snapshot_as_detected() {
    let mut state = loaded("");
    let template = Template::of(&Feed::default(), "generic").unwrap();
    let mut feed = Feed {
        settings: Some(SettingsSnapshot {
            config: json!({ "harnesses": { "generic": { "command": template.command } } })
                .as_object()
                .cloned()
                .unwrap(),
            ..SettingsSnapshot::default()
        }),
        link: Link::Up,
        ..Feed::default()
    };
    fn status(state: &State, feed: &Feed) -> Detection {
        Details::read(&state.config, feed).rows["generic"]
            .discovery
            .status
    }
    assert_eq!(status(&state, &feed), Detection::NotChecked);
    feed.settings.as_mut().unwrap().harness_facts = Some(vec![HarnessFact {
        name: "generic".into(),
        status: HarnessStatus::Installed,
        executable: Some("cat".into()),
        resolved_path: Some("/bin/cat".into()),
    }]);
    assert_eq!(status(&state, &feed), Detection::Detected);
    state.edit_config(Edit::OverrideHarness("generic".into(), template));
    assert_eq!(status(&state, &feed), Detection::Detected);
    state.edit_config(Edit::Text(
        Path::entry("harnesses", "generic", "command"),
        "new-cli {prompt_file}".into(),
    ));
    assert_eq!(status(&state, &feed), Detection::Draft);
    state.discard_config();
    feed.topics.insert(
        "settings".into(),
        TopicState {
            stale: true,
            ..TopicState::default()
        },
    );
    assert_eq!(status(&state, &feed), Detection::Stale);
}
