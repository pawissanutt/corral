use ratatui::crossterm::event::KeyCode;

use super::DetailView;
use crate::artifacts::panels_fixture::{fixture, key, render_at};
use crate::{Action, View};

#[test]
fn overview_keeps_goal_visible_without_promoting_activity_to_result() {
    // Given a task whose latest action is unrelated to its stable goal.
    let fx = fixture();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut().unwrap().summary =
                Some("Rebased onto main.\nVerified: command passed".into());
            Ok(())
        })
        .unwrap();
    let mut view = DetailView::new(&fx.ctx, &fx.id).unwrap();
    // When opened at narrow and wide terminal sizes.
    for width in [80, 120] {
        let (screen, _) = render_at(&mut view, &fx.ctx, width, 35);
        println!("OVERVIEW {width}\n{screen}");
        // Then canonical purpose and activity are separate, and the live pane takes no space.
        assert!(
            screen.contains("Cache shard reads per character."),
            "{screen}"
        );
        assert!(screen.contains("Rebased onto main."), "{screen}");
        assert!(!screen.contains("command passed"), "{screen}");
        assert!(!view.show_live);
        assert!(view.overview);
    }
}

#[test]
fn overview_navigation_preserves_detail_shortcuts_and_returns_to_context() {
    // Given the overview of a reviewable task.
    let fx = fixture();
    let mut view = DetailView::new(&fx.ctx, &fx.id).unwrap();
    assert_eq!(
        view.on_key(key(KeyCode::Char('r')), &fx.ctx),
        Action::OpenReview(fx.id.clone())
    );
    // When the spec is opened and then the overview is restored.
    view.on_key(key(KeyCode::Char('1')), &fx.ctx);
    assert!(!view.overview);
    assert!(!view.show_live);
    view.on_key(key(KeyCode::Char('0')), &fx.ctx);
    // Then the stable task context is restored.
    assert!(view.overview);
    assert!(
        render_at(&mut view, &fx.ctx, 80, 30)
            .0
            .contains("Cache shard reads per character.")
    );
}

#[test]
fn live_output_is_explicit_and_can_be_hidden_again() {
    // Given an overview that starts without live output.
    let fx = fixture();
    let mut view = DetailView::new(&fx.ctx, &fx.id).unwrap();
    // When live output is requested.
    view.on_key(key(KeyCode::Char('L')), &fx.ctx);
    // Then it is shown beside the spec until explicitly hidden.
    assert!(!view.overview && view.show_live);
    let screen = render_at(&mut view, &fx.ctx, 120, 30).0;
    println!("LIVE 120\n{screen}");
    assert!(screen.contains("No live run."));
    view.on_key(key(KeyCode::Char('L')), &fx.ctx);
    assert!(!view.show_live);
    assert!(
        !render_at(&mut view, &fx.ctx, 120, 30)
            .0
            .contains("No live run.")
    );
}

#[test]
fn explicit_goal_wins_over_introductory_activity() {
    // Given background text followed by a canonical goal.
    let fx = fixture();
    let mut task = fx.ctx.store.get(&fx.id).unwrap();
    task.body = "# Notes\nRebased yesterday.\n\n## Goal\nKeep shard reads bounded.\n\n## Checks\nRun tests.".into();
    // When the purpose excerpt is selected.
    let purpose = super::overview::purpose(&task);
    // Then it comes from the goal rather than the first activity paragraph.
    assert_eq!(purpose, "Keep shard reads bounded.");
}

#[test]
fn cumulative_outcome_survives_activity_and_warns_after_scope_changes() {
    // Given a result bound to the existing spec and a separate rebase activity report.
    let fx = fixture();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.result = Some(corral_core::TaskResult {
                text: "Shard reads now use a bounded per-character cache.".into(),
                run: "R0".into(),
                spec_digest: corral_ops::completion_spec_digest(task)?,
            });
            task.current_run_mut().unwrap().summary = Some("Rebased only.".into());
            Ok(())
        })
        .unwrap();
    let mut view = DetailView::new(&fx.ctx, &fx.id).unwrap();
    let (screen, _) = render_at(&mut view, &fx.ctx, 80, 35);
    assert!(screen.contains("Shard reads now use a bounded per-character cache."));
    view.on_key(key(KeyCode::PageDown), &fx.ctx);
    let (activity, _) = render_at(&mut view, &fx.ctx, 80, 35);
    assert!(activity.contains("Rebased only."), "{activity}");
    view.on_key(key(KeyCode::Home), &fx.ctx);
    println!("OUTCOME 80\n{screen}");
    // When scope changes without a new outcome.
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.body.push_str("\nAlso invalidate stale cache entries.");
            Ok(())
        })
        .unwrap();
    view.refresh(&fx.ctx);
    // Then the saved outcome remains readable but its provenance is warned.
    let (screen, _) = render_at(&mut view, &fx.ctx, 80, 35);
    assert!(screen.contains("R0 for an older or unverifiable spec"));
    assert!(screen.contains("Shard reads now use a bounded per-character cache."));
    println!("STALE OUTCOME 80\n{screen}");
}

#[test]
fn refresh_preserves_overview_after_returning_from_lazy_tabs() {
    // Given an owner who returned to Overview from a tab refreshed lazily.
    let fx = fixture();
    for tab in ['4', '5', '7'] {
        let mut view = DetailView::new(&fx.ctx, &fx.id).unwrap();
        view.on_key(key(KeyCode::Char(tab)), &fx.ctx);
        view.on_key(key(KeyCode::Char('0')), &fx.ctx);
        // When periodic sync refreshes the view.
        view.refresh(&fx.ctx);
        // Then it does not reopen the previous detail tab.
        assert!(view.overview);
        assert!(
            render_at(&mut view, &fx.ctx, 80, 30)
                .0
                .contains("Cache shard reads per character.")
        );
    }
}
