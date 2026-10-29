use super::*;
use corral_core::{ArtifactKind, Store, Task};
use serde_json::json;

fn workspace() -> (tempfile::TempDir, Ctx) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("app/.git")).unwrap();
    let store = Store::init(tmp.path()).unwrap();
    (tmp, crate::resources::FixedResourceProbe::ctx(store))
}

fn idea(ctx: &Ctx, title: &str) -> Task {
    let task = ctx
        .store
        .create(
            title,
            Some("app"),
            Status::Idea,
            "## Acceptance criteria\n- Tests pass.",
        )
        .unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.created = "2026-10-15T00:00:00Z".parse().unwrap();
            Ok(())
        })
        .unwrap()
}

#[test]
fn acceptance_requires_real_nonempty_section() {
    for body in [
        "## Acceptance\nTests pass.",
        "# acceptance-criteria\n- Works.\n## Notes\nOther.",
        "### ACCEPTANCE CRITERIA ###\n#### Tests\n- Works.",
        "## Acceptance\n<!-- hidden -->\n\n## Acceptance criteria\nWorks.",
    ] {
        assert!(acceptance(body).is_some(), "{body}");
    }
    for body in [
        "Acceptance should be added.",
        "## Acceptance\n\n## Notes\nNot acceptance.",
        "## Acceptance\n- [ ]\n<!-- not content -->",
        "```\n## Acceptance\nNot a section.\n```",
        "<!--\n## Acceptance\nhidden\n-->",
        "## Acceptance\n### Tests\n",
    ] {
        assert_eq!(acceptance(body), None, "{body}");
    }
}

#[test]
fn every_eligibility_guard_excludes_unsafe_or_unactionable_ideas() {
    let (_tmp, mut ctx) = workspace();
    let eligible = idea(&ctx, "Valid idea");
    let dependency = ctx
        .store
        .create("Dependency", Some("app"), Status::Done, "")
        .unwrap();
    ctx.store
        .update(&eligible.id, |t| {
            t.depends_on = vec![dependency.id.clone()];
            t.size = Some(TaskSize::M);
            Ok(())
        })
        .unwrap();
    for guard in [
        "acceptance",
        "large",
        "xl",
        "plan",
        "design",
        "owner",
        "hold",
        "launch",
        "project",
        "unknown",
        "missing_dependency",
        "malformed_dependency",
        "blocked_dependency",
        "not_idea",
    ] {
        let task = idea(&ctx, guard);
        if guard == "acceptance" {
            ctx.store
                .owner_decision(
                    &task.id,
                    "narrow",
                    "Set up acceptance guard fixture",
                    "test",
                )
                .unwrap();
        }
        ctx.store
            .update(&task.id, |t| {
                match guard {
                    "acceptance" => t.body = "## Acceptance\n\n## Notes\nLater".into(),
                    "large" => t.size = Some(TaskSize::L),
                    "xl" => t.size = Some(TaskSize::Xl),
                    "plan" => t.plan_required = Some(true),
                    "design" => {
                        t.design = true;
                        t.plan_required = Some(false);
                    }
                    "owner" => t.labels = vec!["owner-only".into()],
                    "hold" => t.dispatch_hold = true,
                    "launch" => {
                        t.launch = Some(
                            serde_json::from_value(json!({
                                "run":"R1","request":"request","reserved":true,
                                "at":"2026-10-15T00:00:00Z","stage":"reserved"
                            }))
                            .unwrap(),
                        )
                    }
                    "project" => t.project = None,
                    "unknown" => t.project = Some("unknown".into()),
                    "missing_dependency" => t.depends_on = vec!["T-999".into()],
                    "malformed_dependency" => t.depends_on = vec!["../not-a-task".into()],
                    "blocked_dependency" => t.depends_on = vec![eligible.id.clone()],
                    "not_idea" => t.status = Status::Backlog,
                    _ => unreachable!(),
                }
                Ok(())
            })
            .unwrap();
    }
    let candidates = promotion_shortlist(&ctx).unwrap();
    assert_eq!(
        candidates.iter().map(|c| &c.id).collect::<Vec<_>>(),
        [&eligible.id]
    );
    assert_eq!(candidates[0].depends_on, [dependency.id]);
    ctx.store.config.review.plan_required = Some(true);
    assert!(promotion_shortlist(&ctx).unwrap().is_empty());
    ctx.store
        .update(&eligible.id, |t| {
            t.plan_required = Some(false);
            Ok(())
        })
        .unwrap();
    assert_eq!(promotion_shortlist(&ctx).unwrap().len(), 1);
}

