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
fn operating_confirmation_needs_the_explicit_grant_key() {
    if run_as_owner("operating_confirmation_needs_the_explicit_grant_key") {
        return;
    }
    // Given a reviewed preview opened by a local owner.
    let (fx, mut view) = selected();
    assert_eq!(view.operating_preview(&fx.ctx), Action::None);
    let screen = render_at(&mut view, &fx.ctx, 120, 42);
    assert!(screen.contains("[Grant] y"), "{screen}");
    // When Enter or another key is pressed instead of y.
    view.on_key(k(KeyCode::Enter), &fx.ctx);
    view.on_key(c('x'), &fx.ctx);
    // Then no authority is granted and the preview remains open.
    assert!(fx.ctx.store.operating_state().unwrap().approval.is_none());
    assert!(view.operating.preview.is_some());
}

#[test]
fn operating_owner_confirmation_publishes_the_reviewed_finite_grant() {
    if run_as_owner("operating_owner_confirmation_publishes_the_reviewed_finite_grant") {
        return;
    }
    // Given a reviewed preview.
    let (fx, mut view) = selected();
    view.operating_preview(&fx.ctx);
    let preview = view.operating.preview.as_ref().unwrap().clone();
    // When the owner explicitly grants it.
    view.on_key(c('y'), &fx.ctx);
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
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("review.max_rounds", "7"))
        .unwrap();
    // When the owner grants the old preview.
    view.on_key(c('y'), &fx.ctx);
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

#[test]
fn operating_pointer_confirmation_and_cancellation_match_keyboard_at_both_sizes() {
    if run_as_owner("operating_pointer_confirmation_and_cancellation_match_keyboard_at_both_sizes")
    {
        return;
    }
    for (width, height) in [(80, 24), (160, 45)] {
        // Given the actual scope preview and its modal hit map at this terminal size.
        let (fx, mut view) = selected();
        view.operating_preview(&fx.ctx);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        let mut hits = crate::HitMap::default();
        terminal
            .draw(|frame| view.draw(frame, frame.area(), &fx.ctx, &mut hits))
            .unwrap();
        assert!(
            hits.iter()
                .all(|(rect, _)| rect.right() <= width && rect.bottom() <= height)
        );
        let button = |code| {
            hits.iter()
                .find_map(|(_, hit)| match hit {
                    crate::Hit::Key(key) if key.code == code => Some(*key),
                    _ => None,
                })
                .unwrap()
        };
        let confirm = button(KeyCode::Char('y'));
        let cancel = button(KeyCode::Esc);
        // When a pointer cancels, nothing is granted.
        view.on_key(cancel, &fx.ctx);
        assert!(view.operating.preview.is_none());
        assert!(fx.ctx.store.operating_state().unwrap().approval.is_none());

        // When the owner reopens the preview and uses the Grant pointer hit.
        view.operating_preview(&fx.ctx);
        let preview = view.operating.preview.as_ref().unwrap().clone();
        view.on_key(confirm, &fx.ctx);
        // Then the persisted finite policy is exactly the reviewed policy.
        assert_eq!(
            Store::open(&fx.root).unwrap().config.automation,
            preview.policy
        );
    }
}

#[test]
fn operating_stale_preview_keeps_current_exceptions_inspectable() {
    if run_as_owner("operating_stale_preview_keeps_current_exceptions_inspectable") {
        return;
    }
    // Given an open confirmation followed by an external owner-only restriction.
    let (mut fx, mut view) = selected();
    view.operating_preview(&fx.ctx);
    let preview = view.operating.preview.as_ref().unwrap().clone();
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("projects.app.review.accept", "\"owner\""))
        .unwrap();
    fx.ctx.reload_config().unwrap();
    // When the live view refreshes without replacing the owner's preview.
    view.refresh(&fx.ctx);
    // Then current exceptions are inspectable, but old consent still fails closed.
    assert!(view.operating.stale);
    assert_eq!(view.operating.preview.as_ref().unwrap().id, preview.id);
    assert!(
        view.operating
            .current
            .iter()
            .any(|fact| fact.contains("app implementation acceptance"))
    );
    view.on_key(c('y'), &fx.ctx);
    assert!(fx.ctx.store.operating_state().unwrap().approval.is_none());
    assert!(view.operating.error.is_some());
}

