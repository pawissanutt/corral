//! The chat operations the app waits on (a profile change, an End, a new chat): their replies
//! settle as they arrive, and a new chat's page opens once corrald created it. The conversation
//! itself is a session: Ask and the chat's page read it through `model::session`.

use corral_protocol::rows::AssistantRow;
use nsh_core::prelude::*;

use crate::model::assistant::OpOutcome;
use crate::state::AppState;

/// Settles the replies to the chat operations this app sent as they arrive, opening a new
/// chat's page once corrald created it.
pub(crate) fn watch_ops(app: AppState) {
    effect(move || {
        let keys = app.assistant.with(|state| state.live.waiting_ops());
        for key in keys {
            let outcome = app.with_operation(&key, |op| match op {
                None => OpOutcome::Pending,
                Some(op) if op.loading => OpOutcome::Pending,
                Some(op) => match &op.error {
                    Some(error) => OpOutcome::Failed(error.message.clone()),
                    None => op.decode::<AssistantRow>().map_or_else(
                        || OpOutcome::Failed("corrald replied without the session".into()),
                        OpOutcome::Row,
                    ),
                },
            });
            // A changed profile or an End shows through its row; only a refusal or a new chat
            // needs settling, and settling removes the key from the waiting set.
            let creating = app
                .assistant
                .with_untracked(|state| state.live.new_session.as_ref() == Some(&key));
            let settle = match &outcome {
                OpOutcome::Pending => false,
                OpOutcome::Failed(_) => true,
                OpOutcome::Row(_) => creating,
            };
            if settle {
                untracked(|| {
                    app.reduce(app.assistant, move |state, _, _, _| {
                        state.settle(&key, outcome)
                    });
                });
            }
        }
    });
}
