//! The Changes tab's Impact view: the task's `system_review` as Activity › System's map at the
//! task's head (`screens::system::Host::Impact`: touched cards ringed solid with their lines,
//! cards holding dependents ringed dashed, the rest dimmed), drilled like Activity › System.
//! On a wide window the list of what changed (`impact_list.rs`) is a column beside the map's
//! stage, each scrolling on its own: the breadcrumb, Escape hint and summary over the map, the
//! legend and provenance under it, and the inspector (`impact_inspect.rs`) beside it while a
//! card or edge is picked. Narrower, on the phone and in a terminal they stack, and an item
//! that moves the map brings it into view. Every word is `model::review::impact`'s.
//!
//! Test ids: "task-impact", "impact-list", "impact-crumb-<n>", "impact-summary",
//! "impact-provenance", "inspector", "system-legend", and those of `impact_list.rs` and
//! `impact_inspect.rs`.

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use corral_types::SystemAt;

use crate::model::review::impact::{self, Action, LEGEND_LISTED, READING, Shown, foot};
use crate::model::system::Action as MapAction;
use crate::model::system::lens;
use crate::model::system::marks::Marks;
use crate::model::system::scene::Scene;
use crate::model::{Form, Hue, Id};
use crate::screens::system::{Host, canvas, legend_of, tree};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::plain;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

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
        let trail = trail.get();
        app.feed
            .with(|feed| impact::shown(feed, task.get().as_str(), &trail))
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
        untracked(|| {
            drawn(
                app,
                Parts {
                    task,
                    trail,
                    head,
                    level,
                    scene,
                    marks,
                    listing,
                },
            )
        })
    });
    view()
        .test_id("task-impact")
        .when(app.form == Form::Desktop, |v| {
            v.flex_1().min_h(px(0.0)).flex_col()
        })
        .child(body)
        .into_element()
}

/// Keeps the impact read at the project level and at the level shown, and, for the selected
/// item, Activity › System's maps along its target, which say whether that page has it.
fn keep(app: AppState, task: Memo<Id>, trail: Memo<Vec<SystemAt>>, listing: Memo<Listing>) {
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
                .chain(
                    sections
                        .iter()
                        .flat_map(|s| s.groups.iter().flat_map(|g| g.items.iter())),
                )
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

/// What the drawn view reads, each tracked on its own.
#[derive(Clone, Copy)]
struct Parts {
    task: Memo<Id>,
    trail: Memo<Vec<SystemAt>>,
    head: Memo<Option<(String, Vec<String>)>>,
    level: Memo<Option<(Vec<String>, Option<String>)>>,
    scene: Memo<Option<Scene>>,
    marks: Memo<Marks>,
    listing: Memo<Listing>,
}

/// Content width from which the list, the map and the inspector stand side by side.
const SIDE_BY_SIDE: f32 = 1100.0;
/// The list's column beside the map, as the mockup draws it.
const LIST_WIDTH: f32 = 390.0;
const INSPECTOR_WIDTH: f32 = 320.0;

/// The list beside the map on a wide window, each scrolling on its own, the map's breadcrumb
/// and summary over it and its legend and provenance under it; stacked when narrower, on the
/// phone and in a terminal, where selecting an item brings the map into view.
fn drawn(app: AppState, parts: Parts) -> AnyElement {
    let form = app.form;
    let width = signal(0.0_f32);
    let wide = computed(move || form == Form::Desktop && width.get() >= SIDE_BY_SIDE);
    let body = dynamic(move || {
        if wide.get() {
            untracked(|| beside(app, parts))
        } else {
            untracked(|| stacked(app, parts))
        }
    });
    view()
        .when(form == Form::Desktop, |v| {
            v.flex_1()
                .min_h(px(0.0))
                .flex_col()
                .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        })
        .child(body)
        .into_element()
}

fn beside(app: AppState, parts: Parts) -> AnyElement {
    let map_ref = element_ref();
    let list = super::impact_list::list(app, parts.task, parts.listing, map_ref);
    let picked = picked(app, parts.task);
    let inspector = dynamic(move || {
        picked.get().then(|| {
            untracked(|| {
                scroll()
                    .test_id("inspector")
                    .flex_none()
                    .w(px(INSPECTOR_WIDTH))
                    .min_h(px(0.0))
                    .border_l(px(1.0))
                    .border_color(color(Token::Rule))
                    .bg(color(Token::BgSunk))
                    .child(
                        view()
                            .px(px(14.0))
                            .py(px(12.0))
                            .child(inspector(app, parts)),
                    )
                    .into_element()
            })
        })
    });
    let stage = view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex_col()
        .child(
            view()
                .flex_none()
                .px(px(14.0))
                .py(px(8.0))
                .border_b(px(1.0))
                .border_color(color(Token::Rule))
                .child(stage_head(app, parts)),
        )
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .element_ref(&map_ref)
                    .px(px(14.0))
                    .py(px(12.0))
                    .child(map(app, parts)),
            ),
        )
        .child(
            view()
                .flex_none()
                .px(px(14.0))
                .py(px(8.0))
                .border_t(px(1.0))
                .border_color(color(Token::Rule))
                .child(foot(app, parts)),
        );
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_row()
        .border_t(px(1.0))
        .border_color(color(Token::Rule))
        .child(
            scroll()
                .test_id("impact-list")
                .flex_none()
                .w(px(LIST_WIDTH))
                .min_h(px(0.0))
                .border_r(px(1.0))
                .border_color(color(Token::Rule))
                .child(view().pr(px(14.0)).py(px(8.0)).child(list)),
        )
        .child(stage)
        .child(inspector)
        .into_element()
}

