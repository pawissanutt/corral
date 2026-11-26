//! A hosted tool route's model: picked from what the host's provider login lists, or typed.
//! The typed ID stays reachable from the keyboard whatever the list's state.
use corral_protocol::config::PiHostedToolModelsRequest;
use nsh_base::{menu, menu_item};
use nsh_core::prelude::*;

use super::{AppState, Token, cell, entry};
use crate::client::requests;
use crate::model::{
    Id,
    daemon::Operation,
    settings::pi_hosted_tool_models::Choice,
    settings::pi_hosted_tools::{Editor, Route, Tool},
};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with, menu_trigger};
use crate::ui::kit::overlay::counted;
use crate::ui::tokens::color;

/// Asks for `request`'s list under a new key, so an earlier reply arrives late and is dropped.
fn discover(app: AppState, editor: Signal<Editor>, tool: Tool, request: PiHostedToolModelsRequest) {
    let request_id = requests::new_id();
    let key = Id::from(request_id.clone());
    let sent = app
        .pi_hosted_tool_models(request_id, request)
        .map(|sent| (key, sent));
    editor.update(|editor| {
        let models = &mut editor.models[tool as usize];
        if models.asked() == Some(request) {
            models.sent(sent);
        }
    });
}

/// Lists the models of the route `tool` shows, on load and whenever the route changes.
pub(super) fn follow(app: AppState, editor: Signal<Editor>, tool: Tool, route: Memo<usize>) {
    effect(move || {
        route.get();
        untracked(|| {
            let mut asked = None;
            editor.update(|editor| asked = editor.wanted(tool));
            if let Some(request) = asked {
                discover(app, editor, tool, request);
            }
        });
    });
}

/// Settles each list waiting on corrald with its reply.
pub(super) fn settle(app: AppState, editor: Signal<Editor>) {
    effect(move || {
        let keys: Vec<Id> = editor.with(|editor| {
            editor
                .models
                .iter()
                .filter_map(|models| models.pending().cloned())
                .collect()
        });
        if keys.is_empty() {
            return;
        }
        let replies: Vec<(Id, Operation)> = app.feed.with(|feed| {
            keys.iter()
                .filter_map(|key| {
                    let reply = feed.operations.get(key).filter(|op| !op.loading)?;
                    Some((key.clone(), reply.clone()))
                })
                .collect()
        });
        if !replies.is_empty() {
            editor.update(|editor| {
                for (key, reply) in &replies {
                    for models in &mut editor.models {
                        models.settle(key, reply);
                    }
                }
            });
        }
    });
}

/// The picker over a usable list, the typed ID when chosen or when there is no list, and the
/// list's state with Refresh.
pub(super) fn model(app: AppState, editor: Signal<Editor>, tool: Tool, route: Route) -> AnyElement {
    let index = tool as usize;
    // The chosen ID with the choices, so a pick between listed IDs relabels the trigger.
    let choices = computed(move || {
        editor.with(|editor| {
            let current = &editor.draft[index].model;
            let choices = editor.models[index].choices(current)?;
            Some((choices, current.clone()))
        })
    });
    let typing = computed(move || editor.with(|editor| editor.models[index].typing()));
    let picker = dynamic(move || match choices.get() {
        Some((choices, current)) => {
            untracked(|| picker(app, editor, tool, route, choices, current))
        }
        None => view().into_element(),
    });
    let field = dynamic(move || {
        if !typing.get() {
            return view().into_element();
        }
        untracked(|| {
            entry(
                app,
                format!("field-pi-hosted-tools-{}-model", tool.id()),
                tool.model_label(route),
                move || editor.with(|state| state.draft[index].model.clone()),
                move |model| editor.update(|state| state.type_model(tool, model)),
            )
        })
    });
    let note = dynamic(move || {
        let (note, problem) = editor.with(|state| state.models[index].note(tool, route.0));
        let ink = if problem { Token::Held } else { Token::Ink3 };
        cell(app, note, ink, false).into_element()
    });
    let refreshable = computed(move || editor.with(|state| state.models[index].refreshable()));
    let refresh = dynamic(move || {
        if !refreshable.get() {
            return view().into_element();
        }
        untracked(|| {
            button_with(
                app,
                "Refresh models",
                ButtonKind::Ghost,
                ButtonOptions::default(),
                format!("btn-pi-hosted-tools-{}-models-refresh", tool.id()),
                move || {
                    let asked = editor.with_untracked(|state| state.models[index].asked());
                    if let Some(request) = asked {
                        discover(app, editor, tool, request);
                    }
                },
            )
        })
    });
    view()
        .min_w(px(0.0))
        .child(picker)
        .child(field)
        .child(note)
        .child(refresh)
        .into_element()
}

fn picker(
    app: AppState,
    editor: Signal<Editor>,
    tool: Tool,
    route: Route,
    choices: Vec<Choice>,
    current: String,
) -> AnyElement {
    let index = tool as usize;
    let shown = if current.is_empty() {
        "Choose a model".to_owned()
    } else {
        current
    };
    let mut picker = menu()
        .popup_style(|s| {
            s.bg(color(Token::Surface))
                .border_1()
                .border_color(color(Token::Rule2))
        })
        .on_open_change(counted(app))
        .trigger(menu_trigger(
            app,
            shown.clone(),
            format!("{}: {shown}", tool.model_label(route)),
            &format!("btn-pi-hosted-tools-{}-model", tool.id()),
        ))
        // First, so a long list never hides the way to type an ID.
        .child(item(
            app,
            "Enter model ID manually".into(),
            format!("seg-pi-hosted-tools-{}-model-manual", tool.id()),
            move || editor.update(|state| state.models[index].manual = true),
        ));
    for choice in choices {
        let words = if choice.listed {
            choice.id.clone()
        } else {
            format!("{} (saved; not listed)", choice.id)
        };
        let test_id = format!("seg-pi-hosted-tools-{}-model-{}", tool.id(), choice.id);
        let id = choice.id;
        picker = picker.child(item(app, words, test_id, move || {
            editor.update(|state| state.pick(tool, id.clone()))
        }));
    }
    picker.into_element()
}

fn item(app: AppState, words: String, test_id: String, chosen: impl Fn() + 'static) -> AnyElement {
    menu_item()
        .accessibility_label(words.clone())
        .on_select(chosen)
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus(|s| s.bg(color(Token::Hover)))
        .child(
            view()
                .test_id(&test_id)
                .child(cell(app, words, Token::Ink, false)),
        )
        .into_element()
}