#[test]
fn ranks_priority_then_age_then_conflict_risk_then_id() {
    let (_tmp, ctx) = workspace();
    let conflict = idea(&ctx, "Conflict");
    let unknown = idea(&ctx, "Unknown");
    let clear = idea(&ctx, "Clear");
    let same = idea(&ctx, "Same");
    let older = idea(&ctx, "Older");
    let urgent = idea(&ctx, "Urgent");
    let invalid_priority = idea(&ctx, "Invalid priority");
    ctx.store
        .update(&invalid_priority.id, |t| {
            t.priority = "!unknown".into();
            t.created = "2026-10-01T00:00:00Z".parse().unwrap();
            Ok(())
        })
        .unwrap();
    let active = ctx
        .store
        .create("Worker", Some("app"), Status::Running, "")
        .unwrap();
    ctx.store
        .update(&active.id, |t| {
            t.write_scope = vec!["src/shared/**".into()];
            Ok(())
        })
        .unwrap();
    for (task, path) in [
        (&conflict, "src/shared/*.rs"),
        (&clear, "docs/**"),
        (&same, "other/**"),
    ] {
        ctx.store
            .update(&task.id, |t| {
                t.write_scope = vec![path.into()];
                Ok(())
            })
            .unwrap();
    }
    ctx.store
        .update(&older.id, |t| {
            t.created = "2026-10-14T00:00:00Z".parse().unwrap();
            Ok(())
        })
        .unwrap();
    ctx.store
        .update(&urgent.id, |t| {
            t.priority = "P0".into();
            t.created = "2026-10-16T00:00:00Z".parse().unwrap();
            Ok(())
        })
        .unwrap();
    let candidates = promotion_shortlist(&ctx).unwrap();
    assert_eq!(
        candidates.iter().map(|c| &c.id).collect::<Vec<_>>(),
        [
            &urgent.id,
            &older.id,
            &clear.id,
            &same.id,
            &unknown.id,
            &invalid_priority.id
        ]
    );
    assert_eq!(
        candidates.iter().map(|c| c.rank).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6]
    );
    assert!(candidates.iter().all(|c| c.predicted_conflicts.is_empty()));
    assert!(!candidates.iter().any(|c| c.id == conflict.id));
    let c = &candidates[4];
    assert!(!c.overlap_checked);
    assert_eq!(c.unchecked_tasks, c.compared_tasks);
}

#[test]
fn one_shortlist_derives_each_projects_lesson_signals_once() {
    use corral_core::work::{Work, count};
    // Given history similar to three Ideas and to an active task of the same project.
    let (_tmp, ctx) = workspace();
    ctx.store
        .create("Repair alpha engine", Some("app"), Status::Done, "")
        .unwrap();
    for n in 0..3 {
        idea(&ctx, &format!("Improve alpha engine {n}"));
    }
    ctx.store
        .create("Tune alpha engine", Some("app"), Status::Running, "")
        .unwrap();
    // When the live board is shortlisted once (T-547).
    let start = count(Work::LessonSignalScan);
    let candidates = promotion_shortlist(&ctx).unwrap();
    // Then every candidate and active comparison shares one derivation of the history.
    assert_eq!(candidates.len(), 3);
    assert_eq!(count(Work::LessonSignalScan) - start, 1);
}