#[test]
fn operating_unrelated_draft_save_preserves_concurrent_policy_and_owner_exception() {
    // Given an unrelated staged settings edit and a policy changed externally.
    let (mut fx, mut view) = selected();
    view.set(&fx.ctx, &Field::SlotLimit, "9").unwrap();
    fx.ctx.store.pause_operating_policy(true).unwrap();
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("projects.app.review.accept", "\"owner\""))
        .unwrap();
    fx.ctx.reload_config().unwrap();
    view.refresh(&fx.ctx);
    // When the unrelated draft is saved.
    view.save(&fx.ctx).unwrap();
    // Then neither the operating policy nor the concurrent exception is overwritten.
    let saved = Store::open(&fx.root).unwrap();
    assert!(saved.config.automation.paused);
    assert_eq!(saved.config.limits.running, 9);
    assert_eq!(
        saved.config.projects["app"].review.accept,
        Some(corral_core::Accept::Owner)
    );
    assert!(saved.operating_state().unwrap().approval.is_none());
}

#[test]
fn operating_conflicting_draft_cannot_overwrite_new_owner_exception() {
    // Given a draft acceptance edit whose same field changes externally.
    let (fx, mut view) = selected();
    let field = Field::Review(
        super::automation::Scope::Project("app".into()),
        super::automation::Key::Accept,
    );
    view.set(&fx.ctx, &field, "auto").unwrap();
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("projects.app.review.accept", "\"owner\""))
        .unwrap();
    // When the stale draft is saved, the lock-protected comparison rejects it.
    assert!(view.save(&fx.ctx).is_err());
    // Then current owner consent is preserved and the draft remains available.
    assert_eq!(
        Store::open(&fx.root).unwrap().config.projects["app"]
            .review
            .accept,
        Some(corral_core::Accept::Owner)
    );
    assert!(view.dirty());
}

#[test]
fn operating_picker_selects_tasks_and_projects_without_typing() {
    // Given the editor and a newest task in Review.
    let fx = fixture();
    let task = fx
        .ctx
        .store
        .create("Pick me", Some("app"), Status::Review, "Work")
        .unwrap();
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.created += jiff::SignedDuration::from_hours(1);
            Ok(())
        })
        .unwrap();
    let mut view = SettingsView::operating(&fx.ctx);
    // When the owner opens the task picker, selects the first task and applies.
    assert_eq!(view.operating_action(Key::Tasks, &fx.ctx), Action::None);
    view.on_key(c(' '), &fx.ctx);
    view.on_key(k(KeyCode::Enter), &fx.ctx);
    // Then that task is selected and the picker is closed.
    assert!(view.operating.pick.is_none());
    assert_eq!(
        view.operating.selection.tasks,
        std::collections::BTreeSet::from([task.id.clone()])
    );
    // When the owner picks the first project from the same list.
    view.operating_action(Key::Projects, &fx.ctx);
    view.on_key(c(' '), &fx.ctx);
    view.on_key(k(KeyCode::Enter), &fx.ctx);
    // Then that project is selected too.
    let first = fx.ctx.store.config.projects.keys().next().unwrap();
    assert!(view.operating.selection.projects.contains(first));
}

#[test]
fn operating_policy_details_stay_collapsed_until_opened() {
    // Given the editor with a finite selection.
    let (fx, mut view) = selected();
    let details = |view: &SettingsView| {
        view.operating
            .rows()
            .iter()
            .filter(|row| matches!(row.field, Field::Operating(Key::Fact(_))))
            .count()
    };
    assert_eq!(details(&view), 0);
    // When the owner opens the policy details.
    view.operating_action(Key::Details, &fx.ctx);
    // Then the full policy is listed.
    assert_eq!(details(&view), view.operating.current.len());
    assert!(details(&view) > 0);
}

#[test]
fn harness_question_json_reads_as_its_question() {
    // Given the raw request an agent's question tool records.
    let raw = r#"[{"header":"T-137 scope","id":"task_scope","question":"Implement only the approved child scope?","options":[]}]"#;
    // Then the owner sees the question, and plain text is unchanged.
    assert_eq!(
        crate::app::operating::readable_question(raw),
        "Implement only the approved child scope?"
    );
    assert_eq!(crate::app::operating::readable_question("Plain"), "Plain");
}
