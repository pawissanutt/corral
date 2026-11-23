//! The Changes tab's Impact view: the task's `system_review` as Activity › System's map at the
//! task's head (`screens::system::Host::Impact`: touched packages ringed with their lines,
//! packages holding dependents ringed apart, the rest dimmed), then the touched modules and the
//! Model impact list. Selecting an item shows its source anchor and selects its node; "Open in
//! System" opens Activity › System with that node selected. Every word is
//! `model::review::impact`'s.
//!
//! Test ids: "task-impact", "impact-item-<key>", "btn-open-in-system", "btn-impact-more-<n>".

use nsh_core::prelude::*;

use crate::model::review::impact::{
    self, Action, HEADING, Item, LEAD, LEGEND_DRAWN, LEGEND_LISTED, NO_ANCHOR, NO_MODULE,
    OPEN_IN_SYSTEM, READING, Section, Shown, Sign, TOUCHED,
};
use crate::model::system::marks::Marks;
use crate::model::{Form, Hue, Id};
use crate::screens::system::{Host, canvas, tree};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::rows::list_row;
use crate::ui::kit::text::plain;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::parts::{block, quiet};

/// The task whose Impact this is, and its project.
#[derive(Clone)]
struct At {
    task: Id,
    project: Id,
}

/// The Impact view of task `id`.
pub(crate) fn impact(app: AppState, id: Id) -> AnyElement {
    let target = {
        let id = id.clone();
        computed(move || vec![impact::request(id.as_str())])
    };
    super::keep::keep_details(app, target, None);
    let task = {
        let id = id.clone();
        computed(move || id.clone())
    };
    // The pick is read by each row and by the map, so selecting does not rebuild the list
    // (and lose the keyboard's place in it).
    let shown = computed(move || {
        let more = app.impact.with(|state| state.more.clone());
        app.feed
            .with(|feed| impact::shown(feed, id.as_str(), &more))
    });
    let said = computed(move || match shown.get() {
        Shown::Loading => Some(READING.to_owned()),
        Shown::Said(words) => Some(words),
        Shown::Ready(_) => None,
    });
    let scene = computed(move || match shown.get() {
        Shown::Ready(impact) => Some(impact.scene),
        _ => None,
    });
    let marks = computed(move || match shown.get() {
        Shown::Ready(impact) => impact.marks,
        _ => Marks::new(),
    });
    let waiting = computed(move || said.get().is_some());
    let body = dynamic(move || {
        if waiting.get() {
            return untracked(|| {
                dynamic(move || quiet(app, said.get().unwrap_or_default())).into_element()
            });
        }
        untracked(|| drawn(app, task, shown, scene, marks))
    });
    view().test_id("task-impact").child(body).into_element()
}

