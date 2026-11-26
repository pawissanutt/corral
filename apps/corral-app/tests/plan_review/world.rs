//! What the scripted host holds and how it reads back: its live state, its open cards, the
//! submitted plan, and the authoritative status of a review as `session.action` `plan_status`
//! reports it from the host as it stands when asked.

use corral_protocol::detail::{
    CardKind, CardOption, CardQuestion, DetailBody, DetailGet, DetailRef, DetailReply,
    HostPresence, PlanModeDocPage, SessionCard, SessionCards, SessionPage,
};
use corral_protocol::session_host::{PlanStatus, SessionActionDone};
use corral_protocol::session_state::{
    CardBrief, HostNotice, PlanModeBrief, PlanModePhase, SessionMode, SessionState,
};
use serde_json::{Value, json};

pub const STORE: &str = "st-plan-harness";
pub const DEVICE: &str = "dev-plan-harness";
pub const SESSION: &str = "B-plan1";
pub const MODEL: &str = "anthropic/claude-harness";
pub const DUPLICATE: &str = "This review is already being answered; wait for it";
pub const DOC_HEAD: &str = "# Harness plan\n\nFirst step: change one file.\n";
pub const DOC_TAIL: &str = "\n## Verification\n\nThe whole plan was read.\n";
/// The option ids in the card's order: `opt-<card>-<n>`.
pub const EXECUTE: usize = 0;
pub const COMPACT: usize = 2;
/// The host instance every scenario starts on.
pub const HOST: &str = "test-host";

pub fn now() -> String {
    jiff::Timestamp::now().to_string()
}

pub fn card(n: u32) -> String {
    format!("plan-{n}")
}

/// What the scripted host holds: its live state, its open cards and the submitted plan.
pub struct World {
    pub state: SessionState,
    cards: Vec<SessionCard>,
    doc: String,
    rev: u64,
    seq: u64,
    /// The host instance now serving the session, and its approval counter; both outlive plan
    /// mode, so a status read after execution starts still names them.
    host: String,
    epoch: String,
}

impl World {
    pub fn new() -> Self {
        let mut world = World {
            state: SessionState {
                mode: SessionMode::Headless,
                turn: 1,
                model: Some(MODEL.into()),
                ..SessionState::default()
            },
            cards: Vec::new(),
            doc: format!("{DOC_HEAD}{DOC_TAIL}"),
            rev: 0,
            seq: 0,
            host: HOST.into(),
            epoch: "0".into(),
        };
        world.submit(1);
        world
    }

    /// The host submits plan `n`: a new review card, plan mode in review, no notices.
    pub fn submit(&mut self, n: u32) {
        let id = card(n);
        let title = format!("Harness plan {n}");
        let option = |id: &str, label: &str| CardOption {
            id: id.into(),
            label: label.into(),
            detail: None,
            unavailable: None,
            needs_text: None,
            destructive: false,
        };
        self.cards = vec![SessionCard {
            id: id.clone(),
            kind: CardKind::PlanReview,
            title: title.clone(),
            meta: Some(format!("revision {n}")),
            // Clipped, so Execute waits for the whole document as the host serves it.
            body: Some(DOC_HEAD.into()),
            body_more: true,
            questions: vec![CardQuestion {
                header: None,
                text: "How should this plan proceed?".into(),
                options: vec![
                    option("execute", "Execute"),
                    option("execute_clean", "Execute with clean context"),
                    option("execute_compact", "Execute with compacted context"),
                    option("revise", "Keep planning"),
                ],
                initial: None,
                multi: false,
                free_text: true,
            }],
            child: None,
            tool: Some("plan_submit".into()),
            opened_at: now(),
            expires_at: None,
        }];
        self.state.cards = vec![CardBrief {
            id,
            kind: CardKind::PlanReview,
            title: title.clone(),
            child: None,
            expires_at: None,
        }];
        self.state.plan_mode = Some(PlanModeBrief {
            phase: PlanModePhase::Review,
            title: Some(title),
            revision: Some(n),
            preparing: None,
            approval_epoch: Some(self.epoch.clone()),
            approval_host: Some(self.host.clone()),
            approval_pending: None,
            approval_error: None,
            approval_failures: Vec::new(),
        });
        self.state.notices.clear();
    }

    /// Another host instance now serves the session and restored the same review, idle: its
    /// approval counter starts over and it knows nothing of answers the last instance took.
    pub fn restart(&mut self, host: &str) {
        self.host = host.into();
        self.epoch = "0".into();
        if let Some(plan) = self.state.plan_mode.as_mut() {
            idle(plan, host);
        }
    }

    pub fn preparing(&mut self, kind: Option<&str>) {
        self.rev += 1;
        if let Some(plan) = self.state.plan_mode.as_mut() {
            if kind.is_some() {
                if plan.approval_pending != Some(true) {
                    self.epoch = self.rev.to_string();
                    plan.approval_epoch = Some(self.epoch.clone());
                }
                plan.approval_pending = Some(true);
                plan.approval_error = None;
            } else {
                plan.approval_pending = None;
            }
            plan.preparing = kind.map(str::to_owned);
        }
    }

