use corral_protocol::config::{PiAppStartupChange, PiAppStartupRequest};
use nsh_base::{menu, menu_item};
use nsh_core::prelude::*;

use super::{AppState, Token, button_row, cell, entry, panel as boxed, space};
use crate::client::requests::{self, ClientRequest};
use crate::model::{
    Id,
    settings::pi_app_startup::{
        ASK_NOTE, ASK_UNAVAILABLE, BUILD_NOTE, Editor, Slot, ask_available, ask_label, ask_options,
        effective_ask, unavailable,
    },
};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with, menu_trigger};
use crate::ui::kit::overlay::counted;
use crate::ui::kit::text::h3_flush;

fn send(
    app: AppState,
    editor: Signal<Editor>,
    save: Option<Slot>,
    change: Option<PiAppStartupChange>,
) {
    if app.feed.with_untracked(unavailable).is_some()
        || editor.with_untracked(|editor| editor.pending().is_some())
    {
        return;
    }
    let request_id = requests::new_id();
    let key = Id::from(request_id.clone());
    let sent = requests::send(ClientRequest::PiAppStartup {
        request_id,
        request: PiAppStartupRequest { change },
    });
    editor.update(|editor| editor.sent(key, save, sent));
}

/// Both defaults, each with its own draft, status and save. One request is in flight at a time,
/// and a reply never replaces an unsaved edit to the other side.
pub(super) fn panel(app: AppState) -> AnyElement {
    let editor = signal(Editor::default());
    let reason = computed(move || app.feed.with(unavailable));
    // Another source, store or enrolment starts over: a draft read from one daemon never
    // reaches another.
    let source = computed(move || {
        let data = app.data.get();
        app.feed.with(|feed| {
            (
                data,
                unavailable(feed),
                feed.store.clone(),
                feed.session
                    .as_ref()
                    .map(|session| session.device.id.clone()),
            )
        })
    });
    effect(move || {
        let ready = source.with(|source| source.1.is_none());
        untracked(|| {
            editor.set(Editor::default());
            if ready {
                send(app, editor, None, None);
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
    dynamic(move || {
        if let Some(reason) = reason.get() {
            return boxed(
                app,
                vec![
                    h3_flush(app, "App startup defaults", Some("pi.toml".into())),
                    cell(app, reason, Token::Ink3, false).into_element(),
                ],
            );
        }
        untracked(|| {
            view()
                .min_w(px(0.0))
                .child(ask_section(app, editor))
                .child(space(app, 10.0))
                .child(build_section(app, editor))
                .child(space(app, 10.0))
                .child(reload(app, editor))
                .into_element()
        })
    })
    .into_element()
}

fn reload(app: AppState, editor: Signal<Editor>) -> AnyElement {
    dynamic(move || {
        let waiting = editor.with(|editor| editor.pending().is_some());
        untracked(|| {
            button_row(
                app,
                vec![button_with(
                    app,
                    "Reload both",
                    ButtonKind::Ghost,
                    ButtonOptions {
                        disabled: waiting,
                        ..Default::default()
                    },
                    "btn-pi-app-startup-reload",
                    move || send(app, editor, None, None),
                )],
            )
        })
    })
    .into_element()
}

fn status(app: AppState, editor: Signal<Editor>, slot: Slot) -> AnyElement {
    dynamic(move || {
        let (note, error) = editor.with(|editor| editor.status(slot));
        cell(
            app,
            note,
            if error { Token::Held } else { Token::Ink2 },
            false,
        )
        .into_element()
    })
    .into_element()
}

fn save_button(
    app: AppState,
    editor: Signal<Editor>,
    slot: Slot,
    label: &'static str,
    test_id: &'static str,
) -> AnyElement {
    dynamic(move || {
        let can_save = editor.with(|editor| editor.change(slot).is_some());
        untracked(|| {
            button_row(
                app,
                vec![button_with(
                    app,
                    label,
                    ButtonKind::Primary,
                    ButtonOptions {
                        disabled: !can_save,
                        ..Default::default()
                    },
                    test_id,
                    move || {
                        let change = editor.with_untracked(|editor| editor.change(slot));
                        if change.is_some() {
                            send(app, editor, Some(slot), change);
                        }
                    },
                )],
            )
        })
    })
    .into_element()
}

fn ask_section(app: AppState, editor: Signal<Editor>) -> AnyElement {
    let loaded = computed(move || editor.with(|editor| editor.saved.is_some()));
    let control = dynamic(move || {
        if !loaded.get() {
            return view().into_element();
        }
        let draft = editor.with(|editor| editor.ask.clone());
        let options = app.feed.with(ask_options);
        let label = ask_label(&options, draft.as_deref());
        untracked(|| {
            let mut picker = menu()
                .popup_style(|s| {
                    s.bg(crate::ui::tokens::color(Token::Surface))
                        .border_1()
                        .border_color(crate::ui::tokens::color(Token::Rule2))
                })
                .on_open_change(counted(app))
                .trigger(menu_trigger(
                    app,
                    label.clone(),
                    format!("Ask and Chat profile: {label}"),
                    "btn-pi-app-startup-ask-profile",
                ));
            for (profile, text) in options {
                let suffix = profile.clone().unwrap_or_else(|| "default".into());
                picker = picker.child(
                    menu_item()
                        .accessibility_label(text.clone())
                        .on_select(move || editor.update(|state| state.ask = profile.clone()))
                        .cursor_pointer()
                        .hover(|s| s.bg(crate::ui::tokens::color(Token::Hover)))
                        .focus(|s| s.bg(crate::ui::tokens::color(Token::Hover)))
                        .child(
                            view()
                                .test_id(&format!("seg-pi-app-startup-ask-{suffix}"))
                                .child(cell(app, text, Token::Ink, false)),
                        ),
                );
            }
            picker.into_element()
        })
    });
    let effective = dynamic(move || {
        let note = app.feed.with(|feed| {
            if ask_available(feed) {
                effective_ask(feed).map(|now| format!("Workspace default: {now}"))
            } else if feed.settings.is_some() {
                Some(ASK_UNAVAILABLE.to_owned())
            } else {
                None
            }
        });
        match note {
            Some(note) => cell(app, note, Token::Ink3, false).into_element(),
            None => view().into_element(),
        }
    });
    boxed(
        app,
        vec![
            h3_flush(app, "Ask and Chat default", Some("pi.toml".into())),
            cell(app, ASK_NOTE, Token::Ink3, false).into_element(),
            control.into_element(),
            effective.into_element(),
            status(app, editor, Slot::Ask),
            save_button(
                app,
                editor,
                Slot::Ask,
                "Save Ask and Chat default",
                "btn-pi-app-startup-ask-save",
            ),
        ],
    )
}

fn build_section(app: AppState, editor: Signal<Editor>) -> AnyElement {
    let loaded = computed(move || editor.with(|editor| editor.saved.is_some()));
    let field = dynamic(move || {
        if !loaded.get() {
            return view().into_element();
        }
        untracked(|| {
            entry(
                app,
                "field-pi-app-startup-build-model".into(),
                "Build startup model (provider/model)",
                move || editor.with(|editor| editor.build.clone()),
                move |draft| editor.update(|editor| editor.build = draft),
            )
        })
    });
    boxed(
        app,
        vec![
            h3_flush(app, "Build default (app-managed)", Some("pi.toml".into())),
            cell(app, BUILD_NOTE, Token::Ink3, false).into_element(),
            field.into_element(),
            status(app, editor, Slot::Build),
            save_button(
                app,
                editor,
                Slot::Build,
                "Save Build default",
                "btn-pi-app-startup-build-save",
            ),
        ],
    )
}
