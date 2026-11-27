//! A new Ask/Chat shows the profile corrald resolved for it (model and effort); a chat that
//! exists keeps its own.
use corral_protocol::rows::SettingsSnapshot;
use serde_json::json;

use super::{Cx, Facts, SessionRole, Ui};
use crate::model::Id;
use crate::model::assistant::{QUICK, quick_id};
use crate::model::daemon::Feed;
use crate::model::health::daemon_env;
use crate::model::nav::Form;

fn settings(ask_chat_profile: Option<&str>) -> SettingsSnapshot {
    SettingsSnapshot {
        config: json!({
            "defaults": {"profile": "pi-sol"},
            "profiles": {
                "pi-sol": {"harness": "pi", "model": "openai/gpt-6.2-sol", "effort": "medium"},
                "pi-sol-high": {"harness": "pi", "model": "openai/gpt-6.2-sol", "effort": "high"}
            }
        })
        .as_object()
        .unwrap()
        .clone(),
        ask_chat_profile: ask_chat_profile.map(str::to_owned),
        ..Default::default()
    }
}

fn shown(feed: &Feed) -> (Option<String>, Option<String>) {
    let id = Id::from(QUICK);
    let facts = Facts {
        role: SessionRole::Assistant,
        task: None,
    };
    let ui = Ui::default();
    let cx = Cx::new(feed, &ui, &id, daemon_env(feed, Form::Desktop), &facts);
    (cx.model(), cx.effort())
}

#[test]
fn a_new_quick_thread_starts_on_the_resolved_profile_model_and_effort() {
    let mut feed = Feed::default();
    feed.settings = Some(settings(Some("pi-sol-high")));
    assert_eq!(
        shown(&feed),
        (Some("openai/gpt-6.2-sol".into()), Some("high".into()))
    );
}

#[test]
fn an_old_daemon_without_a_resolved_profile_keeps_the_legacy_fallback() {
    let mut feed = Feed::default();
    feed.settings = Some(settings(None));
    assert_eq!(
        shown(&feed),
        (Some("openai/gpt-6.2-sol".into()), Some("medium".into()))
    );
    // A resolved profile the configuration does not list as a chat profile is not trusted.
    feed.settings = Some(settings(Some("gone")));
    assert_eq!(shown(&feed).1.as_deref(), Some("medium"));
}

#[test]
fn an_existing_chat_keeps_its_own_effort_whatever_the_default_says() {
    let sample = crate::model::preview::feed(0, String::new(), String::new(), None);
    let id = Id::from(quick_id(&sample).expect("the sample has a quick chat"));
    let mut row = sample.assistants.get(&id).unwrap().clone();
    row.model = Some("openai/gpt-5".into());
    row.effort = Some("low".into());
    let mut feed = Feed::default();
    feed.assistants.insert(id.clone(), row);
    feed.settings = Some(settings(Some("pi-sol-high")));
    assert_eq!(
        shown(&feed),
        (Some("openai/gpt-5".into()), Some("low".into()))
    );
    // A chat row with no reported effort stays unknown rather than taking the default's.
    let row = feed.assistants.get_mut(&id).unwrap();
    row.effort = None;
    assert_eq!(shown(&feed).1, None);
    // After End the replacement uses the newly reported workspace default.
    feed.assistants.get_mut(&id).unwrap().state = corral_protocol::rows::AssistantState::Ended;
    assert_eq!(
        shown(&feed),
        (Some("openai/gpt-6.2-sol".into()), Some("high".into()))
    );
    // An old daemon may carry forward the ended profile; don't guess its replacement's route.
    feed.settings = Some(settings(None));
    assert_eq!(shown(&feed), (None, None));
}
