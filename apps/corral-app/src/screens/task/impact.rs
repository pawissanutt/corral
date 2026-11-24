//! The Changes tab's Impact view: the task's `system_review` as Activity › System's map at the
//! task's head (`screens::system::Host::Impact`: touched nodes ringed with their lines, nodes
//! holding dependents ringed apart, the rest dimmed), drilled like Activity › System with a
//! breadcrumb, then the touched modules and the Model impact list. Selecting an item opens its
//! node's level, selects the node and shows its source anchor; "Open in System" opens Activity
//! › System where its own map has the node, or says why it cannot. Every word is
//! `model::review::impact`'s.
//!
//! Test ids: "task-impact", "impact-crumb-<n>", "impact-item-<key>", "btn-open-in-system",
//! "impact-unreachable", "btn-impact-more-<n>", "impact-lens" (a touched lens, drawn under its item).

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::review::impact::{
    self, Action, CHECKING, HEADING, Item, LEAD, LEGEND_DRAWN, LEGEND_LISTED, NO_ANCHOR, NO_MODULE,
    READING, Reach, Section, Shown, Sign, TOUCHED,
};
use crate::model::system::Action as MapAction;
use crate::model::system::lens::{self, Touch};
use crate::model::system::marks::Marks;
use crate::model::system::scene::Scene;
use crate::model::{Form, Hue, Id};
use crate::screens::system::{Host, canvas, lenses, tree};
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

/// What the list shows: it does not change with the map's level or pick, so selecting keeps
/// the keyboard's place in it.
type Listing = Option<(Id, Vec<Item>, Vec<Section>)>;

/// The Impact view of task `id`.
pub(crate) fn impact(app: AppState, id: Id) -> AnyElement {
    let task = {
        let id = id.clone();
        computed(move || id.clone())
    };
    // This task's map level; another task's state is let go on its first action here.
    let trail = computed(move || {
        app.impact.with(|state| {
            if state.task.as_deref() == Some(task.get().as_str()) {
                state.map.trail.clone()
            } else {
                Vec::new()
            }
        })
    });
    let shown = computed(move || {
        let more = app.impact.with(|state| state.more.clone());
        let trail = trail.get();
        app.feed
            .with(|feed| impact::shown(feed, task.get().as_str(), &trail, &more))
    });
    let ready = move || match shown.get() {
        Shown::Ready(impact) => Some(impact),
        _ => None,
    };
    let listing: Memo<Listing> = computed(move || {
        ready().map(|impact| (Id::from(impact.project), impact.touched, impact.sections))
    });
    let head = computed(move || ready().map(|impact| (impact.summary, impact.notices)));
    let level = computed(move || ready().map(|impact| (impact.crumbs, impact.map_said)));
    let scene: Memo<Option<Scene>> = computed(move || ready().and_then(|impact| impact.scene));
    let marks = computed(move || ready().map(|impact| impact.marks).unwrap_or_default());
    keep(app, task, trail, listing);
    let said = computed(move || match shown.get() {
        Shown::Loading => Some(READING.to_owned()),
        Shown::Said(words) => Some(words),
        Shown::Ready(_) => None,
    });
    let waiting = computed(move || said.get().is_some());
    let body = dynamic(move || {
        if waiting.get() {
            return untracked(|| {
                dynamic(move || quiet(app, said.get().unwrap_or_default())).into_element()
            });
        }
        untracked(|| drawn(app, task, head, level, scene, marks, listing))
    });
    view().test_id("task-impact").child(body).into_element()
}

/// Keeps the impact read at the project level and at the level shown, and, for the selected
/// item, Activity › System's maps along its target, which say whether that page has it.
fn keep(
    app: AppState,
    task: Memo<Id>,
    trail: Memo<Vec<corral_types::SystemAt>>,
    listing: Memo<Listing>,
) {
    let targets = computed(move || {
        let task = task.get();
        let trail = trail.get();
        let mut targets: Vec<DetailRef> = vec![impact::request(task.as_str(), None)];
        if let Some(at) = trail.last() {
            targets.push(impact::request(task.as_str(), Some(at)));
        }
        let item = app.impact.with(|state| {
            (state.task.as_deref() == Some(task.as_str()))
                .then(|| state.item.clone())
                .flatten()
        });
        listing.with(|listing| {
            let Some((project, touched, sections)) = listing else {
                return;
            };
            let selected = touched
                .iter()
                .chain(sections.iter().flat_map(|s| s.items.iter()))
                .find(|i| Some(&i.key) == item.as_ref());
            if let Some(target) = selected.and_then(|i| i.target.as_ref()) {
                targets.extend(impact::wanted(project.as_str(), target));
            }
            // A touched lens is drawn as checked at the commit the item names.
            if let Some(touched) = selected.and_then(|i| i.lens.as_ref()) {
                for at in [&touched.head, &touched.base] {
                    targets.push(lens::lens_request(
                        project.as_str(),
                        &touched.file,
                        Some(at),
                    ));
                }
            }
        });
        targets
    });
    super::keep::keep_details(app, targets, None);
}