fn drawn(
    app: AppState,
    task: Memo<Id>,
    shown: Memo<Shown>,
    scene: Memo<Option<crate::model::system::scene::Scene>>,
    marks: Memo<Marks>,
) -> AnyElement {
    let form = app.form;
    let host = Host::Impact(marks, task);
    let map = match form {
        Form::Desktop => canvas::map(app, scene, host),
        Form::Phone | Form::Terminal => tree::tree(app, scene, host),
    };
    let head = dynamic(move || {
        let Shown::Ready(impact) = shown.get() else {
            return None;
        };
        Some(untracked(|| {
            view()
                .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                .child(plain(app, impact.summary.clone()))
                .children(
                    impact
                        .notices
                        .iter()
                        .map(|n| hued(app, n.clone(), Hue::Held).into_element()),
                )
                .into_element()
        }))
    });
    let list = dynamic(move || {
        let Shown::Ready(impact) = shown.get() else {
            return None;
        };
        let at = At {
            task: untracked(|| task.get()),
            project: Id::from(impact.project.clone()),
        };
        Some(untracked(|| {
            let touched = if impact.touched.is_empty() {
                quiet(app, NO_MODULE)
            } else {
                items(app, &at, impact.touched.clone())
            };
            view()
                .child(block(app, TOUCHED, None, touched))
                .child(block(
                    app,
                    HEADING,
                    None,
                    view()
                        .child(quiet(app, LEAD))
                        .children(
                            impact
                                .sections
                                .into_iter()
                                .map(|section| category(app, &at, section)),
                        )
                        .into_element(),
                ))
                .into_element()
        }))
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .child(head)
        .child(map)
        .child(quiet(
            app,
            if form == Form::Desktop {
                LEGEND_DRAWN
            } else {
                LEGEND_LISTED
            },
        ))
        .when(form == Form::Terminal, |v| v.child(term::blank()))
        .child(list)
        .into_element()
}

fn hued(app: AppState, words: String, hue: Hue) -> Text {
    plain(app, words).text_color(hue_ink(hue))
}

fn category(app: AppState, at: &At, section: Section) -> AnyElement {
    let task = at.task.clone();
    let shown = section.items.len() - section.hidden;
    let index = section.index;
    let content = view()
        .children(section.empty.map(|words| quiet(app, words)))
        .children(section.notes.into_iter().map(|note| quiet(app, note)))
        .child(items(
            app,
            at,
            section.items.into_iter().take(shown).collect(),
        ))
        .children((section.hidden > 0).then(|| {
            button(
                app,
                format!("Show {} more", section.hidden),
                ButtonKind::Quiet,
                format!("btn-impact-more-{index}"),
                move || app.impact_act(task.as_str(), Action::More(index)),
            )
        }))
        .into_element();
    view()
        .when(app.form != Form::Terminal, |v| v.pt(px(8.0)))
        .child(block(
            app,
            section.title,
            Some(section.supply.figure()),
            content,
        ))
        .into_element()
}

fn sign_hue(sign: Sign) -> Hue {
    match sign {
        Sign::Added => Hue::Ok,
        Sign::Removed => Hue::Fail,
        Sign::Changed => Hue::Worker,
        Sign::Dependent => Hue::Held,
        Sign::Neutral => Hue::Neutral,
    }
}

fn items(app: AppState, at: &At, items: Vec<Item>) -> AnyElement {
    view()
        .children(items.into_iter().map(|item| row(app, at.clone(), item)))
        .into_element()
}

fn row(app: AppState, at: At, item: Item) -> AnyElement {
    let form = app.form;
    let At { task, project } = at;
    let selected = {
        let (key, task) = (item.key.clone(), task.clone());
        computed(move || {
            app.impact.with(|state| {
                state.task.as_deref() == Some(task.as_str()) && state.item.as_ref() == Some(&key)
            })
        })
    };
    let label = format!("{} {} {}", item.sign.word(), item.main, item.right);
    let (key, node) = (item.key.clone(), item.node.clone());
    let line = list_row(
        app,
        label,
        format!("impact-item-{}", item.key),
        move || selected.get(),
        move || {
            app.impact_act(
                task.as_str(),
                Action::Item {
                    key: key.clone(),
                    node: node.clone(),
                },
            )
        },
        vec![
            view()
                .w(if form == Form::Terminal {
                    term::cells(2.0)
                } else {
                    px(16.0)
                })
                .flex_shrink_0()
                .child(text(item.sign.glyph()).text_color(hue_ink(sign_hue(item.sign))))
                .into_element(),
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(plain(app, item.main.clone()).number_of_lines(1u32))
                .into_element(),
            view()
                .flex_shrink_0()
                .child(text(item.right.clone()).text_color(color(Token::Ink3)))
                .into_element(),
        ],
    );
    let detail = dynamic(move || {
        if !selected.get() {
            return None;
        }
        let (project, node) = (project.clone(), item.node.clone());
        let (sub, anchor) = (item.sub.clone(), item.anchor.clone());
        Some(untracked(|| {
            view()
                .when(form == Form::Terminal, |v| v.pl(term::cells(3.0)))
                .when(form != Form::Terminal, |v| {
                    v.pl(px(28.0)).py(px(4.0)).gap(px(4.0)).items_start()
                })
                .children(sub.map(|sub| quiet(app, sub)))
                .child(plain(app, anchor.unwrap_or_else(|| NO_ANCHOR.into())))
                .child(button(
                    app,
                    OPEN_IN_SYSTEM,
                    ButtonKind::Ghost,
                    "btn-open-in-system",
                    move || app.open_in_system(project.clone(), node.clone()),
                ))
                .into_element()
        }))
    });
    view().child(line).child(detail).into_element()
}
