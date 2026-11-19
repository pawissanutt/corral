//! The Usage view under Chat (`Chat › Usage`, `session::UsageView`): what the session spent as
//! its host reports it (cost, tokens, context; unknown is never zero), and where the read of its
//! members' usage stands. test id `session-usage`.

use nsh_core::prelude::*;

use crate::model::session::{Body, SessionPage};
use crate::state::AppState;

use super::tabs::{part, surface};

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Usage(view) => Some(view.part.clone()),
            _ => None,
        })
    });
    let content = dynamic(move || {
        shown.with(|body| {
            body.as_ref()
                .map(|body| untracked(|| part(app, body, "usage")))
        })
    });
    surface(app, "session-usage", content)
}
