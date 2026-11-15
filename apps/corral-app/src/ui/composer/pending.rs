//! Sends the composer handed to the client and the client has not answered yet, and the drafts
//! of named sessions. Both belong to the app's root, not to a composer: a composer closing (a
//! phone sheet, a page change) while its message waits for the outbox must neither lose the
//! answer nor bring the same words back for a second send.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use nsh_core::element::create_root;
use nsh_core::prelude::*;

use super::draft::keep;
use crate::model::daemon::{ClientError, SendAcceptance};
use crate::model::{Effect, Id};
use crate::state::AppState;

/// One message handed to the client: what the composer held, and what to do once the client's
/// outbox accepted it.
pub(super) struct Intent {
    pub scope: Id,
    /// The draft exactly as it was when Send was pressed.
    pub text: String,
    /// The staged attachments the message carries.
    pub keys: Vec<String>,
    /// The scope's saved draft at that moment, which the client retires with the message.
    pub saved: Option<String>,
    pub draft: Signal<String>,
    pub after: Vec<Effect>,
}

type Pending = Rc<RefCell<BTreeMap<String, Intent>>>;

/// A served session's draft, with the composers using it now.
struct Named {
    draft: Signal<String>,
    owner: Owner,
    users: usize,
}

#[derive(Default)]
struct Ledger {
    pending: Pending,
    drafts: BTreeMap<Id, Named>,
    watching: bool,
}

thread_local! {
    /// One ledger per app, found by its root owner so that apps sharing a thread (tests) never
    /// share intents.
    static LEDGERS: RefCell<HashMap<Owner, Ledger>> = RefCell::new(HashMap::new());
}

/// Runs `f` on the app's ledger. `f` must not call back into the ledger.
fn with_ledger<R>(app: AppState, f: impl FnOnce(&mut Ledger) -> R) -> R {
    LEDGERS.with(|all| {
        let mut all = all.borrow_mut();
        all.retain(|owner, _| owner.is_live());
        f(all.entry(app.root).or_default())
    })
}

fn pending(app: AppState) -> Pending {
    with_ledger(app, |ledger| ledger.pending.clone())
}

/// The draft of a served session's own composer. Owned by the root, so the words survive the
/// composer unmounting and the next one finds them as typed. It is dropped again once no
/// composer uses it, it is empty and no message of the scope waits for the client.
pub(super) fn named_draft(app: AppState, scope: &Id) -> Signal<String> {
    let known = with_ledger(app, |ledger| {
        ledger.drafts.get_mut(scope).map(|named| {
            named.users += 1;
            named.draft
        })
    });
    let draft = known.unwrap_or_else(|| {
        let (owner, draft) = app.root.run(|| create_root(|| signal(String::new())));
        with_ledger(app, |ledger| {
            ledger.drafts.insert(
                scope.clone(),
                Named {
                    draft,
                    owner,
                    users: 1,
                },
            );
        });
        draft
    });
    let scope = scope.clone();
    cleanup(move || {
        with_ledger(app, |ledger| {
            if let Some(named) = ledger.drafts.get_mut(&scope) {
                named.users = named.users.saturating_sub(1);
            }
        });
        prune(app, &scope);
    });
    draft
}

/// Forgets the scope's draft when nothing could still need it.
fn prune(app: AppState, scope: &Id) {
    let idle = with_ledger(app, |ledger| {
        let removable = ledger.drafts.get(scope).is_some_and(|named| {
            named.users == 0
                && untracked(|| named.draft.try_get()).is_some_and(|text| text.is_empty())
        }) && !ledger
            .pending
            .borrow()
            .values()
            .any(|intent| intent.scope == *scope);
        if removable {
            ledger.drafts.remove(scope).map(|named| named.owner)
        } else {
            None
        }
    });
    if let Some(owner) = idle {
        owner.dispose();
    }
}

/// Whether sending these words or attachments again would repeat a message still waiting for
/// the client's answer.
pub(super) fn duplicate(app: AppState, scope: &Id, text: &str, keys: &[String]) -> bool {
    pending(app).borrow().values().any(|intent| {
        intent.scope == *scope
            && (intent.text.trim() == text.trim()
                || intent.keys.iter().any(|key| keys.contains(key)))
    })
}

/// Whether `text` is, in this scope, the words of a message still waiting for the client's
/// answer, so it is no unsent draft to keep.
pub(super) fn sending(app: AppState, scope: &Id, text: &str) -> bool {
    pending(app)
        .borrow()
        .values()
        .any(|intent| intent.scope == *scope && intent.text == text)
}

/// Starts waiting for the client's answer to `cmd_id`.
pub(super) fn begin(app: AppState, cmd_id: String, intent: Intent) {
    let (pending, install) = with_ledger(app, |ledger| {
        (
            ledger.pending.clone(),
            !std::mem::replace(&mut ledger.watching, true),
        )
    });
    pending.borrow_mut().insert(cmd_id, intent);
    if install {
        watch(app, pending);
    }
}

/// The client did not take `cmd_id` at all, so no answer will come.
pub(super) fn abandon(app: AppState, cmd_id: &str) {
    pending(app).borrow_mut().remove(cmd_id);
}

/// Settles each intent the client answered. Made under the root, so it outlives every composer.
fn watch(app: AppState, pending: Pending) {
    app.root.run(move || {
        effect(move || {
            let answered: Vec<(String, Result<(), ClientError>)> = app.feed.with(|feed| {
                pending
                    .borrow()
                    .keys()
                    .filter_map(|cmd_id| {
                        let answer = match feed.send_acceptance.get(cmd_id.as_str())? {
                            SendAcceptance::Accepted => Ok(()),
                            SendAcceptance::Refused(error) => Err(error.clone()),
                        };
                        Some((cmd_id.clone(), answer))
                    })
                    .collect()
            });
            for (cmd_id, answer) in answered {
                let intent = pending.borrow_mut().remove(&cmd_id);
                if let Some(intent) = intent {
                    app.root.run(|| settle(app, intent, answer));
                }
                // The answer is read once; the map holds only answers still waited for.
                app.feed.update(|feed| {
                    feed.send_acceptance.remove(cmd_id.as_str());
                });
            }
        });
    });
}

/// The client answered for `intent`.
fn settle(app: AppState, intent: Intent, answer: Result<(), ClientError>) {
    let Intent {
        scope,
        text,
        keys,
        saved,
        draft,
        after,
    } = intent;
    match answer {
        // The outbox holds the message: its attachments are the message's now, its words
        // leave the composer if the owner has not edited them since, and what Send was to do
        // next happens.
        Ok(()) => {
            app.sent_attachments(&keys);
            // The client retires the saved draft in the same write as the message; until the
            // feed shows that, no composer of this scope may bring it back.
            if let Some(saved) = saved {
                let unchanged = app
                    .feed
                    .with_untracked(|feed| feed.drafts.get(scope.as_str()) == Some(&saved));
                if unchanged {
                    app.assistant
                        .update(|state| state.handed_off(scope.as_str(), saved));
                }
            }
            if draft.try_get().is_some_and(|now| now == text) {
                draft.try_set(String::new());
            }
            app.run(after);
        }
        // Nothing left the composer. Words kept back from the device while the send waited
        // are the owner's unsent draft again, unless they were edited since: those edits
        // were kept on their own.
        Err(error) => {
            app.toast(error.message);
            if draft.try_get().is_some_and(|now| now == text) {
                keep(app, &scope, text);
            }
        }
    }
    prune(app, &scope);
}
