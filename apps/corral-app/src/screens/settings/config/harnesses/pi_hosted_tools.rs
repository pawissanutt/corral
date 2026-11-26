use corral_protocol::config::{PiHostedToolsChange, PiHostedToolsRequest};
use nsh_core::prelude::*;

use super::pi_hosted_tool_models as models;
use super::{AppState, Token, button_row, cell, panel as boxed};
use crate::client::requests;
use crate::model::{
    Id,
    settings::pi_hosted_tool_models::listable,
    settings::pi_hosted_tools::{Editor, NOTE, Tool, unavailable},
};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::text::h3_flush;
use crate::ui::kit::{button::menu_trigger, overlay::counted};
use nsh_base::{menu, menu_item};

fn send(app: AppState, editor: Signal<Editor>, change: Option<PiHostedToolsChange>) {
    if app.feed.with_untracked(unavailable).is_some()
        || editor.with_untracked(|editor| editor.pending().is_some())
    {
        return;
    }
    let request_id = requests::new_id();
    let key = Id::from(request_id.clone());
    let saving = change.is_some();
    let sent = app.pi_hosted_tools(request_id, PiHostedToolsRequest { change });
    editor.update(|editor| editor.sent(key, saving, sent));
}

pub(super) fn panel(app: AppState) -> AnyElement {
    let editor = signal(Editor::default());
    let reason = computed(move || app.feed.with(unavailable));
    // Another source, store, enrolment or capability set starts over: a draft or a list read
    // from one daemon never reaches another, and replies to the old one are dropped.
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
                listable(feed),
            )
        })
    });
    effect(move || {
        let ready = source.with(|source| source.1.is_none());
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
    models::settle(app, editor);
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
                    view()
                        .child(tool_row(app, editor, Tool::Search))
                        .child(tool_row(app, editor, Tool::Image))
                        .into_element()
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
                                "Save hosted tools",
                                ButtonKind::Primary,
                                ButtonOptions {
                                    disabled: !can_save,
                                    ..Default::default()
                                },
                                "btn-pi-hosted-tools-save",
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
                                "btn-pi-hosted-tools-reload",
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
            h3_flush(app, "Hosted tools", Some("pi.toml".into())),
            cell(app, NOTE, Token::Ink3, false).into_element(),
            body.into_element(),
        ],
    )
}

fn tool_row(app: AppState, editor: Signal<Editor>, tool: Tool) -> AnyElement {
    let index = tool as usize;
    let selected = computed(move || editor.with(|state| state.draft[index].route));
    models::follow(app, editor, tool, selected);
    let control = dynamic(move || {
        let route = tool.routes()[selected.get()];
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
                    route.1,
                    format!("{} route: {}", tool.label(), route.1),
                    &format!("btn-pi-hosted-tools-{}-route", tool.id()),
                ));
            for (choice, route) in tool.routes().iter().enumerate() {
                let suffix = if route.0.is_empty() {
                    "default"
                } else {
                    route.0
                };
                picker = picker.child(
                    menu_item()
                        .accessibility_label(route.1)
                        .on_select(move || {
                            editor.update(|state| {
                                let draft = &mut state.draft[index];
                                if draft.route != choice {
                                    draft.route = choice;
                                    draft.model.clear();
                                }
                            })
                        })
                        .cursor_pointer()
                        .hover(|s| s.bg(crate::ui::tokens::color(Token::Hover)))
                        .focus(|s| s.bg(crate::ui::tokens::color(Token::Hover)))
                        .child(
                            view()
                                .test_id(&format!("seg-pi-hosted-tools-{}-{suffix}", tool.id()))
                                .child(cell(app, route.1, Token::Ink, false)),
                        ),
                );
            }
            picker.into_element()
        })
    });
    let model = dynamic(move || {
        let route = tool.routes()[selected.get()];
        if !route.needs_model() {
            return view().into_element();
        }
        untracked(|| {
            view()
                .child(models::model(app, editor, tool, route))
                .child(cell(app, route.guidance(tool), Token::Ink3, false))
                .child(dynamic(move || {
                    match editor.with(|state| state.draft[index].value(tool).err()) {
                        Some(error) => cell(app, error, Token::Held, false).into_element(),
                        None => view().into_element(),
                    }
                }))
                .into_element()
        })
    });
    view()
        .min_w(px(0.0))
        .child(cell(app, tool.label(), Token::Ink, false))
        .child(control)
        .child(model)
        .into_element()
}