#[test]
fn reuses_historical_files_without_requiring_any_saved_lessons() {
    let (tmp, ctx) = workspace();
    let history = ctx
        .store
        .create("Repair alpha engine", Some("app"), Status::Done, "")
        .unwrap();
    let diff = tmp.path().join("history.diff");
    std::fs::write(&diff, "--- a/src/alpha.rs\n+++ b/src/alpha.rs\n").unwrap();
    crate::add_artifact(&ctx, &history.id, &diff, Some(ArtifactKind::Diff), None).unwrap();
    let task = idea(&ctx, "Improve alpha engine");
    let active = ctx
        .store
        .create("Worker", Some("app"), Status::NeedsMe, "")
        .unwrap();
    ctx.store
        .update(&active.id, |t| {
            t.write_scope = vec!["docs/**".into()];
            Ok(())
        })
        .unwrap();
    let other = ctx
        .store
        .create("Unscoped worker", Some("app"), Status::Running, "")
        .unwrap();
    let tasks = ctx.store.list().unwrap();
    let events = ctx.store.events().unwrap();
    let first = promotion_shortlist(&ctx).unwrap();
    assert_eq!(first, promotion_shortlist(&ctx).unwrap());
    assert_eq!(first[0].id, task.id);
    assert_eq!(first[0].likely_files, ["src/alpha.rs"]);
    assert!(first[0].predicted_conflicts.is_empty());
    assert!(!first[0].overlap_checked);
    assert_eq!(first[0].unchecked_tasks, [other.id]);
    assert_eq!(ctx.store.list().unwrap(), tasks);
    assert_eq!(ctx.store.events().unwrap(), events);
    ctx.store
        .update(&active.id, |t| {
            t.write_scope = vec!["src/*.rs".into()];
            Ok(())
        })
        .unwrap();
    assert!(promotion_shortlist(&ctx).unwrap().is_empty());
}

#[test]
fn wildcard_intersections_are_not_literal_path_matches() {
    for (a, b, expected) in [
        ("src/**/*.rs", "src/shared/*", true),
        ("src/ab*.rs", "src/*cd.rs", true),
        ("src/*.rs", "src/deep/*.rs", false),
        ("src/**", "docs/**", false),
        ("src/**", "src", true),
        ("src/foo.rs", "src/foo.rs", true),
    ] {
        assert_eq!(scopes_overlap(a, b), expected, "{a} {b}");
        assert_eq!(scopes_overlap(b, a), expected, "{b} {a}");
    }
}

#[test]
fn auto_candidates_exclude_overlapping_launches_only_within_the_same_project() {
    let (_tmp, mut ctx) = workspace();
    ctx.store.config.dispatch.promote_ideas = corral_core::PromoteIdeas::Auto;
    let task = idea(&ctx, "Promotion");
    ctx.store
        .update(&task.id, |t| {
            t.write_scope = vec!["src/**/*.rs".into()];
            Ok(())
        })
        .unwrap();
    let active = ctx
        .store
        .create("Launching", Some("app"), Status::Backlog, "")
        .unwrap();
    ctx.store
        .update(&active.id, |t| {
            t.write_scope = vec!["src/shared/*".into()];
            t.launch = Some(
                serde_json::from_value(json!({
                    "stage":"reserved", "reserved":true
                }))
                .unwrap(),
            );
            Ok(())
        })
        .unwrap();
    assert!(promotion_shortlist(&ctx).unwrap().is_empty());
    ctx.store
        .update(&active.id, |t| {
            t.project = Some("other".into());
            Ok(())
        })
        .unwrap();
    let candidates = promotion_shortlist(&ctx).unwrap();
    assert_eq!(candidates[0].id, task.id);
    assert!(candidates[0].overlap_checked);
    assert!(candidates[0].compared_tasks.is_empty());
}

#[test]
fn labels_are_optional_and_empty_labels_are_not_serialized() {
    let (_tmp, ctx) = workspace();
    let task = idea(&ctx, "Labels");
    let value = serde_json::to_value(&task).unwrap();
    assert!(value.get("labels").is_none());
    let decoded = serde_json::from_value::<Task>(value).unwrap();
    assert!(decoded.labels.is_empty());
    ctx.store
        .update(&task.id, |t| {
            t.labels = vec!["owner-only".into()];
            Ok(())
        })
        .unwrap();
    assert_eq!(ctx.store.get(&task.id).unwrap().labels, ["owner-only"]);
}
