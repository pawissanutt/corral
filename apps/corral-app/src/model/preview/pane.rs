//! Pane requests against the sample: answered under their request id as corrald answers them.
//! A headless run refuses keys and focus with corrald's words; a sample pane takes typed keys
//! (no process runs to echo them) but cannot be focused, since no herdr runs for the preview.

use corral_protocol::pane::{PANE_FOCUS, PANE_KEYS, PaneFocus};
use corral_types::{Refusal, SessionRef};

use crate::model::Id;
use crate::model::daemon::{ClientError, Feed, Operation};
use crate::model::effect::PaneRequest;

/// Records the outcome of `request` under `key`.
pub fn pane(feed: &mut Feed, key: &Id, request: &PaneRequest) {
    let (method, target) = match request {
        PaneRequest::Focus(target) => (PANE_FOCUS, target),
        PaneRequest::Keys(keys) => (PANE_KEYS, &keys.target),
    };
    let mut operation = Operation {
        method: method.into(),
        loading: false,
        value: None,
        error: None,
        refusal: None,
    };
    match (session(feed, target), request) {
        (None, _) => {
            operation.error = Some(ClientError {
                code: "not_found".into(),
                message: "live_session_not_found".into(),
            });
        }
        (Some(SessionRef::Process { .. } | SessionRef::Pi { .. }), PaneRequest::Focus(_)) => {
            operation.refusal = Some(refusal(format!(
                "headless process sessions have no pane to focus; use `corral show {}`",
                target.task
            )));
        }
        (Some(SessionRef::Process { .. } | SessionRef::Pi { .. }), PaneRequest::Keys(_)) => {
            operation.refusal = Some(refusal(
                "headless process sessions have no pane to send keys to".into(),
            ));
        }
        (Some(SessionRef::Herdr { .. }), PaneRequest::Focus(_)) => {
            operation.error = Some(ClientError {
                code: "preview".into(),
                message: "Preview: no herdr runs here, so no pane was focused".into(),
            });
        }
        (Some(SessionRef::Herdr { .. }), PaneRequest::Keys(_)) => {
            operation.value = Some(serde_json::Value::Null);
        }
    }
    feed.operations.insert(key.clone(), operation);
}

/// The run's placement: a pi session's pane or process.
fn session(feed: &Feed, target: &PaneFocus) -> Option<SessionRef> {
    feed.agents
        .get(format!("{}_{}", target.task, target.run).as_str())
        .and_then(|row| row.run.session.as_ref())
        .map(|session| crate::model::agents::placement(session).clone())
}

fn refusal(reason: String) -> Refusal {
    Refusal {
        reason,
        next: Vec::new(),
        problems: None,
    }
}
