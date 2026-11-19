//! The Proof tab of a task-bound session (`session::ProofTab`): the gate and review checks of the
//! run's task, and where their read stands, or why the tab does not apply. test id
//! `session-proof`.

use nsh_core::prelude::*;

use crate::model::session::{Body, SessionPage};
use crate::state::AppState;

use super::tabs::{part, surface};

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Proof(view) => Some(view.part.clone()),
            _ => None,
        })
    });
    let content = dynamic(move || {
        shown.with(|body| {
            body.as_ref()
                .map(|body| untracked(|| part(app, body, "proof")))
        })
    });
    surface(app, "session-proof", content)
}
