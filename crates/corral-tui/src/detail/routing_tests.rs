use super::*;
use crate::board::tests::{code, key};
use crate::choice::tests::{click, draw, locate};
use crate::routes::tests::fixture;

fn loaded(ctx: &Ctx) -> DetailView {
    let mut view = DetailView::new(ctx, "T-002").unwrap();
    view.routing.cache.receive();
    view
}

fn saved(view: &mut DetailView, ctx: &Ctx, action: Action) {
    assert!(
        matches!(
            action,
            Action::Toast(Toast {
                tone: Tone::Good,
                ..
            })
        ),
        "{action:?}"
    );
    view.refresh(ctx);
    view.routing.cache.receive();
}

#[test]
fn detail_renders_route_profiles_sources_plan_review_and_warnings() {
    // Given a default implementer, explicit same-vendor reviewer, and required plan.
    let fx = fixture();
    fx.ctx
        .store
        .update("T-002", |task| {
            task.profile = None;
            task.reviewer = Some("gpt-sol".into());
            task.plan_required = Some(true);
            Ok(())
        })
        .unwrap();
    let expected =
        corral_ops::route(&fx.ctx, "T-002", &corral_core::ProfileChoice::default()).unwrap();
    let mut view = loaded(&fx.ctx);
    // When the actual detail explanation is opened and rendered.
    view.on_key(key('r'), &fx.ctx);
    let (text, _) = draw(&mut view, &fx.ctx);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // Then every source, identity and warning from the CLI service is visible.
    for value in [
        &expected.implementer.profile,
        &expected.implementer.harness,
        &expected.implementer.route.to_string(),
    ] {
        assert!(text.contains(value), "{value}\n{text}");
    }
    for mapping in [
        &expected.review.reviewer,
        expected.review.plan_reviewer.as_ref().unwrap(),
    ] {
        for value in [
            &mapping.selection,
            mapping.profile.as_ref().unwrap(),
            &mapping.source,
            &mapping.policy_source,
        ] {
            assert!(text.contains(value), "{value}\n{text}");
        }
        for warning in &mapping.warnings {
            assert!(text.contains(warning), "{warning}\n{text}");
        }
    }
    for warning in &expected.implementer.warnings {
        assert!(text.contains(warning), "{warning}\n{text}");
    }
    let planner = expected.review.planner.unwrap();
    assert!(
        text.contains(&planner.profile) && text.contains(&planner.source.to_string()),
        "{text}"
    );
}

#[test]
fn keyboard_profile_pin_updates_storage_and_route_without_dispatch() {
    // Given a Backlog detail and a loaded route.
    let fx = fixture();
    let mut view = loaded(&fx.ctx);
    view.on_key(key('p'), &fx.ctx);
    let expected = {
        let routing::Picker::Implementer(choice) = view.routing.picker.as_ref().unwrap() else {
            panic!("implementer picker")
        };
        let n = choice.profiles.len();
        choice.profiles[(choice.idx + 1) % n].clone()
    };
    // When the reused dispatch profile control is advanced and saved.
    view.on_key(key('l'), &fx.ctx);
    let action = view.on_key(code(KeyCode::Enter), &fx.ctx);
    saved(&mut view, &fx.ctx, action);
    // Then the task pin, new route and logged event all agree without starting a run.
    let task = fx.ctx.store.get("T-002").unwrap();
    assert_eq!(task.profile.as_deref(), Some(expected.as_str()));
    assert_eq!(
        view.routing
            .cache
            .get(&task.id)
            .unwrap()
            .next
            .as_ref()
            .unwrap()
            .implementer
            .profile,
        expected
    );
    assert!(task.runs.is_empty());
    assert_eq!(
        fx.ctx.store.events().unwrap().last().unwrap().kind,
        "implementer_override"
    );
}

