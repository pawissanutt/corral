use corral_protocol::detail::{DetailBody, DetailRef, PlanDetail};
use corral_protocol::rows::{PlanRole, PlanRow, PlanState};

use crate::model::daemon::{DetailCache, Feed, detail_key};
use crate::model::effect::{Conn, Env};
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::plans::{Detail, State, live_detail, live_list_filtered};
use crate::model::projects::ProjectFilter;
use crate::model::{Id, Selection};

#[test]
fn legacy_plans_have_unknown_projects_and_ignore_stored_filters() {
    let mut feed = crate::model::preview::feed(0, String::new(), String::new(), None);
    feed.capabilities.remove(corral_protocol::PROJECTS_V1);
    feed.plans.clear();
    feed.plans.insert(
        "T-1".into(),
        PlanRow {
            id: "T-1".into(),
            title: "plan".into(),
            projects: vec![],
            role: None,
            linked: None,
            state: PlanState::Drafting,
            revision: None,
            delivery: None,
            members_total: 0,
            members_landed: 0,
            blockers: vec![],
            accepted_at: None,
            rev: 1,
        },
    );
    let env = Env {
        conn: Conn::Healthy,
        form: Form::Desktop,
    };
    for filter in [
        ProjectFilter::All,
        ProjectFilter::Unassigned,
        ProjectFilter::Project("corral".into()),
    ] {
        let list = live_list_filtered(&Selection::default(), &feed, env, &filter);
        let rows: Vec<_> = list
            .sections
            .iter()
            .flat_map(|section| &section.rows)
            .collect();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].project_chip.is_none());
        assert!(
            crate::model::projects::chips(
                &feed,
                &filter,
                crate::model::projects::FilterScreen::Plans
            )
            .is_empty()
        );
    }
}

#[test]
fn planning_states_never_become_owner_acceptance_sections_or_details() {
    let env = Env {
        conn: Conn::Healthy,
        form: Form::Desktop,
    };
    let mut feed = Feed::default();
    let state = State::default();
    for (i, status) in [
        PlanState::Drafting,
        PlanState::Reviewing,
        PlanState::PlanFailed,
    ]
    .into_iter()
    .enumerate()
    {
        let id = Id::from(format!("T-{i}"));
        feed.plans.insert(
            id.clone(),
            PlanRow {
                id: id.to_string(),
                title: "plan".into(),
                projects: vec![],
                role: Some(PlanRole::Design),
                linked: None,
                state: status,
                revision: None,
                delivery: None,
                members_total: 0,
                members_landed: 0,
                blockers: vec![],
                accepted_at: None,
                rev: 1,
            },
        );
        let target = DetailRef::Plan { id: id.to_string() };
        feed.details.insert(
            detail_key(&target),
            DetailCache {
                target,
                pages: vec![],
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::Plan(PlanDetail {
                    id: id.to_string(),
                    role: Some(PlanRole::Design),
                    linked: None,
                    revision: None,
                    revisions: vec![],
                    owner_decisions: vec![],
                    state: status,
                    document: None,
                    reviews: vec![],
                    members: vec![],
                    edges: vec![],
                    scenarios: None,
                    mapped: None,
                    acceptable: false,
                    accept_blockers: vec![],
                })),
            },
        );
        assert!(matches!(
            live_detail(&state, Some(&id), &feed, env),
            Detail::Planning(_)
        ));
    }
    let list = live_list_filtered(&Selection::default(), &feed, env, &ProjectFilter::All);
    assert_eq!(list.sections.len(), 3);
    assert!(list.sections.iter().all(|section| section.hue != Hue::You));
    assert!(
        list.sections
            .iter()
            .flat_map(|section| &section.rows)
            .all(|row| row.progress.is_none())
    );
}
