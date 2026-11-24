//! Draft a lens and the task it filed: the kind and the owner's one sentence go out as the
//! engine's `lens_draft` operation; a Repair goes out as `lens_repair`. Either then names the
//! task corrald filed, with a way to open it.

use corral_types::Op;
use corral_types::lenses::LensKind;
use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use super::lenses::{hued, quiet};
use crate::model::act::{self, Act, ActStatus};
use crate::model::effect::Effect;
use crate::model::system::lens::{self, DRAFT_LEAD, DRAFT_PLACEHOLDER, KINDS};
use crate::model::system::{Action, Draft, Filed};
use crate::model::{Form, Hue, Id, Live};
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::kit::button::{ButtonKind, button, seg};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

/// Files a lens task through `op` and keeps its key to show what it filed.
pub(super) fn file(app: AppState, op: Op, title: String) {
    let mut key = None;
    app.act(app.work, |state, _| {
        let act = Act::new(&mut state.ids, op, false);
        key = Some(act.key.clone());
        vec![Effect::Act(act)]
    });
    if let Some(key) = key {
        app.system_act(Action::Filed(Filed { key, title }));
    }
}

/// Draft a lens: closed, a button; open, the kind and the one sentence.
pub(super) fn draft(app: AppState, project: String) -> AnyElement {
    let open = computed(move || app.system.with(|s| s.draft.is_some()));
    dynamic(move || {
        let project = project.clone();
        if !open.get() {
            return untracked(|| {
                button(
                    app,
                    "Draft a lens",
                    ButtonKind::Ghost,
                    "btn-lens-draft",
                    move || {
                        app.system_act(Action::Draft(Some(Draft {
                            kind: LensKind::State,
                            about: String::new(),
                        })))
                    },
                )
            });
        }
        untracked(|| draft_form(app, project))
    })
    .into_element()
}

fn draft_now(app: AppState) -> Draft {
    app.system.with_untracked(|s| {
        s.draft.clone().unwrap_or(Draft {
            kind: LensKind::State,
            about: String::new(),
        })
    })
}

fn draft_form(app: AppState, project: String) -> AnyElement {
    let form = app.form;
    let kinds = seg(
        app,
        &KINDS,
        move || {
            app.system
                .with(|s| s.draft.as_ref().map_or(LensKind::State, |d| d.kind))
        },
        move |kind| {
            let mut d = draft_now(app);
            d.kind = kind;
            app.system_act(Action::Draft(Some(d)));
        },
        "seg-lens-kind",
    );
    let send = {
        let project = project.clone();
        move || {
            let d = draft_now(app);
            let about = d.about.trim().to_owned();
            if about.is_empty() {
                app.toast("Say in one sentence what the lens should show.");
                return;
            }
            let title = format!("{} lens: {about}", lens::kind_word(Some(d.kind)));
            file(
                app,
                Op::LensDraft {
                    project: project.clone(),
                    kind: d.kind.as_str().to_owned(),
                    about,
                    ready: false,
                },
                title,
            );
        }
    };
    let on_enter = send.clone();
    let entry = copyable_input(app, None, move || {
        app.system.with(|s| {
            s.draft
                .as_ref()
                .map(|d| d.about.clone())
                .unwrap_or_default()
        })
    })
    .test_id("field-lens-about")
    .label("What the lens should show, in one sentence")
    .placeholder(DRAFT_PLACEHOLDER)
    .on_change(move |typed: &str| {
        let mut d = draft_now(app);
        d.about = typed.to_owned();
        app.system_act(Action::Draft(Some(d)));
    })
    .on_submit(move |_: &str| on_enter())
    .flex_1()
    .min_w(px(0.0))
    .text_color(color(Token::Ink))
    .caret_color(move || for_appearance(color(Token::Ink)))
    .bg(AdaptiveColor::TRANSPARENT);
    let field = match form {
        Form::Terminal => view()
            .flex_row()
            .child(text("› ").text_color(color(Token::Ink3)))
            .child(entry.h(term::rows(1.0)))
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .flex_row()
            .items_center()
            .h(px(30.0))
            .px(px(10.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(color(Token::Rule2))
            .bg(color(Token::Surface))
            .focus_within(|s| s.border_color(color(Token::Focus)))
            .child(
                entry
                    .h(px(30.0))
                    .font_family(FONT_TEXT)
                    .text_size(px(text_size(form))),
            )
            .into_element(),
    };
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(quiet(app, DRAFT_LEAD))
        .child(kinds)
        .child(field)
        .child(
            view()
                .flex_row()
                .gap(px(8.0))
                .child(button(
                    app,
                    "File the draft task",
                    ButtonKind::Primary,
                    "btn-lens-draft-send",
                    send,
                ))
                .child(button(
                    app,
                    "Cancel",
                    ButtonKind::Quiet,
                    "btn-lens-draft-cancel",
                    move || app.system_act(Action::Draft(None)),
                )),
        )
        .into_element()
}

/// The task the last Draft or Repair filed, once corrald says which.
pub(super) fn filed(app: AppState) -> AnyElement {
    let said = computed(move || {
        let filed = app.system.with(|s| s.filed.clone())?;
        let status = app.act_status(&filed.key);
        let task = app.feed.with(|feed| act::created(feed, filed.key.as_str()));
        Some((filed.title, status, task))
    });
    dynamic(move || {
        let (title, status, task) = said.get()?;
        Some(untracked(|| {
            let line = match (&status, &task) {
                (Some(ActStatus::Done), Some(task)) => {
                    hued(app, Hue::Ok, format!("Filed {task}: {title}"))
                }
                (Some(ActStatus::Refused(r)), _) => {
                    hued(app, Hue::Fail, format!("Not filed: {}", r.reason))
                }
                (Some(ActStatus::Failed(why)), _) => {
                    hued(app, Hue::Fail, format!("Not filed: {why}"))
                }
                // A slow operation stays "Filing" while `op.result` follows it; only a corrald
                // too old to name the operation answers a bare Running.
                (Some(ActStatus::Running), _) | (Some(ActStatus::Done), None) => quiet(
                    app,
                    format!("Sent: {title}. corrald did not name the task; look for it in Tasks."),
                )
                .into_element(),
                _ => quiet(app, format!("Filing: {title}…")).into_element(),
            };
            let open = task
                .filter(|_| status == Some(ActStatus::Done))
                .map(|task| {
                    button(
                        app,
                        format!("Open {task}"),
                        ButtonKind::Ghost,
                        "btn-lens-filed-open",
                        move || app.open_live(Live::Task(Id::from(task.clone()))),
                    )
                });
            view()
                .test_id("lens-filed")
                .gap(px(4.0))
                .child(line)
                .children(open)
                .into_element()
        }))
    })
    .into_element()
}