#[test]
fn mouse_profile_pin_updates_the_displayed_pair() {
    // Given the detail route editor opened by clicking its footer-equivalent key.
    let fx = fixture();
    let mut view = loaded(&fx.ctx);
    view.on_key(key('r'), &fx.ctx);
    let (text, hits) = draw(&mut view, &fx.ctx);
    let point = locate(&text, "Pin implementer", "[Pin implementer]");
    click(&mut view, &fx.ctx, &hits, point, false);
    let (text, hits) = draw(&mut view, &fx.ctx);
    let point = locate(&text, "gpt-sol", " sonnet ");
    click(&mut view, &fx.ctx, &hits, (point.0 + 1, point.1), false);
    // When Save is clicked.
    let (text, hits) = draw(&mut view, &fx.ctx);
    let point = locate(&text, "[Save]", "[Save]");
    let action = click(&mut view, &fx.ctx, &hits, point, false);
    saved(&mut view, &fx.ctx, action);
    // Then the route follows the new task profile.
    assert_eq!(
        fx.ctx.store.get("T-002").unwrap().profile.as_deref(),
        Some("sonnet")
    );
    assert_eq!(
        view.routing.cache.get("T-002").unwrap().card.label,
        "sonnet -> gpt-sol"
    );
    assert!(draw(&mut view, &fx.ctx).0.contains("Implementer: sonnet"));
}

#[test]
fn keyboard_reviewer_override_updates_then_inherit_restores_policy() {
    // Given a task inheriting cross-vendor review.
    let fx = fixture();
    let mut view = loaded(&fx.ctx);
    view.on_key(key('v'), &fx.ctx);
    // When the picker chooses 'none' (inherit, cross-vendor, pm, none).
    for _ in 0..3 {
        view.on_key(code(KeyCode::Down), &fx.ctx);
    }
    let action = view.on_key(code(KeyCode::Enter), &fx.ctx);
    saved(&mut view, &fx.ctx, action);
    // Then code review is disabled and both storage and presentation reflect it.
    assert_eq!(
        fx.ctx.store.get("T-002").unwrap().reviewer.as_deref(),
        Some("none")
    );
    assert_eq!(
        view.routing.cache.get("T-002").unwrap().card.label,
        "gpt-sol -> none"
    );
    // When clear is pressed, then the inherited pair returns.
    let action = view.on_key(key('c'), &fx.ctx);
    saved(&mut view, &fx.ctx, action);
    assert_eq!(fx.ctx.store.get("T-002").unwrap().reviewer, None);
    assert_eq!(
        view.routing.cache.get("T-002").unwrap().card.label,
        "gpt-sol -> sonnet"
    );
}

#[test]
fn mouse_reviewer_override_and_clear_use_the_ops_event_path() {
    // Given an open route explanation.
    let fx = fixture();
    let mut view = loaded(&fx.ctx);
    view.on_key(key('r'), &fx.ctx);
    let (text, hits) = draw(&mut view, &fx.ctx);
    let point = locate(&text, "[Set reviewer]", "[Set reviewer]");
    click(&mut view, &fx.ctx, &hits, point, false);
    let (text, hits) = draw(&mut view, &fx.ctx);
    let point = locate(&text, " none", " none");
    click(&mut view, &fx.ctx, &hits, (point.0 + 2, point.1), false);
    let (text, hits) = draw(&mut view, &fx.ctx);
    let point = locate(&text, "[Save]", "[Save]");
    // When mouse Save is activated.
    let action = click(&mut view, &fx.ctx, &hits, point, false);
    saved(&mut view, &fx.ctx, action);
    assert_eq!(
        view.routing.cache.get("T-002").unwrap().card.label,
        "gpt-sol -> none"
    );
    let (text, hits) = draw(&mut view, &fx.ctx);
    let point = locate(&text, "[Clear reviewer]", "[Clear reviewer]");
    let action = click(&mut view, &fx.ctx, &hits, point, false);
    saved(&mut view, &fx.ctx, action);
    // Then clearing is durably logged and the route is restored.
    let events = fx.ctx.store.events().unwrap();
    let event = events.last().unwrap();
    assert_eq!(event.kind, "reviewer_override");
    assert_eq!(event.detail, serde_json::json!({"old":"none","new":null}));
    assert_eq!(
        view.routing.cache.get("T-002").unwrap().card.label,
        "gpt-sol -> sonnet"
    );
}
