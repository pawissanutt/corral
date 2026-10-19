use corral_core::{OperatingMode, Status, Store};
use ratatui::crossterm::event::KeyCode;

use super::SettingsView;
use super::model::Field;
use super::operating::Key;
use super::tests::{c, fixture, k, render_at};
use crate::{Action, View};

/// Exercise the real owner boundary in a child; never mutate the parallel test process's environment.
fn run_as_owner(name: &str) -> bool {
    const CHILD: &str = "CORRAL_OPERATING_OWNER_TEST";
    if std::env::var(CHILD).as_deref() == Ok(name) {
        return false;
    }
    let home = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("settings::operating_tests::{name}"),
            "--nocapture",
        ])
        .env(CHILD, name)
        .env_remove("AI_AGENT")
        .env_remove("PI_CODING_AGENT")
        .env_remove("CORRAL_PM")
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_STATE_HOME", home.path().join("state"))
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

fn selected() -> (super::tests::Fixture, SettingsView) {
    let fx = fixture();
    let task = fx
        .ctx
        .store
        .create("Finite work", Some("app"), Status::Backlog, "Agreed scope")
        .unwrap();
    let mut view = SettingsView::new(&fx.ctx);
    view.on_key(c('6'), &fx.ctx);
    view.set(&fx.ctx, &Field::Operating(Key::Tasks), &task.id)
        .unwrap();
    view.set(&fx.ctx, &Field::Operating(Key::Dispatch), "on")
        .unwrap();
    (fx, view)
}

#[test]
fn operating_selection_is_not_granted_by_ordinary_settings_save() {
    // Given an explicit finite selection including dispatch.
    let (fx, mut view) = selected();
    // When ordinary settings are saved.
    view.on_key(c('s'), &fx.ctx);
    // Then selection alone is not consent.
    assert!(fx.ctx.store.operating_state().unwrap().approval.is_none());
    assert_eq!(
        Store::open(&fx.root).unwrap().config.automation.mode,
        OperatingMode::Assist
    );
}

#[test]
fn operating_confirmation_requires_the_exact_typed_digest() {
    if run_as_owner("operating_confirmation_requires_the_exact_typed_digest") {
        return;
    }
    // Given a reviewed preview opened by a local owner.
    let (fx, mut view) = selected();
    assert_eq!(view.operating_preview(&fx.ctx), Action::None);
    let digest = view.operating.preview.as_ref().unwrap().id.clone();
    let screen = render_at(&mut view, &fx.ctx, 120, 42);
    assert!(screen.contains(&digest));
    // When Enter is pressed without typing the digest.
    view.on_key(k(KeyCode::Enter), &fx.ctx);
    // Then no authority is granted and the preview remains open for correction.
    assert!(fx.ctx.store.operating_state().unwrap().approval.is_none());
    assert!(view.operating.error.is_some());
    assert!(view.operating.preview.is_some());
}

#[test]
fn operating_owner_confirmation_publishes_the_reviewed_finite_grant() {
    if run_as_owner("operating_owner_confirmation_publishes_the_reviewed_finite_grant") {
        return;
    }
    // Given a preview and a digest typed character by character.
    let (fx, mut view) = selected();
    view.operating_preview(&fx.ctx);
    let preview = view.operating.preview.as_ref().unwrap().clone();
    for character in preview.id.chars() {
        view.on_key(c(character), &fx.ctx);
    }
    // When the owner explicitly confirms.
    view.on_key(k(KeyCode::Enter), &fx.ctx);
    // Then only the exact reviewed finite policy is persisted.
    let saved = Store::open(&fx.root).unwrap();
    assert_eq!(saved.config.automation, preview.policy);
    let event = saved
        .events()
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "operating_policy_confirmed")
        .unwrap();
    assert_eq!(event.actor, Some(corral_core::Actor::Owner));
    assert_eq!(event.detail["preview_id"], preview.id);
    assert_eq!(event.detail["grant_digest"], preview.grant_digest);
    assert_eq!(event.detail["revision"], preview.policy.revision);
    assert_eq!(
        event.detail["authority"],
        serde_json::to_value(&preview.policy.authority).unwrap()
    );
    assert_eq!(
        event.detail["tasks"],
        serde_json::json!(preview.policy.scope.tasks.keys().collect::<Vec<_>>())
    );
    assert_eq!(
        saved
            .operating_state()
            .unwrap()
            .approval
            .unwrap()
            .authorization,
        preview.id
    );
    assert_eq!(
        saved.operating_state().unwrap().approval.unwrap().digest,
        preview.grant_digest
    );
    assert!(view.operating.preview.is_none());
}

#[test]
fn operating_changed_config_rejects_previously_typed_confirmation() {
    if run_as_owner("operating_changed_config_rejects_previously_typed_confirmation") {
        return;
    }
    // Given a preview followed by a concurrent policy change.
    let (fx, mut view) = selected();
    view.operating_preview(&fx.ctx);
    let digest = view.operating.preview.as_ref().unwrap().id.clone();
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("review.max_rounds", "7"))
        .unwrap();
    for character in digest.chars() {
        view.on_key(c(character), &fx.ctx);
    }
    // When the owner submits the old digest.
    view.on_key(k(KeyCode::Enter), &fx.ctx);
    // Then stale consent never publishes authority.
    assert!(fx.ctx.store.operating_state().unwrap().approval.is_none());
    assert!(view.operating.error.is_some());
}

#[test]
fn operating_escape_cancels_without_authority() {
    if run_as_owner("operating_escape_cancels_without_authority") {
        return;
    }
    // Given a reviewed preview.
    let (fx, mut view) = selected();
    view.operating_preview(&fx.ctx);
    assert!(view.operating.preview.is_some());
    // When the owner cancels.
    view.on_key(k(KeyCode::Esc), &fx.ctx);
    // Then the preview closes without granting authority.
    assert!(view.operating.preview.is_none());
    assert!(fx.ctx.store.operating_state().unwrap().approval.is_none());
}
