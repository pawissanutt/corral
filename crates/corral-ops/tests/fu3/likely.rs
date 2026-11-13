use super::fixture::*;
use corral_core::Lesson;
use corral_core::work::WorkSnapshot;
use corral_ops::{PmActionKind, PromotionCandidate, pm_next, task_prompt};

// The declared-scope shortlist result for this fixture's single eligible Idea.
fn expected_shortlist(fixture: &Fixture) -> Vec<PromotionCandidate> {
    let task = &fixture.idea;
    let overlap = "overlap check skipped: no declared Idea write scope";
    vec![PromotionCandidate {
        id: task.id.clone(),
        why: format!(
            "Eligible: nonempty acceptance, size <= M or unset, no plan or owner-only restriction, dependencies Done; {overlap}."
        ),
        predicted_conflicts: vec![],
        overlap_checked: false,
        rank: 1,
        project: "app".into(),
        priority: task.priority.clone(),
        created: task.created,
        size: task.size,
        acceptance: "- Verify the result.".into(),
        plan_required: false,
        labels: vec![],
        depends_on: vec![],
        write_scope: vec![],
        compared_tasks: vec![fixture.active.id.clone()],
        unchecked_tasks: vec![fixture.active.id.clone()],
        overlap_evidence: vec![],
    }]
}

#[test]
fn a_genuinely_cancelled_current_plan_remains_an_owner_blocker() {
    let fixture = Fixture::new(10, true, false);
    let before = fixture.ctx.store.get(&fixture.active.id).unwrap();
    fixture
        .ctx
        .store
        .update(&fixture.active.id, |task| {
            task.plan_runs.last_mut().unwrap().state = corral_core::PlanRunState::Cancelled;
            Ok(())
        })
        .unwrap();
    let after = fixture.ctx.store.get(&fixture.active.id).unwrap();
    assert_eq!(before.plan_runs.len(), after.plan_runs.len());
    assert_eq!(
        &before.plan_runs[..before.plan_runs.len() - 1],
        &after.plan_runs[..after.plan_runs.len() - 1]
    );
    assert_eq!(
        after.plan_runs.last().unwrap().state,
        corral_core::PlanRunState::Cancelled
    );
    let action = pm_next(&fixture.ctx).unwrap();
    assert_eq!(action.kind, PmActionKind::Owner);
    assert_eq!(action.task.as_deref(), Some(fixture.active.id.as_str()));
    assert!(action.why.contains("Cancelled"), "{action:?}");
}

#[test]
fn pm_next_reaches_promotion_and_the_resource_walk() {
    for similar in [false, true] {
        let variants = [1, 10].map(|scale| shortlist_variant(scale, similar));
        assert_growth(&variants[0], &variants[1]);
    }
}

pub(super) fn shortlist_variant(scale: usize, similar: bool) -> WorkSnapshot {
    // Given one eligible Idea, one active task and terminal growth.
    let fixture = Fixture::new(scale, similar, false);
    fixture.ctx.store.save_lessons("app", &[]).unwrap();
    let expected_shortlist = expected_shortlist(&fixture);
    // When PM next takes the ordinary promotion branch.
    let ((state, action), cells) = measure_evaluation(|| {
        // Start admission with the same absent latch at both sizes, before observe
        // persists host PSI and the history-dependent volume inventory.
        let action = pm_next(&fixture.ctx).unwrap();
        let (state, _) =
            measure(|| corral_ops::resources::observe(&fixture.ctx, NOW.parse().unwrap()).unwrap());
        (state, action)
    });
    // Then inventory and branch preconditions hold before the counters are read.
    fixture.state_preconditions(&state);
    assert_eq!(action.kind, PmActionKind::IdeaPromotion);
    assert_eq!(action.shortlist, expected_shortlist);
    assert_eq!(
        action.shortlist[0].compared_tasks,
        vec![fixture.active.id.clone()]
    );
    assert_eq!(fixture.probe.samples(), 2);
    // PM admission remains hot even though it now precedes the pinned projection.
    assert_eq!(cells.followup_calls(FU, "sample"), 2);
    assert_eq!(cells.followup_hydrations(FU, "sample"), 0);
    assert_eq!(cells.followup_rows(FU, "sample"), 0);
    assert_eq!(cells.followup_doc_bytes(FU, "sample"), 0);
    // Pre-view admission and observe each own one live inventory read, independent of history.
    assert_eq!(cells.followup_views(FU, "sample"), 2);
    assert_eq!(
        cells.followup_stats(FU, "sample"),
        2 * fixture.sample_stats()
    );
    assert_eq!(cells.followup_processes(FU, "sample"), 0);
    cells
}

#[test]
fn worker_prompt_preserves_declared_lesson_selection_and_the_resource_walk() {
    for similar in [false, true] {
        let variants = [1, 10].map(|scale| worker_variant(scale, similar));
        assert_growth(&variants[0], &variants[1]);
    }
}

pub(super) fn worker_variant(scale: usize, similar: bool) -> WorkSnapshot {
    // Given a project task with a saved path-scoped lesson and cold history.
    let fixture = Fixture::new(scale, similar, false);
    let lessons = fixture.ctx.store.lessons("app").unwrap();
    let launch = run(
        "R-new".into(),
        fixture.ctx.store.root().join("absent-launch"),
    );
    let expected = corral_core::match_lessons(
        &lessons,
        &fixture.idea,
        Some("generic"),
        Some("generic"),
        fixture.ctx.store.config.lessons.top,
        NOW.parse().unwrap(),
    );
    // When the public worker-launch prompt builder runs.
    let ((state, prompt), cells) = measure(|| {
        let state = corral_ops::resources::read_status(&fixture.ctx).unwrap();
        let prompt = task_prompt(&fixture.ctx, &fixture.idea, &launch).unwrap();
        (state, prompt)
    });
    // Then compare the machine-readable payload, not pinned prompt prose.
    fixture.state_preconditions(&state);
    let selected: Vec<Lesson> = prompt
        .split("<corral-lessons>\n")
        .nth(1)
        .map(|text| {
            serde_json::from_str(text.split("\n</corral-lessons>").next().unwrap()).unwrap()
        })
        .unwrap_or_default();
    assert_eq!(selected, expected);
    assert_eq!(fixture.probe.samples(), 1);
    assert_sample(&cells, &fixture, 1, false);
    assert_eq!(cells.unattributed_hydrations(), 0);
    cells
}
