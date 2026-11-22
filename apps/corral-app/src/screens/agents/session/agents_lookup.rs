//! The field under the Agents list that opens an agent of the session by its id
//! (`AgentsTab.lookup`), whether the list holds it or not: an agent the bounded live list leaves
//! out, or one of a host that stopped, opens as what corrald recorded. What is typed is checked
//! as it is typed; Open stays off until it is a well-formed id, and whether an agent was ever
//! recorded under it is what the opened conversation says. Shown with or without rows; `i` or
//! the ⋯ menu's row brings the keys to it. test ids `agent-lookup`, `field-agent-id`,
//! `btn-agent-open`.

use nsh_core::element::input;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::text::h3;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::super::super::run;
use super::Tab;
use super::agents_detail::words;

pub(super) fn lookup(app: AppState, tab: Tab) -> AnyElement {
    let form = app.form;
    let field = tab.part(|t| t.lookup.clone());
    let typed = signal(String::new());
    let checked = computed(move || {
        let typed = typed.get();
        field.with(|field| field.as_ref().map(|field| field.check(&typed)))
    });
    let open = move || {
        if let Some(Ok(action)) = checked.get_untracked() {
            typed.set(String::new());
            run(app, action);
        }
    };
    let shape = computed(move || {
        field.with(|field| {
            field
                .as_ref()
                .map(|field| (field.field, field.label, field.placeholder, field.note))
        })
    });
    dynamic(move || {
        let (id, label, placeholder, note) = shape.get()?;
        Some(untracked(|| {
            let handle = element_ref();
            app.focus_when_asked(id, handle);
            let entry = input()
                .element_ref(&handle)
                .test_id(id)
                .label(label)
                .placeholder(placeholder)
                .value(move || typed.get())
                .on_change(move |now: &str| typed.set(now.to_owned()))
                .on_submit(move |_: &str| open())
                .flex_1()
                .min_w(px(0.0))
                .text_color(color(Token::Ink));
            let entry = match form {
                Form::Terminal => entry.bg(AdaptiveColor::TRANSPARENT).into_element(),
                Form::Desktop | Form::Phone => entry
                    .font_family(FONT_TEXT)
                    .text_size(px(if form == Form::Phone { 16.0 } else { 13.0 }))
                    .min_h(px(if form == Form::Phone { 44.0 } else { 30.0 }))
                    .px(px(8.0))
                    .rounded(px(6.0))
                    .bg(color(Token::Bg))
                    .border_1()
                    .border_color(color(Token::Rule2))
                    .focus(|s| s.border_color(color(Token::Focus)))
                    .into_element(),
            };
            let ready = computed(move || matches!(checked.get(), Some(Ok(_))));
            let button = dynamic(move || {
                let disabled = !ready.get();
                Some(untracked(|| {
                    button_with(
                        app,
                        "Open",
                        ButtonKind::Ghost,
                        ButtonOptions {
                            disabled,
                            key_hint: Some("⏎"),
                            ..ButtonOptions::default()
                        },
                        "btn-agent-open",
                        open,
                    )
                }))
            });
            let problem = computed(move || match checked.get() {
                Some(Err(Some(why))) => Some(why),
                _ => None,
            });
            let said = dynamic(move || {
                let said = problem.get();
                Some(untracked(|| match said {
                    Some(why) => words(form, why, color(Token::Held)).into_element(),
                    None => words(form, note, color(Token::Ink3)).into_element(),
                }))
            });
            view()
                .test_id("agent-lookup")
                .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                .child(h3(app, label, None))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(if form == Form::Terminal { 16.0 } else { 8.0 }))
                        .child(entry)
                        .child(button),
                )
                .child(said)
                .into_element()
        }))
    })
    .into_element()
}