fn stacked(app: AppState, parts: Parts) -> AnyElement {
    let form = app.form;
    let map_ref = element_ref();
    let list = super::impact_list::list(app, parts.task, parts.listing, map_ref);
    let picked = picked(app, parts.task);
    let inspector = dynamic(move || {
        picked.get().then(|| {
            untracked(|| {
                view()
                    .test_id("inspector")
                    .when(form != Form::Terminal, |v| {
                        v.p(px(12.0)).rounded(px(8.0)).bg(color(Token::BgSunk))
                    })
                    .child(inspector(app, parts))
                    .into_element()
            })
        })
    });
    let content = view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .child(stage_head(app, parts))
        .child(view().element_ref(&map_ref).child(map(app, parts)))
        .child(foot(app, parts))
        .child(inspector)
        .when(form == Form::Terminal, |v| v.child(term::blank()))
        .child(list);
    match form {
        // The page does not scroll Impact on a desktop; a narrow window scrolls it here.
        Form::Desktop => scroll()
            .flex_1()
            .min_h(px(0.0))
            .child(content.py(px(10.0)))
            .into_element(),
        Form::Phone | Form::Terminal => content.into_element(),
    }
}

/// Whether this task's map has a pick.
fn picked(app: AppState, task: Memo<Id>) -> Memo<bool> {
    computed(move || {
        app.impact.with(|state| {
            state.task.as_deref() == Some(task.get().as_str()) && state.map.picked.is_some()
        })
    })
}

fn map(app: AppState, parts: Parts) -> AnyElement {
    let host = Host::Impact(parts.marks, parts.task, parts.scene);
    match app.form {
        Form::Desktop => canvas::map(app, parts.scene, host),
        Form::Phone | Form::Terminal => tree::tree(app, parts.scene, host),
    }
}

