use super::fixture::*;
use corral_core::work::{Counter, WorkSnapshot};
use corral_core::{Lesson, Task};
use corral_ops::{Ctx, PmActionKind, PromotionCandidate, lesson_signals, pm_next, task_prompt};
use std::collections::BTreeSet;

/// db316644's keyword walk and lesson-signal filter, over the full compatibility tasks.
fn legacy_files(ctx: &Ctx, task: &Task) -> Vec<String> {
    let Some(project) = task.project.as_deref() else {
        return vec![];
    };
    let words = keywords(&format!("{} {}", task.title, task.body));
    let similar: BTreeSet<_> = ctx
        .store
        .list()
        .unwrap()
        .into_iter()
        .filter(|t| t.id != task.id && t.project == task.project)
        .filter(|t| {
            let overlap = keywords(&t.title).intersection(&words).count();
            overlap >= 2 || (overlap > 0 && t.kind.is_some() && t.kind == task.kind)
        })
        .map(|t| t.id)
        .collect();
    let mut files = BTreeSet::new();
    if !similar.is_empty() {
        for signal in lesson_signals(ctx, project).unwrap() {
            if matches!(signal.category.as_str(), "touched_files" | "landed_files")
                && signal.evidence.iter().any(|e| similar.contains(&e.task))
            {
                files.extend(signal.files);
            }
        }
    }
    files.into_iter().collect()
}

fn keywords(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|w| w.len() > 3)
        .map(str::to_ascii_lowercase)
        .collect()
}

// The db316644 shortlist result for this fixture's single eligible, non-conflicting Idea.
fn legacy_shortlist(fixture: &Fixture, files: Vec<String>) -> Vec<PromotionCandidate> {
    let task = &fixture.idea;
    let checked = !files.is_empty();
    let overlap = if checked {
        "overlap checked against 1 active task(s); 0 predicted conflict(s)"
    } else {
        "overlap check skipped: no declared or historically inferable Idea paths"
    };
    vec![PromotionCandidate {
        id: task.id.clone(),
        why: format!(
            "Eligible: nonempty acceptance, size <= M or unset, no plan or owner-only restriction, dependencies Done; {overlap}."
        ),
        predicted_conflicts: vec![],
        overlap_checked: checked,
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
        likely_files: files,
        compared_tasks: vec![fixture.active.id.clone()],
        unchecked_tasks: if checked {
            vec![]
        } else {
            vec![fixture.active.id.clone()]
        },
        overlap_evidence: vec![],
    }]
}

fn assert_likely(cells: &WorkSnapshot, fixture: &Fixture, calls: u64, similar: bool, bound: bool) {
    let (headers, bytes) = fixture.title_work();
    let title_passes = if bound { 1 } else { calls };
    let signal = if similar {
        let ctx = Ctx {
            store: if bound {
                fixture.ctx.store.read_only_snapshot().unwrap()
            } else {
                fixture.ctx.store.clone()
            },
            ..fixture.ctx.clone()
        };
        // PM prepares this pass before its hot scope; the worker's unbound call
        // includes the public reader's preparation inside the likely-files scope.
        let call = || lesson_signals(&ctx, "app").unwrap();
        let (signals, cells) = if bound {
            measure_evaluation(call)
        } else {
            measure(call)
        };
        assert!(signals.iter().any(|s| s.category == "touched_files"));
        cells
    } else {
        WorkSnapshot::default()
    };
    assert_eq!(cells.followup_calls(FU, "likely_files"), calls);
    for counter in Counter::ALL {
        let expected = match counter {
            Counter::ViewsOpened => (if bound { 0 } else { calls }) + signal.total(counter),
            Counter::TaskHeadDecodes => title_passes * headers + signal.total(counter),
            Counter::DecodedDocBytes => title_passes * bytes + signal.total(counter),
            _ => signal.total(counter),
        };
        assert_eq!(
            cells
                .cells(counter)
                .followup
                .get(&(FU.into(), "likely_files".into()))
                .copied()
                .unwrap_or(0),
            expected,
            "{counter:?}"
        );
    }
    assert_eq!(cells.followup_processes(FU, "likely_files"), 0);
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
fn pm_next_reaches_both_likely_files_calls_and_the_resource_walk() {
    for similar in [false, true] {
        let variants = [1, 10].map(|scale| shortlist_variant(scale, similar));
        assert_growth(&variants[0], &variants[1]);
    }
}

pub(super) fn shortlist_variant(scale: usize, similar: bool) -> WorkSnapshot {
    // Given one eligible Idea, one active task and dissimilar or similar terminal growth.
    let fixture = Fixture::new(scale, similar, false);
    // Enrichment has no saved lessons; this evaluation reaches exactly the two
    // shortlist inference calls, rather than a third selected-task preview.
    fixture.ctx.store.save_lessons("app", &[]).unwrap();
    let expected_files = legacy_files(&fixture.ctx, &fixture.idea);
    assert_eq!(
        legacy_files(&fixture.ctx, &fixture.active),
        Vec::<String>::new()
    );
    assert_eq!(
        expected_files,
        if similar {
            vec!["src/copper.rs".to_owned()]
        } else {
            vec![]
        }
    );
    let expected_shortlist = legacy_shortlist(&fixture, expected_files);
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
    // Observe uses a live store, while PM next binds all component reads to one view.
    assert_eq!(cells.followup_calls(FU, "sample"), 2);
    assert_eq!(cells.followup_hydrations(FU, "sample"), 0);
    assert_eq!(cells.followup_rows(FU, "sample"), 0);
    assert_eq!(cells.followup_doc_bytes(FU, "sample"), 0);
    assert_eq!(cells.followup_views(FU, "sample"), 1);
    assert_eq!(
        cells.followup_stats(FU, "sample"),
        2 * fixture.sample_stats()
    );
    assert_eq!(cells.followup_processes(FU, "sample"), 0);
    assert_likely(&cells, &fixture, 2, similar, true);
    cells
}

#[test]
fn worker_prompt_attributes_the_lesson_filter_and_preserves_selected_files() {
    for similar in [false, true] {
        let variants = [1, 10].map(|scale| worker_variant(scale, similar));
        assert_growth(&variants[0], &variants[1]);
    }
}

pub(super) fn worker_variant(scale: usize, similar: bool) -> WorkSnapshot {
    // Given a project task with a saved path-scoped lesson and cold history.
    let fixture = Fixture::new(scale, similar, false);
    let files = legacy_files(&fixture.ctx, &fixture.idea);
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
        &files,
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
    assert_likely(&cells, &fixture, 1, similar, false);
    assert_eq!(cells.unattributed_hydrations(), 0);
    cells
}
