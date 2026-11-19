//! The Task tab of a task-bound session (`session::TaskTab`): the run's task as the board names
//! it, and where the read of its record stands, or why the tab does not apply. test id
//! `session-task`.

use nsh_core::prelude::*;

use crate::model::session::{Body, SessionPage};
use crate::state::AppState;

use super::tabs::{part, surface};

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Task(view) => Some(view.part.clone()),
            _ => None,
        })
    });
    let content = dynamic(move || {
        shown.with(|body| {
            body.as_ref()
                .map(|body| untracked(|| part(app, body, "task")))
        })
    });
    surface(app, "session-task", content)
}
