//! Cheap fallback projection regressions: no host, provider, sync or executable action.
use super::*;
use corral_core::{Status, Store};
use serde_json::json;

fn fixture() -> (tempfile::TempDir, Ctx, BTreeSet<String>, Task, Task) {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.projects.clear();
    for project in ["a-foreign", "z-local"] {
        store.config.projects.insert(
            project.into(),
            serde_json::from_value(json!({"path":"."})).unwrap(),
        );
    }
    store.save_config().unwrap();
    let foreign = store
        .create("foreign history", Some("a-foreign"), Status::Done, "")
        .unwrap();
    let local = store
        .create("local history", Some("z-local"), Status::Done, "")
        .unwrap();
    (
        tmp,
        crate::resources::FixedResourceProbe::ctx(store),
        ["z-local".into()].into(),
        foreign,
        local,
    )
}

#[test]
fn scoped_lessons_filter_before_enrichment_selection() {
    let (_tmp, mut ctx, projects, foreign, local) = fixture();
    ctx.store.config.lessons.after_landings = Some(1);
    ctx.store.config.lessons.profile = Some("generic".into());
    ctx.store.save_config().unwrap();
    ctx.store
        .log_event(
            Some(&foreign.id),
            "approved",
            json!({"run":"R1","strategy":"rebase"}),
        )
        .unwrap();
    let events = ctx.store.events().unwrap();
    assert_eq!(
        crate::pm_next(&ctx).unwrap().kind,
        PmActionKind::LessonsDistill
    );
    let scoped = next_for_projects(&ctx, &projects).unwrap();
    assert_eq!(scoped.kind, PmActionKind::Wait);
    assert_eq!(ctx.store.events().unwrap(), events);
    ctx.store
        .log_event(
            Some(&local.id),
            "approved",
            json!({"run":"R1","strategy":"rebase"}),
        )
        .unwrap();
    let events = ctx.store.events().unwrap();
    // Both projects are due; the alphabetically earlier foreign one still leads the CLI.
    let cli = crate::pm_next(&ctx).unwrap();
    let scoped = next_for_projects(&ctx, &projects).unwrap();
    let project = |action: &PmAction| {
        serde_json::from_str::<serde_json::Value>(&action.evidence[0]).unwrap()["project"].clone()
    };
    assert_eq!(project(&cli), "a-foreign");
    assert_eq!(scoped.kind, PmActionKind::LessonsDistill);
    assert_eq!(project(&scoped), "z-local");
    assert_eq!(ctx.store.events().unwrap(), events);
}

#[test]
fn scoped_inbox_filters_candidates_before_fallback_selection() {
    use corral_core::pm_inbox::{ItemKind, NewPmItem, RequestedAction};
    let (_tmp, ctx, projects, foreign, local) = fixture();
    let post = |refs: Vec<String>, action: Option<String>| {
        ctx.store
            .pm_inbox_post(NewPmItem {
                from: "owner".into(),
                kind: ItemKind::ActionRequest,
                subject: "Bound request".into(),
                body: String::new(),
                refs,
                requested_action: action.map(|s| RequestedAction::parse(&s).unwrap()),
                owner_words: None,
                key: None,
            })
            .unwrap()
            .0
    };
    let first = post(vec![foreign.id.clone()], None);
    for reference in [
        format!("artifacts/{}/R1/report.md", foreign.id),
        format!("{}:R1:report.md", foreign.id),
        format!("{}@P1", foreign.id),
        "project:a-foreign".into(),
        "T-999999".into(),
    ] {
        post(vec![reference], None);
    }
    for action in [
        format!("approve {}", foreign.id),
        format!("approve {}", foreign.id.to_lowercase()),
        format!("approve {}", foreign.id.trim_start_matches("T-")),
        format!("new scoped --depends={}", foreign.id),
        "lessons distill -pa-foreign".into(),
        "lessons distill --project a-foreign".into(),
        "mode autopilot --project=a-foreign".into(),
        "queue admit a-foreign".into(),
        "queue admit unknown-project".into(),
        "config set projects.a-foreign.default_profile generic".into(),
    ] {
        post(vec![], Some(action));
    }
    // Mixed scope must not leak the foreign reference merely because one ref is local.
    post(vec![local.id.clone(), foreign.id.clone()], None);
    let inside = post(
        vec![format!("artifacts/{}/R1/report.md", local.id)],
        Some(format!("approve {}", local.id)),
    );
    let items = ctx.store.pm_inbox_items().unwrap();
    assert!(crate::pm_next(&ctx).unwrap().command.contains(&first.id));
    let scoped = next_for_projects(&ctx, &projects).unwrap();
    assert_eq!(scoped.kind, PmActionKind::Inbox);
    assert!(scoped.command.contains(&inside.id));
    assert_eq!(ctx.store.pm_inbox_items().unwrap(), items);
    ctx.store
        .pm_inbox_withdraw(&inside.id, 0, "owner", "fixture finished")
        .unwrap();
    // Foreign-only requests do not displace the local no-work wait.
    assert_eq!(
        next_for_projects(&ctx, &projects).unwrap().kind,
        PmActionKind::Wait
    );
    let project_only = post(vec![], Some("lessons distill --project=z-local".into()));
    assert!(
        next_for_projects(&ctx, &projects)
            .unwrap()
            .command
            .contains(&project_only.id)
    );
    ctx.store
        .pm_inbox_withdraw(&project_only.id, 0, "owner", "fixture finished")
        .unwrap();
    // Items without typed task/project bindings remain genuinely workspace-wide.
    let workspace = post(vec![], None);
    assert!(
        next_for_projects(&ctx, &projects)
            .unwrap()
            .command
            .contains(&workspace.id)
    );
}
