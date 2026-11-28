//! The phone's composer: while the agent works, Steer now | After this turn over one row of
//! field and button, the button stopping the turn while the agent works and the draft is empty;
//! the `@` popup over it while the draft ends in a mention.

use nsh_core::prelude::*;

use corral_protocol::session_state::SendPolicy;

use crate::model::Id;
use crate::model::hue::Hue;
use crate::model::session::{SendMode, SessionComposer};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};

use super::super::{Ask, Attach};
use super::{act, bar, reasons, send, standing, working};

/// The phone's line over the field, only while it has something to say: what the placeholder
/// does not, then the working indicator and Stop.
fn phone_line(app: AppState, composer: Memo<Option<SessionComposer>>) -> AnyElement {
    let noted = computed(move || {
        composer.with(|c| {
            c.as_ref().is_some_and(|c| {
                c.title.hue != Hue::Neutral
                    || c.title.last.is_some()
                    || c.shell.is_some()
                    || c.activity.is_some()
                    || c.stop.is_some()
            })
        })
    });
    when(
        move || noted.get(),
        move || {
            untracked(|| {
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(standing(app, composer))
                    .child(view().flex_1().min_w(px(0.0)))
                    .child(working(app, composer))
                    .into_element()
            })
        },
    )
    .into_element()
}

/// A remembered queue preference applies only while the current composer offers a queue.
fn send_policy(after: bool, primary: SendPolicy, queue: Option<SendPolicy>) -> SendPolicy {
    if after {
        queue.unwrap_or(primary)
    } else {
        primary
    }
}

/// The phone: while the agent works, Steer now | After this turn decides what the button does;
/// the field and the button in one row; the button stops the turn while the agent works and the
/// draft is empty.
pub(super) fn phone(
    app: AppState,
    id: Id,
    composer: Memo<Option<SessionComposer>>,
    field: AnyElement,
    mention: AnyElement,
    ask: Option<Ask>,
    attach: Attach,
) -> AnyElement {
    let after = signal(false);
    let working = computed(move || {
        composer.with(|c| c.as_ref().is_some_and(|c| c.mode == SendMode::Working))
    });
    let queues = computed(move || composer.with(|c| c.as_ref().is_some_and(|c| c.queue.is_some())));
    let choice = dynamic(move || {
        (working.get() && queues.get()).then(|| {
            untracked(|| {
                crate::ui::kit::button::seg(
                    app,
                    &[(false, "Steer now"), (true, "After this turn")],
                    move || after.get(),
                    move |value| after.set(value),
                    "seg-send-when",
                )
            })
        })
    });
    let action = computed(move || {
        composer.with(|c| {
            let c = c.as_ref()?;
            if c.mode == SendMode::Working && c.draft.trim().is_empty() {
                return c.stop.clone().map(Err);
            }
            Some(Ok(c.send.unavailable.is_some()))
        })
    });
    let id_for_button = id.clone();
    let button = dynamic(move || {
        action.get().map(|action| {
            let id = id_for_button.clone();
            untracked(|| match action {
                Err(stop) => {
                    let press = stop.action.clone();
                    button_with(
                        app,
                        "■",
                        ButtonKind::Ghost,
                        ButtonOptions {
                            danger: true,
                            ..ButtonOptions::default()
                        },
                        "btn-stop",
                        move || act(app, press.clone()),
                    )
                }
                Ok(dimmed) => button_with(
                    app,
                    "↑",
                    ButtonKind::Primary,
                    ButtonOptions {
                        disabled: dimmed,
                        ..ButtonOptions::default()
                    },
                    "btn-send",
                    move || {
                        let policy = composer.with_untracked(|c| {
                            let c = c.as_ref()?;
                            Some(send_policy(
                                after.get_untracked() && c.mode == SendMode::Working,
                                c.send.policy,
                                c.queue
                                    .as_ref()
                                    .filter(|queue| queue.unavailable.is_none())
                                    .map(|queue| queue.policy),
                            ))
                        });
                        if let Some(policy) = policy {
                            send(app, &id, composer, policy, ask, attach);
                        }
                    },
                ),
            })
        })
    });
    view()
        .test_id("session-composer")
        .flex_none()
        .px(px(12.0))
        .pt(px(6.0))
        .gap(px(6.0))
        .child(mention)
        .child(super::super::above(app, &id, ask, attach))
        .child(phone_line(app, composer))
        .child(choice)
        .child(
            view()
                .flex_row()
                .items_end()
                .gap(px(8.0))
                .child(super::super::phone_plus(app, &id, attach))
                .child(field)
                .child(button),
        )
        .child(bar(app, id, composer, ask, attach))
        .child(reasons(app, composer))
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hidden_queue_preference_cannot_block_steering_a_child() {
        let after = true;
        assert_eq!(
            send_policy(after, SendPolicy::Steer, Some(SendPolicy::FollowUp)),
            SendPolicy::FollowUp
        );
        assert_eq!(
            send_policy(after, SendPolicy::Steer, None),
            SendPolicy::Steer
        );
    }
}
