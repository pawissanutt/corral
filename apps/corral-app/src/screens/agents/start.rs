//! Where sessions start from Sessions: the New menu (a new assistant session, which opens on its
//! page once corrald made it; the New build sheet) and the New build sheet itself
//! (`agents::NewBuildForm`): the project, a name the host may choose, a new worktree or the
//! checkout, the model and the effort, then Start build, which reads `Starting…` while it waits.
//! A started build opens its session page; a refusal opens the refusal sheet with the host's
//! lines and the sheet keeps every field. A dialog on desktop, a sheet on the phone, the framed
//! region over the page in a terminal; mounted once by each shell. test ids: `btn-new`,
//! `btn-new-assistant`, `btn-new-build`, the sheet `start-build`, its fields
//! `seg-build-<field>-<value>`, `field-build-name`, `field-build-model`, `btn-start-build`.

use nsh_base::{menu, menu_item};
use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{
    self, Action, Choice, ModelPick, NewBuildForm, NewItem, NewPress, StartAction,
};
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with, menu_trigger};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::overlay::{Modal, counted, modal};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::run_agents;
use super::session::popup;

/// The phone's least touch target.
const TARGET: f32 = 44.0;

/// The New menu at the page head's trailing edge.
pub(crate) fn new_menu(app: AppState) -> AnyElement {
    let form = app.form;
    let new = agents::new_menu();
    let items: Vec<AnyElement> = new
        .items
        .into_iter()
        .map(|item| new_item(app, item))
        .collect();
    menu()
        .popup_style(popup(form))
        .backdrop_style(|s| s.bg(color(Token::Shell).opacity(0.5)))
        .on_open_change(counted(app))
        .trigger(menu_trigger(app, new.label, "New session", new.id))
        .child(view().children(items))
        .into_element()
}

fn new_item(app: AppState, item: NewItem) -> AnyElement {
    let form = app.form;
    let words = text(item.label)
        .number_of_lines(1u32)
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink));
    let cap = item.key.filter(|_| form != Form::Phone).map(|key| {
        text(key)
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3))
    });
    let content = view().flex_row().items_center().gap(px(12.0));
    let content = match form {
        Form::Terminal => content
            .h(term::rows(1.0))
            .px(term::cells(1.0))
            .child(words)
            .children(cap),
        Form::Desktop => content
            .px(px(12.0))
            .py(px(6.0))
            .child(words.font_family(FONT_TEXT).text_size(px(12.5)))
            .children(cap.map(|cap| cap.font_family(FONT_TEXT).text_size(px(11.5)))),
        Form::Phone => content
            .min_h(px(TARGET + 4.0))
            .px(px(18.0))
            .child(words.font_family(FONT_TEXT).text_size(px(16.0))),
    };
    let press = item.press;
    menu_item()
        .accessibility_label(item.label)
        .on_select(move || match press {
            NewPress::Assistant => app.new_assistant(),
            NewPress::Build => run_agents(app, Action::Start(StartAction::Open)),
        })
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus(|s| s.bg(color(Token::Hover)))
        .child(content.test_id(item.id))
        .into_element()
}

/// The New build sheet, mounted once by each shell beside the confirm and refusal sheets.
pub fn sheet(app: AppState) -> AnyElement {
    modal(
        app,
        move || app.agents.with(|state| state.start.open),
        move || run_agents(app, Action::Start(StartAction::Close)),
        move || content(app),
    )
}

fn start(app: AppState, action: StartAction) {
    run_agents(app, Action::Start(action));
}

/// The sheet as the model builds it now (tracked); none once it closed.
fn view_of(app: AppState) -> Memo<Option<NewBuildForm>> {
    computed(move || {
        let filter = app.scope_filter();
        app.feed.with(|feed| {
            app.agents
                .with(|state| agents::form(&state.start, feed, &filter))
        })
    })
}

