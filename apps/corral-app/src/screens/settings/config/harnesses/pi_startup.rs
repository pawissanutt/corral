use corral_protocol::config::{PiStartupChange, PiStartupRequest};
use nsh_core::prelude::*;

use super::{AppState, Token, button_row, cell, entry, panel as boxed};
use crate::client::requests::{self, ClientRequest};
use crate::model::{
    Id,
    settings::pi_startup::{Editor, NOTE, unavailable},
};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::text::h3_flush;

fn send(app: AppState, editor: Signal<Editor>, change: Option<PiStartupChange>) {
    if app.feed.with_untracked(unavailable).is_some()
        || editor.with_untracked(|editor| editor.pending().is_some())
    {
        return;
    }
    let request_id = requests::new_id();
    let key = Id::from(request_id.clone());
    let saving = change.is_some();
    let sent = requests::send(ClientRequest::PiStartup {
        request_id,
        request: PiStartupRequest { change },
    });
    editor.update(|editor| editor.sent(key, saving, sent));
}

pub(super) fn panel(app: AppState) -> AnyElement {
    let editor = signal(Editor::default());
    let reason = computed(move || app.feed.with(unavailable));
    effect(move || {
        let ready = reason.get().is_none();
        untracked(|| {
            editor.set(Editor::default());
            if ready {
                send(app, editor, None);
            }
        });
    });
    effect(move || {
        let Some(key) = editor.with(|editor| editor.pending().cloned()) else {
            return;
        };
        let reply = app
            .feed
            .with(|feed| feed.operations.get(&key).filter(|op| !op.loading).cloned());
        if let Some(reply) = reply {
            editor.update(|editor| editor.settle(reply));
        }
    });
    let body = dynamic(move || {
        if let Some(reason) = reason.get() {
            return cell(app, reason, Token::Ink3, false).into_element();
        }
        untracked(|| {
            let loaded = computed(move || editor.with(|editor| editor.saved.is_some()));
            let field = dynamic(move || {
                if !loaded.get() {
                    return view().into_element();
                }
                untracked(|| {
                    entry(
                        app,
                        "field-pi-startup-model".into(),
                        "Startup model (provider/model)",
                        move || editor.with(|editor| editor.draft.clone()),
                        move |draft| editor.update(|editor| editor.draft = draft),
                    )
                })
            });
            let status = dynamic(move || {
                let (note, error) = editor.with(Editor::status);
                cell(
                    app,
                    note,
                    if error { Token::Held } else { Token::Ink2 },
                    false,
                )
                .into_element()
            });
            let controls = dynamic(move || {
                let (waiting, can_save) =
                    editor.with(|editor| (editor.pending().is_some(), editor.change().is_some()));
                untracked(|| {
                    button_row(
                        app,
                        vec![
                            button_with(
                                app,
                                "Save startup model",
                                ButtonKind::Primary,
                                ButtonOptions {
                                    disabled: !can_save,
                                    ..Default::default()
                                },
                                "btn-pi-startup-save",
                                move || {
                                    let change = editor.with_untracked(Editor::change);
                                    if change.is_some() {
                                        send(app, editor, change);
                                    }
                                },
                            ),
                            button_with(
                                app,
                                "Reload",
                                ButtonKind::Ghost,
                                ButtonOptions {
                                    disabled: waiting,
                                    ..Default::default()
                                },
                                "btn-pi-startup-reload",
                                move || send(app, editor, None),
                            ),
                        ],
                    )
                })
            });
            view()
                .min_w(px(0.0))
                .child(field)
                .child(status)
                .child(controls)
                .into_element()
        })
    });
    boxed(
        app,
        vec![
            h3_flush(app, "Startup model", Some("pi.toml".into())),
            cell(app, NOTE, Token::Ink3, false).into_element(),
            body.into_element(),
        ],
    )
}
