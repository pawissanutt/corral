//! The Board tab of a build (`session::BoardTab`): the corral tasks the build filed and a plan
//! waiting for the owner, as its host counts them, and where the board's read stands. test id
//! `session-board`.

use nsh_core::prelude::*;

use crate::model::session::{Body, SessionPage};
use crate::state::AppState;

use super::tabs::{part, surface};

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Board(view) => Some(view.part.clone()),
            _ => None,
        })
    });
    let content = dynamic(move || {
        shown.with(|body| {
            body.as_ref()
                .map(|body| untracked(|| part(app, body, "board")))
        })
    });
    surface(app, "session-board", content)
}