/// The breadcrumb, each level named with its kind, the Escape hint and the summary, then the
/// notices and what stands in the map's place when it cannot be drawn.
fn stage_head(app: AppState, parts: Parts) -> AnyElement {
    let form = app.form;
    let Parts {
        task,
        trail,
        head,
        level,
        ..
    } = parts;
    let crumbs = dynamic(move || {
        let (crumbs, _) = level.get()?;
        let trail = trail.get();
        Some(untracked(|| {
            let last = crumbs.len().saturating_sub(1);
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
                .role(AccessibilityRole::Navigation)
                .label("Map level")
                .children(crumbs.into_iter().enumerate().flat_map(|(n, name)| {
                    let kind = foot::crumb_kind(n.checked_sub(1).and_then(|i| trail.get(i)));
                    let sep = (n > 0).then(|| {
                        text(if form == Form::Terminal {
                            " › "
                        } else {
                            "›"
                        })
                        .text_color(color(Token::Ink3))
                        .into_element()
                    });
                    let crumb = if n == last {
                        view()
                            .flex_row()
                            .items_center()
                            .test_id(&format!("impact-crumb-{n}"))
                            .child(
                                text(name)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(color(Token::Ink))
                                    .when(form != Form::Terminal, |t| {
                                        t.font_family(FONT_MONO).text_size(px(12.5))
                                    }),
                            )
                            .child(quiet(app, format!(" {kind}")))
                            .into_element()
                    } else {
                        button(
                            app,
                            format!("{name} {kind}"),
                            ButtonKind::Quiet,
                            format!("impact-crumb-{n}"),
                            move || {
                                let task = untracked(|| task.get());
                                app.impact_act(task.as_str(), Action::Map(MapAction::To(n)));
                            },
                        )
                    };
                    sep.into_iter().chain(Some(crumb))
                }))
                .into_element()
        }))
    });
    let hint = dynamic(move || {
        let hint = app.impact_escape_hint()?;
        Some(untracked(|| quiet(app, format!("Esc {hint}"))))
    });
    let summary = dynamic(move || {
        let (summary, _) = head.get()?;
        Some(untracked(|| {
            text(summary)
                .test_id("impact-summary")
                .text_color(color(Token::Ink2))
                .when(form != Form::Terminal, |t| {
                    t.font_family(FONT_TEXT).text_size(px(12.5))
                })
                .into_element()
        }))
    });
    let notices = dynamic(move || {
        let (_, notices) = head.get()?;
        let said = level.get().and_then(|(_, said)| said);
        Some(untracked(|| {
            view()
                .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
                .children(
                    notices
                        .into_iter()
                        .map(|n| hued(app, n, Hue::Held).into_element()),
                )
                .children(said.map(|said| quiet(app, said)))
                .into_element()
        }))
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
                        .child(crumbs)
                        .child(hint),
                )
                .child(view().flex_1().min_w(px(0.0)))
                .child(summary),
        )
        .child(notices)
        .into_element()
}

/// The legend with a sample of each mark (in words where the map is a list), then the
/// provenance line.
fn foot(app: AppState, parts: Parts) -> AnyElement {
    let form = app.form;
    let Parts { task, trail, .. } = parts;
    let legend = dynamic(move || {
        let at = trail.get().last().cloned();
        untracked(|| match form {
            Form::Desktop => legend_of(app, foot::legend(at.as_ref())),
            Form::Phone | Form::Terminal => quiet(app, LEGEND_LISTED),
        })
    });
    let provenance = dynamic(move || {
        let words = app
            .feed
            .with(|feed| foot::provenance(feed, task.get().as_str()))?;
        Some(untracked(|| {
            text(words)
                .test_id("impact-provenance")
                .text_color(color(Token::Ink3))
                .when(form != Form::Terminal, |t| {
                    t.font_family(FONT_TEXT).text_size(px(12.0))
                })
                .into_element()
        }))
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(legend)
        .child(provenance)
        .into_element()
}

fn inspector(app: AppState, parts: Parts) -> AnyElement {
    super::impact_inspect::inspector(app, parts.task, parts.trail, parts.scene, parts.marks)
}

fn hued(app: AppState, words: String, hue: Hue) -> Text {
    plain(app, words).text_color(hue_ink(hue))
}