fn content(app: AppState) -> Modal {
    let form = app.form;
    let sheet = view_of(app);
    let title = untracked(|| sheet.with(|s| s.as_ref().map(|s| s.title).unwrap_or_default()));
    let field = move |label: &'static str, body: AnyElement, note: Option<AnyElement>| {
        let caption = text(label)
            .number_of_lines(1u32)
            .text_color(color(Token::Ink2));
        let caption = match form {
            Form::Terminal => caption,
            Form::Desktop => caption
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD),
            Form::Phone => caption
                .font_family(FONT_TEXT)
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD),
        };
        view()
            .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
            .child(caption)
            .child(body)
            .children(note)
            .into_element()
    };
    let projects = field(
        "Project",
        choices(app, sheet, |s| s.project.choices.clone()),
        Some(quiet(app, sheet, |s| s.project.missing.clone(), Hue::Held)),
    );
    let name = field(
        "Name",
        text_field(
            app,
            sheet,
            "field-build-name",
            |s| s.name.value.clone(),
            StartAction::Name,
        ),
        Some(quiet(
            app,
            sheet,
            |s| Some(s.name.hint.to_owned()),
            Hue::Neutral,
        )),
    );
    let place = field(
        "Where",
        choices(app, sheet, |s| s.place.choices.clone()),
        Some(quiet(
            app,
            sheet,
            |s| Some(s.place.explain.clone()),
            Hue::Neutral,
        )),
    );
    let model = field(
        "Model",
        view()
            .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
            .child(choices(app, sheet, |s| s.model.choices.clone()))
            .child(text_field(
                app,
                sheet,
                "field-build-model",
                |s| s.model.typed.clone(),
                |text| StartAction::Model(ModelPick::Model(text)),
            ))
            .into_element(),
        Some(quiet(
            app,
            sheet,
            |_| Some("Or type <provider>/<model>.".to_owned()),
            Hue::Neutral,
        )),
    );
    let effort = dynamic(move || {
        sheet
            .with(|s| s.as_ref().is_some_and(|s| s.effort.is_some()))
            .then(|| {
                untracked(|| {
                    field(
                        "Effort",
                        choices(app, sheet, |s| {
                            s.effort
                                .as_ref()
                                .map(|e| e.choices.clone())
                                .unwrap_or_default()
                        }),
                        None,
                    )
                })
            })
    });
    let body = view()
        .gap(px(match form {
            Form::Terminal => 0.0,
            Form::Desktop => 14.0,
            Form::Phone => 16.0,
        }))
        .when(form != Form::Terminal, |body| body.pt(px(4.0)))
        .child(projects)
        .child(name)
        .child(place)
        .child(model)
        .child(effort)
        .into_element();
    Modal {
        test_id: "start-build".into(),
        title: title.to_owned(),
        sub: None,
        body,
        foot: Some(foot(app, sheet)),
        done: None,
        confirm: None,
    }
}

/// A quiet line under a field, in `hue` (ink-3 when neutral), while `pick` says one.
fn quiet(
    app: AppState,
    sheet: Memo<Option<NewBuildForm>>,
    pick: fn(&NewBuildForm) -> Option<String>,
    hue: Hue,
) -> AnyElement {
    let form = app.form;
    dynamic(move || {
        sheet.with(|s| s.as_ref().and_then(pick)).map(|words| {
            let words = text(words).text_color(if hue == Hue::Neutral {
                color(Token::Ink3)
            } else {
                hue_ink(hue)
            });
            match form {
                Form::Terminal => words.into_element(),
                Form::Desktop => words
                    .font_family(FONT_TEXT)
                    .text_size(px(12.0))
                    .into_element(),
                Form::Phone => words
                    .font_family(FONT_TEXT)
                    .text_size(px(13.0))
                    .into_element(),
            }
        })
    })
    .into_element()
}