fn drawn(
    app: AppState,
    task: Memo<Id>,
    head: Memo<Option<(String, Vec<String>)>>,
    level: Memo<Option<(Vec<String>, Option<String>)>>,
    scene: Memo<Option<Scene>>,
    marks: Memo<Marks>,
    listing: Memo<Listing>,
) -> AnyElement {
    let form = app.form;
    let host = Host::Impact(marks, task);
    let map = match form {
        Form::Desktop => canvas::map(app, scene, host),
        Form::Phone | Form::Terminal => tree::tree(app, scene, host),
    };
    let head = dynamic(move || {
        let (summary, notices) = head.get()?;
        Some(untracked(|| {
            view()
                .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                .child(plain(app, summary))
                .children(
                    notices
                        .into_iter()
                        .map(|n| hued(app, n, Hue::Held).into_element()),
                )
                .into_element()
        }))
    });
    let crumbs = dynamic(move || {
        let (crumbs, said) = level.get()?;
        Some(untracked(|| {
            let last = crumbs.len().saturating_sub(1);
            view()
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
                        .children(crumbs.into_iter().enumerate().map(|(n, crumb)| {
                            let kind = if n == last {
                                ButtonKind::Ghost
                            } else {
                                ButtonKind::Quiet
                            };
                            button(app, crumb, kind, format!("impact-crumb-{n}"), move || {
                                let task = untracked(|| task.get());
                                app.impact_act(task.as_str(), Action::Map(MapAction::To(n)));
                            })
                        })),
                )
                .children(said.map(|said| quiet(app, said)))
                .into_element()
        }))
    });
    let list = dynamic(move || {
        let (project, touched, sections) = listing.get()?;
        let at = At {
            task: untracked(|| task.get()),
            project,
        };
        Some(untracked(|| {
            let touched = if touched.is_empty() {
                quiet(app, NO_MODULE)
            } else {
                items(app, &at, touched)
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
                            sections
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
        .child(crumbs)
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
    let (key, target) = (item.key.clone(), item.target.clone());
    let press_task = task.clone();
    let line = list_row(
        app,
        label,
        format!("impact-item-{}", item.key),
        move || selected.get(),
        move || {
            app.impact_act(
                press_task.as_str(),
                Action::Item {
                    key: key.clone(),
                    target: target.clone(),
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
        let (sub, anchor, note) = (item.sub.clone(), item.anchor.clone(), item.note.clone());
        let drawn = item.lens.clone().map(|touched| {
            let head = lens::lens_request(project.as_str(), &touched.file, Some(&touched.head));
            let base = lens::lens_request(project.as_str(), &touched.file, Some(&touched.base));
            let touch = Touch {
                touched: touched.touched,
                added: touched.added,
                removed: touched.removed,
            };
            untracked(|| lenses::touched_lens(app, head, base, touch))
        });
        // A lens has no node on the map; it is drawn here instead.
        let to_system = if item.lens.is_some() {
            view().into_element()
        } else {
            to_system(app, project.clone(), item.target.clone())
        };
        Some(untracked(|| {
            view()
                .when(form == Form::Terminal, |v| v.pl(term::cells(3.0)))
                .when(form != Form::Terminal, |v| {
                    v.pl(px(28.0)).py(px(4.0)).gap(px(4.0)).items_start()
                })
                .children(sub.map(|sub| quiet(app, sub)))
                .child(plain(
                    app,
                    anchor.map_or_else(|| NO_ANCHOR.to_owned(), |a| a.words()),
                ))
                .children(note.map(|note| quiet(app, note)))
                .children(drawn)
                .child(to_system)
                .into_element()
        }))
    });
    view().child(line).child(detail).into_element()
}

/// "Open in System" where Activity › System's own map has the item's node (or the level that
/// holds it, said so), and why not where it has none of it.
fn to_system(app: AppState, project: Id, target: Option<impact::Target>) -> AnyElement {
    let reach = {
        let project = project.clone();
        computed(move || {
            let target = target.as_ref()?;
            Some(
                app.feed
                    .with(|feed| impact::reach(feed, project.as_str(), target)),
            )
        })
    };
    dynamic(move || {
        let reach = reach.get();
        let project = project.clone();
        untracked(move || match reach {
            Some(Reach::Open {
                trail,
                node,
                label,
                note,
            }) => view()
                .gap(px(4.0))
                .children(note.map(|note| quiet(app, note)))
                .child(button(
                    app,
                    label,
                    ButtonKind::Ghost,
                    "btn-open-in-system",
                    move || app.open_in_system(project.clone(), trail.clone(), node.clone()),
                ))
                .into_element(),
            Some(Reach::Checking) => quiet(app, CHECKING),
            Some(Reach::Unavailable(why)) => view()
                .test_id("impact-unreachable")
                .child(quiet(app, why))
                .into_element(),
            None => view()
                .test_id("impact-unreachable")
                .child(quiet(app, impact::NO_NODE))
                .into_element(),
        })
    })
    .into_element()
}
