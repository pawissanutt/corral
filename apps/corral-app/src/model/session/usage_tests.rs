use corral_protocol::detail::{
    DetailBody, DetailRef, DetailReply, SessionUsage, UsageRow, UsageTotalRow,
};
use corral_protocol::session_state::{Capability, SessionState, UsageBrief};

use super::status::status;
use super::transcript::LiveSession;
use super::usage::usage_view;
use super::{Cx, Facts, SessionRole, SubView, Tab, Ui};
use crate::model::Id;
use crate::model::daemon::{DetailCache, Feed, detail_key};
use crate::model::health::daemon_env;
use crate::model::nav::Form;

const SESSION: &str = "B-k3x9";

fn row(key: &str, name: &str, kind: &str, cost_usd: Option<f64>) -> UsageRow {
    UsageRow {
        key: key.into(),
        name: name.into(),
        kind: kind.into(),
        tokens: 1_200,
        input: 900,
        output: 300,
        cost_usd,
        context_percent: None,
        turns: 3,
    }
}

/// A feed whose session reads `usage`, and whose live state briefs `brief`.
fn feed_with(usage: SessionUsage, brief: UsageBrief, can: Vec<Capability>) -> Feed {
    let mut feed = Feed::default();
    let mut live = LiveSession::default();
    live.replace_state(Some(Box::new(SessionState {
        usage: brief,
        can,
        ..SessionState::default()
    })));
    feed.live.insert(Id::from(SESSION), live);
    let target = DetailRef::SessionUsage { id: SESSION.into() };
    let body = DetailBody::SessionUsage(usage);
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            target,
            pages: vec![DetailReply {
                etag: "e1".into(),
                as_of: "2026-11-19T12:00:00Z".into(),
                seq: 1,
                next: None,
                body: Some(body.clone()),
            }],
            loading: false,
            stale: false,
            error: None,
            merged: Some(body),
        },
    );
    feed
}

fn usage(rows: Vec<UsageRow>, total: UsageTotalRow) -> SessionUsage {
    SessionUsage {
        model: Some("provider/model".into()),
        effort: None,
        context: None,
        rows,
        total: Some(total),
    }
}

fn brief(cost_usd: Option<f64>, cost_partial: bool) -> UsageBrief {
    UsageBrief {
        tokens: 2_400,
        cost_usd,
        cost_partial,
        ..UsageBrief::default()
    }
}

/// The Usage view and the status bar of `feed`'s session with the view open.
fn open(feed: &Feed) -> (super::UsageView, super::StatusBar) {
    let id = Id::from(SESSION);
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let ui = Ui {
        tab: Tab::Chat,
        sub: Some(SubView::Usage),
        ..Ui::default()
    };
    let cx = Cx::new(feed, &ui, &id, daemon_env(feed, Form::Desktop), &facts);
    (usage_view(&cx), status(&cx))
}

/// A child the host could not price must not drop out of the total as if it cost nothing: the
/// row says `unpriced`, the total is marked partial and names the child, and the status bar's
/// cost is the total, mark included. A session where nothing has a price shows no cost at all.
#[test]
fn an_unpriced_child_makes_the_total_partial_never_zero() {
    let feed = feed_with(
        usage(
            vec![
                row("pane", "Pane", "session", Some(0.5)),
                row("child:a2", "Parser worker", "worker", None),
            ],
            UsageTotalRow {
                tokens: 2_400,
                cost_usd: Some(0.5),
                partial: true,
            },
        ),
        // The brief the status bar would show if it ignored the read total.
        brief(Some(0.5), false),
        Vec::new(),
    );
    let (view, bar) = open(&feed);

    assert_eq!(view.rows.len(), 2);
    assert_eq!(view.rows[0].cost, "$0.50");
    assert_eq!(view.rows[1].cost, "unpriced");
    assert!(view.rows[1].unpriced);
    let total = view.total.expect("the host sent a total");
    assert!(total.partial);
    assert_eq!(total.cost, "$0.50+");
    assert!(
        total
            .why
            .as_deref()
            .is_some_and(|why| why.contains("Parser worker"))
    );
    assert_eq!(bar.cost.text, total.cost);
    assert!(bar.cost.partial);

    let nothing_priced = feed_with(
        usage(
            vec![
                row("pane", "Pane", "session", None),
                row("child:a2", "Parser worker", "worker", None),
            ],
            UsageTotalRow {
                tokens: 2_400,
                cost_usd: Some(0.0),
                partial: true,
            },
        ),
        brief(None, true),
        Vec::new(),
    );
    let (view, bar) = open(&nothing_priced);
    let total = view.total.expect("the host sent a total");
    assert!(!total.cost.contains("0.00"), "{}", total.cost);
    assert!(view.rows.iter().all(|row| row.cost == "unpriced"));
    assert!(!bar.cost.text.contains("0.00"), "{}", bar.cost.text);
}

/// A host that cannot serve usage now keeps the view's rows out of sight and says why, even
/// when an earlier read is still held.
#[test]
fn an_unavailable_host_shows_its_reason_and_no_rows() {
    let feed = feed_with(
        usage(
            vec![row("pane", "Pane", "session", Some(0.5))],
            UsageTotalRow {
                tokens: 1_200,
                cost_usd: Some(0.5),
                partial: false,
            },
        ),
        brief(Some(0.5), false),
        vec![Capability {
            id: "usage".into(),
            unavailable: Some("the model is busy".into()),
        }],
    );
    let (view, _) = open(&feed);

    assert_eq!(view.part.unavailable.as_deref(), Some("the model is busy"));
    assert!(view.rows.is_empty());
    assert!(view.total.is_none());
}

/// Three rows of a tenth of a cent each are priced, not free: rows, total, head line and status
/// bar all read `<$0.01`, and `$0.00` is left to a cost of exactly zero.
#[test]
fn a_cost_below_one_cent_reads_the_same_everywhere_and_is_not_zero() {
    let rows = vec![
        row("pane", "Pane", "session", Some(0.001)),
        row("child:a2", "Parser worker", "worker", Some(0.001)),
        row("child:a3", "Test worker", "worker", Some(0.001)),
    ];
    let feed = feed_with(
        usage(
            rows,
            UsageTotalRow {
                tokens: 490,
                cost_usd: Some(0.003),
                partial: false,
            },
        ),
        brief(Some(0.003), false),
        Vec::new(),
    );
    let (view, bar) = open(&feed);

    assert!(view.rows.iter().all(|row| row.cost == "<$0.01"));
    let total = view.total.expect("the host sent a total");
    assert_eq!(total.cost, "<$0.01");
    assert_eq!(bar.cost.text, "<$0.01");
    assert!(
        view.part.summary.starts_with("<$0.01 "),
        "{}",
        view.part.summary
    );

    let free = feed_with(
        usage(
            vec![row("pane", "Pane", "session", Some(0.0))],
            UsageTotalRow {
                tokens: 10,
                cost_usd: Some(0.0),
                partial: false,
            },
        ),
        brief(Some(0.0), false),
        Vec::new(),
    );
    assert_eq!(open(&free).1.cost.text, "$0.00");
}