/// One button per choice, the chosen one filled; a press picks it.
fn choices(
    app: AppState,
    sheet: Memo<Option<NewBuildForm>>,
    pick: fn(&NewBuildForm) -> Vec<Choice>,
) -> AnyElement {
    let form = app.form;
    let shown = computed(move || sheet.with(|s| s.as_ref().map(pick).unwrap_or_default()));
    dynamic(move || {
        let shown = shown.get();
        untracked(|| {
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }))
                .role(AccessibilityRole::Radiogroup)
                .children(
                    shown
                        .into_iter()
                        .map(|choice| {
                            let action = choice.action.clone();
                            let label = match &choice.detail {
                                Some(detail) => format!("{} · {detail}", choice.label),
                                None => choice.label.clone(),
                            };
                            button_with(
                                app,
                                label,
                                if choice.selected {
                                    ButtonKind::Primary
                                } else {
                                    ButtonKind::Ghost
                                },
                                ButtonOptions::default(),
                                choice.id.clone(),
                                move || run_agents(app, action.clone()),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    })
    .into_element()
}

/// A one-line text field whose value is the model's; each change goes to the sheet's reducer.
fn text_field(
    app: AppState,
    sheet: Memo<Option<NewBuildForm>>,
    test_id: &'static str,
    value: fn(&NewBuildForm) -> String,
    change: fn(String) -> StartAction,
) -> AnyElement {
    let form = app.form;
    let displayed = move || sheet.with(|s| s.as_ref().map(value).unwrap_or_default());
    let entry = copyable_input(app, None, displayed)
        .test_id(test_id)
        .label(test_id)
        .multiline(false)
        .submit_behavior(SubmitBehavior::BlurAndSubmit)
        .on_change(move |typed: &str| start(app, change(typed.to_owned())))
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT)
        .border_1()
        .border_color(color(Token::Rule2))
        .focus(|s| s.border_color(color(Token::Focus)));
    match form {
        Form::Terminal => entry.h(term::rows(1.0)).px(term::cells(1.0)).into_element(),
        Form::Desktop => entry
            .font_family(FONT_TEXT)
            .text_size(px(13.0))
            .h(px(32.0))
            .px(px(10.0))
            .rounded(px(6.0))
            .into_element(),
        Form::Phone => entry
            .font_family(FONT_TEXT)
            .text_size(px(16.0))
            .min_h(px(TARGET))
            .px(px(12.0))
            .rounded(px(10.0))
            .into_element(),
    }
}

/// Why Start cannot go yet or why the last start failed, then Cancel and Start build.
fn foot(app: AppState, sheet: Memo<Option<NewBuildForm>>) -> AnyElement {
    let form = app.form;
    let phone = form == Form::Phone;
    let start_button = computed(move || sheet.with(|s| s.as_ref().map(|s| s.start.clone())));
    let cancel = untracked(|| sheet.with(|s| s.as_ref().map(|s| s.cancel.clone())));
    let error = quiet(app, sheet, |s| s.error.clone(), Hue::Fail);
    let why = quiet(app, sheet, |s| s.start.unavailable.clone(), Hue::Held);
    let cancel = button_with(
        app,
        "Cancel",
        ButtonKind::Ghost,
        ButtonOptions {
            wide: phone,
            key_hint: Some("Esc"),
            ..ButtonOptions::default()
        },
        "btn-start-cancel",
        move || {
            if let Some(action) = cancel.clone() {
                run_agents(app, action);
            }
        },
    );
    let submit = dynamic(move || {
        start_button.get().map(|button| {
            let action = button.action.clone();
            untracked(|| {
                button_with(
                    app,
                    button.label,
                    ButtonKind::Primary,
                    ButtonOptions {
                        wide: phone,
                        disabled: button.unavailable.is_some(),
                        ..ButtonOptions::default()
                    },
                    button.id,
                    move || run_agents(app, action.clone()),
                )
            })
        })
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .child(error)
        .child(why)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .when(!phone, |row| row.justify_end())
                .child(cancel)
                .child(submit),
        )
        .into_element()
}
