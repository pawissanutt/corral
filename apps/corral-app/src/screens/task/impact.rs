//! The Changes tab's Impact view: the task's `system_review` as Activity › System's map at the
//! task's head (`screens::system::Host::Impact`: touched nodes ringed with their lines, nodes
//! holding dependents ringed apart, the rest dimmed), drilled like Activity › System with a
//! breadcrumb, then the touched modules and the Model impact list. Selecting an item opens its
//! node's level, selects the node and shows its source anchor; "Open in System" opens Activity
//! › System where its own map has the node, or says why it cannot. Every word is
//! `model::review::impact`'s.
//!
//! Test ids: "task-impact", "impact-crumb-<n>", "impact-item-<key>", "btn-open-in-system",
//! "impact-unreachable", "btn-impact-more-<n>", "impact-lens" (a touched lens, drawn under its item),
//! "impact-source" (a data model change's committed lines, or why there are none).

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::review::impact::{self, Action, LEGEND_DRAWN, LEGEND_LISTED, READING, Shown};
use crate::model::system::Action as MapAction;
use crate::model::system::lens;
use crate::model::system::marks::Marks;
use crate::model::system::scene::Scene;
use crate::model::{Form, Hue, Id};
use crate::screens::system::{Host, canvas, tree};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::plain;
use crate::ui::term;

use super::impact_list::Listing;
use super::parts::quiet;

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
    let host = Host::Impact(marks, task, scene);
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
    let map_ref = element_ref();
    let list = super::impact_list::list(app, task, listing, map_ref);
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .child(head)
        .child(crumbs)
        .child(view().element_ref(&map_ref).child(map))
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