    /// Execution was admitted: plan mode ends and the review card closes.
    pub fn started(&mut self) {
        self.state.plan_mode = None;
        self.state.cards.clear();
        self.cards.clear();
    }

    /// Preparing the context failed: the review stays open with the host's reason.
    pub fn failed(&mut self, reason: &str) {
        let was_pending = self
            .state
            .plan_mode
            .as_ref()
            .and_then(|plan| plan.approval_pending)
            .unwrap_or(false);
        if !was_pending {
            self.preparing(Some("compact"));
        }
        self.preparing(None);
        if let Some(plan) = self.state.plan_mode.as_mut() {
            plan.approval_error = Some(format!("The plan was not started: {reason}"));
        }
        let until = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(600);
        let id = format!("plan-failure-{}", self.seq);
        self.state.notices.push(HostNotice {
            id,
            tone: "error".into(),
            text: format!("The plan was not started: {reason}"),
            until: Some(until.to_string()),
        });
    }

    /// `plan_status` for `card`, read from the host as it is now.
    pub fn status(&self, card: &str) -> Value {
        let open = self.state.cards.iter().any(|brief| brief.id == card);
        let plan = self.state.plan_mode.as_ref().filter(|_| open);
        let status = PlanStatus {
            card: card.into(),
            host: self.host.clone(),
            epoch: self.epoch.clone(),
            pending: plan.is_some_and(|plan| {
                plan.approval_pending == Some(true) || plan.preparing.is_some()
            }),
            error: plan.and_then(|plan| plan.approval_error.clone()),
            failures: plan.map_or_else(Vec::new, |plan| plan.approval_failures.clone()),
            open,
        };
        serde_json::to_value(SessionActionDone {
            text: None,
            plan_status: Some(status),
        })
        .unwrap()
    }

    /// The live session as the host holds it now.
    pub fn session_page(&mut self, scope: &str) -> Value {
        let state = self.state.clone();
        self.page_of(scope, &state)
    }

    /// The live session announcing `state`, which need not be the host's own: a snapshot that
    /// was queued before the host moved on.
    pub fn page_of(&mut self, scope: &str, state: &SessionState) -> Value {
        self.rev += 1;
        if scope != SESSION {
            return json!({"topic": "session", "scope": scope, "epoch": 1, "generation": null,
                "events": [], "cursor": {"rev": self.rev, "after": null}, "more": false,
                "gap": null});
        }
        let mut event = |kind: Value| {
            self.seq += 1;
            let mut event = json!({"seq": self.seq, "generation": 1, "at": now()});
            if let (Some(event), Some(kind)) = (event.as_object_mut(), kind.as_object()) {
                event.extend(kind.clone());
            }
            event
        };
        let events = vec![
            event(json!({"kind": "live_state"})),
            event(json!({"kind": "live_part", "part": "cards"})),
        ];
        json!({"topic": "session", "scope": scope, "epoch": 1, "generation": 1,
            "events": events, "cursor": {"rev": self.rev, "after": null}, "more": false,
            "gap": null, "state": state})
    }

    pub fn detail(&mut self, get: DetailGet) -> Result<Value, (&'static str, String)> {
        let body = match get.target {
            DetailRef::Session { id } if id == SESSION => DetailBody::Session(SessionPage {
                id,
                role: "build".into(),
                task: None,
                run: None,
                machine: None,
                messages: Vec::new(),
                older: None,
                replyable: true,
                reply_refusal: None,
                host: HostPresence::Running,
            }),
            DetailRef::SessionCards { id } if id == SESSION => {
                DetailBody::SessionCards(SessionCards {
                    cards: self.cards.clone(),
                })
            }
            DetailRef::SessionPlanModeDoc { id, card }
                if id == SESSION && self.cards.iter().any(|open| open.id == card) =>
            {
                let plan = self.state.plan_mode.as_ref();
                DetailBody::SessionPlanModeDoc(PlanModeDocPage {
                    title: plan.and_then(|plan| plan.title.clone()).unwrap_or_default(),
                    revision: plan.and_then(|plan| plan.revision).unwrap_or(0),
                    body: self.doc.clone(),
                    bytes: self.doc.len() as u64,
                    offset: 0,
                })
            }
            DetailRef::SessionPlanModeDoc { .. } => {
                return Err(("not_found", "this session has no plan document".into()));
            }
            _ => return Err(("unavailable", "not served by the plan harness".into())),
        };
        self.rev += 1;
        let reply = DetailReply {
            etag: format!("harness-{}", self.rev),
            as_of: now(),
            seq: self.rev,
            next: None,
            body: Some(body),
        };
        Ok(serde_json::to_value(reply).unwrap())
    }
}

/// `plan` as `host` reports a review it restored with no approval underway.
pub fn idle(plan: &mut PlanModeBrief, host: &str) {
    plan.approval_host = Some(host.into());
    plan.approval_epoch = Some("0".into());
    plan.approval_pending = None;
    plan.preparing = None;
    plan.approval_error = None;
    plan.approval_failures.clear();
}
