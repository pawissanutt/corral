use corral_protocol::detail::{DetailBody, DetailRef, Plan, PlanStep, SessionPlan};
use corral_protocol::session_state::{Capability, SessionState};

use super::{Cx, Facts, SessionRole, Ui, ledger, plan_tab};
use crate::model::Id;
use crate::model::daemon::{DetailCache, Feed, detail_key};
use crate::model::health::daemon_env;
use crate::model::nav::Form;
use crate::model::session::transcript::LiveSession;

const SESSION: &str = "B-k3x9";

/// A session whose host serves its plan and last answered with `served`.
fn feed(served: SessionPlan) -> Feed {
    let mut feed = Feed::default();
    let mut session = LiveSession::default();
    session.replace_state(Some(Box::new(SessionState {
        can: vec![Capability {
            id: "plan".into(),
            unavailable: None,
        }],
        ..SessionState::default()
    })));
    feed.live.insert(Id::from(SESSION), session);
    let target = DetailRef::SessionPlan { id: SESSION.into() };
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            transcript_seams: Default::default(),
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::SessionPlan(served)),
        },
    );
    feed
}

/// What the Plan tab and the open ledger sheet say about the plan: (stale, empty) of each.
fn said(served: SessionPlan) -> [(bool, bool); 2] {
    let feed = feed(served);
    let id = Id::from(SESSION);
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let mut ui = Ui::default();
    ui.sidebar.plan.ledger = true;
    let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
    let tab = plan_tab(&cx);
    let sheet = ledger(&cx);
    [
        (tab.stale.is_some(), tab.empty.is_some()),
        (
            sheet.as_ref().is_some_and(|sheet| sheet.stale.is_some()),
            sheet.as_ref().is_some_and(|sheet| sheet.empty.is_some()),
        ),
    ]
}

/// A ledger the host could not confirm is named stale in the tab and in the sheet, never
/// presented as "no plan yet", whether or not it still has a last copy; only a confirmed
/// answer without a plan is empty.
#[test]
fn a_stale_ledger_is_named_stale_not_empty() {
    let stale_without_copy = SessionPlan {
        plan: None,
        stale: true,
        continued: None,
        history: Vec::new(),
    };
    assert_eq!(said(stale_without_copy), [(true, false), (true, false)]);

    let stale_with_copy = SessionPlan {
        plan: Some(Plan {
            title: "auth".into(),
            revision: 3,
            steps: vec![PlanStep {
                id: "s1".into(),
                text: "Split the module".into(),
                state: "in_progress".into(),
                detail: None,
                wait: None,
            }],
        }),
        stale: true,
        continued: None,
        history: Vec::new(),
    };
    assert_eq!(said(stale_with_copy), [(true, false), (true, false)]);

    let confirmed_none = SessionPlan {
        plan: None,
        stale: false,
        continued: None,
        history: Vec::new(),
    };
    assert_eq!(said(confirmed_none), [(false, true), (false, true)]);
}
