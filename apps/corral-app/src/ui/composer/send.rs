//! Send: the draft goes to the client, and the composer keeps it until the client's outbox
//! accepted it.

use super::{To, pending};
use crate::model::assistant::{self, ThreadRef};
use crate::state::AppState;

/// Sends the draft to the composer's session or Ask's target. The fixture stores it with its
/// state; with the daemon it goes out as one `assistant.send` under an identity allocated here
/// once (the client keeps it in this device's outbox while offline), with Ask's real context
/// and the refs of every staged attachment, all of them ready. The words and the attachments
/// stay in the composer while the outbox decides: its acceptance clears an unedited draft and
/// retires exactly the attachments the message carries; a refusal keeps them as typed.
pub(super) fn send(app: AppState, to: &To) {
    let text = to.draft.get_untracked();
    let target = app.ask_target.get_untracked();
    match &to.thread {
        ThreadRef::Fixture(id) => {
            let session = (!to.thread.is_quick()).then_some(*id);
            let mut clear = false;
            app.act(app.assistant, |state, env| {
                let outcome = state.key(assistant::Key::Send, &text, target, session, env);
                clear = outcome.clear_draft;
                outcome.effects
            });
            if clear {
                to.draft.set(String::new());
            }
        }
        ThreadRef::Live(scope) => {
            if text.trim().is_empty() {
                return;
            }
            let staged = app.ask_staged.get_untracked();
            let refs = match app
                .feed
                .with_untracked(|feed| assistant::send_refs(&staged, feed))
            {
                Ok(refs) => refs,
                Err(why) => {
                    app.toast(why);
                    return;
                }
            };
            let keys = assistant::carried(&refs);
            if pending::duplicate(app, scope, &text, &keys) {
                return;
            }
            let context = if to.thread.is_quick() {
                app.ask_context.get_untracked()
            } else {
                None
            };
            let stamp = assistant::Stamp {
                cmd_id: crate::client::requests::new_id(),
                typed_at: jiff::Timestamp::now().to_string(),
            };
            let docked = to.docked;
            let mut ready = None;
            app.reduce(app.assistant, |state, _, _, env| {
                match state.send_live(
                    scope.as_str(),
                    target,
                    &text,
                    context,
                    refs,
                    stamp,
                    docked,
                    env,
                ) {
                    Ok(message) => {
                        ready = Some(message);
                        Vec::new()
                    }
                    Err(refusal) => refusal,
                }
            });
            let Some((message, after)) = ready else {
                return;
            };
            let cmd_id = message.cmd_id.clone();
            let saved = app
                .feed
                .with_untracked(|feed| feed.drafts.get(scope.as_str()).cloned());
            // Waiting starts before the hand-off: the answer can only come after it.
            pending::begin(
                app,
                cmd_id.clone(),
                pending::Intent {
                    scope: scope.clone(),
                    text,
                    keys,
                    saved,
                    draft: to.draft,
                    after,
                },
            );
            if !app.send_message(message) {
                pending::abandon(app, &cmd_id);
            }
        }
    }
}
