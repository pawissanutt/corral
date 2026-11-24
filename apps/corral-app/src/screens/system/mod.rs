//! Activity › System: one project's map. The head carries the overlay (Structure, Activity,
//! Coverage) and, under Activity, its window; under it the breadcrumb, which goes up a level as
//! Escape does, and the filter; then the notices, the summary, the map with its legend and the
//! one provenance line, and the inspector beside it. Desktop and web draw the map on a canvas
//! (`canvas.rs`); the phone and a terminal list the same nodes with their edges (`tree.rs`).
//! With All projects in scope the page asks for one project. Every word, number and branch is
//! `model::system`'s.
//!
//! Test ids: "page-title", "seg-overlay-<n>", "seg-window-<n>", "crumb-<n>", "field-system",
//! "system-pick-<project>", "system-map", "system-provenance", "inspector", "btn-system-open",
//! "inspect-row-<n>", and those of `canvas.rs` and `tree.rs`.

pub(crate) mod canvas;
pub(crate) mod lens_canvas;
mod lens_draft;
pub(crate) mod lens_impact;
pub(crate) mod lenses;
pub(crate) mod tree;

use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use crate::model::list::inspector_width;
use crate::model::projects::Scope;
use crate::model::review::impact;
use crate::model::system::inspect::Inspector;
use crate::model::system::marks::Marks;
use crate::model::system::scene::EdgeKind;
use crate::model::system::shown::{ASK_HEADING, ASK_WORDS, Shown, Status, TITLE, shown};
use crate::model::system::words::{Swatch, slot_hue};
use crate::model::system::{Action, Overlay, Pick, State, View, Window, view_request};
use crate::model::text::span;
use crate::model::{Form, Hue, Id};
use crate::screens::activity::subnav;
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::kit::button::{ButtonKind, button, seg};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice::{empty, notice};
use crate::ui::kit::page::{Width, body, columns, context, head, pane};
use crate::ui::kit::rows::list_row;
use crate::ui::kit::text::{h3, label_value, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

/// Whose state a map draws and picks: Activity › System's, or a task page's Impact, which
/// marks what its change touched.
#[derive(Clone, Copy)]
pub(crate) enum Host {
    System,
    /// The marks, and the task whose Impact this is.
    Impact(Memo<Marks>, Memo<Id>),
}

impl Host {
    /// Tracked.
    fn state(self, app: AppState) -> State {
        match self {
            Host::System => app.system.get(),
            Host::Impact(_, task) => app.impact.with(|state| {
                if state.task.as_deref() == Some(task.get().as_str()) {
                    state.map.clone()
                } else {
                    State::default()
                }
            }),
        }
    }

    /// Tracked.
    fn marks(self) -> Option<Marks> {
        match self {
            Host::System => None,
            Host::Impact(marks, _) => Some(marks.get()),
        }
    }

    fn pick(self, app: AppState, pick: Pick) {
        match self {
            Host::System => app.system_act(Action::Pick(pick)),
            Host::Impact(_, task) => {
                let task = untracked(|| task.get());
                app.impact_act(task.as_str(), impact::Action::Map(Action::Pick(pick)));
            }
        }
    }
}

/// Activity › System.
pub fn screen(app: AppState) -> AnyElement {
    let data = computed(move || {
        let scope = app.project_scope();
        let state = app.system.get();
        app.feed.with(|feed| shown(&state, feed, &scope))
    });
    keep_loaded(app, data);
    let asking = computed(move || data.with(|shown| shown.project.is_none()));
    let lensing = computed(move || {
        app.system
            .with(|s| s.view == View::Lenses)
            .then(|| data.with(|shown| shown.project.clone()))
            .flatten()
    });
    let head = page_head(app, data);
    let content = dynamic(move || {
        if asking.get() {
            untracked(|| picker(app, data))
        } else if let Some(project) = lensing.get() {
            untracked(|| lenses::page(app, project))
        } else {
            untracked(|| map_page(app, data))
        }
    });
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(head)
        .children(subnav(app, None))
        .child(content)
        .into_element()
}

/// Follows the scope's project and keeps the shown level's map requested.
fn keep_loaded(app: AppState, data: Memo<Shown>) {
    let project = computed(move || data.with(|shown| shown.project.clone()));
    effect(move || {
        if let Some(project) = project.get() {
            untracked(|| app.system.update(|state| state.follow(&project)));
        }
    });
    let wanted = computed(move || {
        let project = project.get()?;
        // A trail kept from another project names a level this one does not have.
        let at = app.system.with(|state| {
            (state.project.as_deref() == Some(project.as_str()))
                .then(|| state.at().cloned())
                .flatten()
        });
        let target = app
            .system
            .with(|state| view_request(state, &project, at.as_ref()));
        app.daemon().then_some(target)
    });
    effect(move || {
        if let Some(target) = wanted.get() {
            untracked(|| app.load(target));
        }
    });
}

fn page_head(app: AppState, data: Memo<Shown>) -> AnyElement {
    let overlays: Vec<(Overlay, &'static str)> =
        Overlay::ALL.iter().map(|o| (*o, o.label())).collect();
    let windows: Vec<(Window, &'static str)> =
        Window::ALL.iter().map(|w| (*w, w.label())).collect();
    let views: Vec<(View, &'static str)> = View::ALL.iter().map(|v| (*v, v.label())).collect();
    // Asking for a project, there is no map to re-colour.
    let drawing = computed(move || data.with(|shown| shown.project.is_some()));
    let structure =
        computed(move || drawing.get() && app.system.with(|s| s.view == View::Structure));
    let view_seg = dynamic(move || {
        let views = views.clone();
        drawing.get().then(|| {
            untracked(|| {
                seg(
                    app,
                    &views,
                    move || app.system.with(|s| s.view),
                    move |v| app.system_act(Action::View(v)),
                    "seg-view",
                )
            })
        })
    })
    .into_element();
    let overlay = dynamic(move || {
        let overlays = overlays.clone();
        structure.get().then(|| {
            untracked(|| {
                seg(
                    app,
                    &overlays,
                    move || app.system.with(|s| s.overlay),
                    move |o| app.system_act(Action::Overlay(o)),
                    "seg-overlay",
                )
            })
        })
    })
    .into_element();
    let activity =
        computed(move || structure.get() && app.system.with(|s| s.overlay == Overlay::Activity));
    let window = dynamic(move || {
        activity.get().then(|| {
            untracked(|| {
                seg(
                    app,
                    &windows,
                    move || app.system.with(|s| s.window),
                    move |w| app.system_act(Action::Window(w)),
                    "seg-window",
                )
            })
        })
    })
    .into_element();
    // On the phone the switches take the width under the head, one under the other, rather
    // than squeeze its title; the head keeps the connection banner.
    if app.form == Form::Phone {
        return view()
            .flex_none()
            .flex_col()
            .child(head(app, TITLE, None, Vec::new()))
            .child(
                view()
                    .flex_col()
                    .gap(px(8.0))
                    .px(px(16.0))
                    .py(px(8.0))
                    .child(view_seg)
                    .child(overlay)
                    .child(window),
            )
            .into_element();
    }
    head(app, TITLE, None, vec![view_seg, overlay, window])
}

/// All projects in scope: the page asks for one.
fn picker(app: AppState, data: Memo<Shown>) -> AnyElement {
    // The projects can arrive, come and go while the page asks; the choices follow them.
    let projects = computed(move || data.with(|shown| shown.projects.clone()));
    let choices = dynamic(move || {
        let projects = projects.get();
        untracked(|| {
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(8.0))
                .children(projects.into_iter().map(|id| {
                    let pick = id.clone();
                    button(
                        app,
                        id.to_string(),
                        ButtonKind::Ghost,
                        format!("system-pick-{id}"),
                        move || app.set_scope(Scope::Project(pick.clone())),
                    )
                }))
                .into_element()
        })
    })
    .into_element();
    body(
        app,
        "system-map",
        empty(app, ASK_HEADING, ASK_WORDS, Some(choices)),
    )
}

fn map_page(app: AppState, data: Memo<Shown>) -> AnyElement {
    let form = app.form;
    let scene = computed(move || data.with(|shown| shown.scene.clone()));
    let drawn = match form {
        Form::Desktop => canvas::map(app, scene, Host::System),
        Form::Phone | Form::Terminal => tree::tree(app, scene, Host::System),
    };
    let main = view()
        .flex_col()
        .min_w(px(0.0))
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .when(form != Form::Terminal, |v| v.px(px(16.0)).py(px(12.0)))
        .child(notices(app, data))
        .child(summary(app, data))
        .child(drawn)
        .child(legend(app, data))
        .child(provenance(app, data))
        .into_element();
    let side = context(app, "inspector", inspector(app, data));
    let page = match form {
        Form::Phone => pane(
            app,
            "system-map",
            view().child(main).child(side).into_element(),
        ),
        Form::Desktop | Form::Terminal => {
            let terminal = form == Form::Terminal;
            let room = if terminal {
                term::page_columns(app)
            } else {
                1200.0
            };
            let side_w = inspector_width(room, terminal) * if terminal { 12.0 } else { 1.0 };
            columns(
                app,
                vec![
                    (Width::Fill, body_or_pane(app, main)),
                    (Width::Fixed(side_w), side),
                ],
                0,
            )
        }
    };
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(bar(app, data))
        .when(form == Form::Terminal, |v| v.child(term::hrule()))
        .child(page)
        .into_element()
}

fn body_or_pane(app: AppState, content: AnyElement) -> AnyElement {
    match app.form {
        Form::Terminal => body(app, "system-map", content),
        _ => pane(app, "system-map", content),
    }
}

/// The breadcrumb and the filter.
fn bar(app: AppState, data: Memo<Shown>) -> AnyElement {
    let form = app.form;
    let crumbs = computed(move || data.with(|shown| shown.crumbs.clone()));
    let trail = dynamic(move || {
        let crumbs = crumbs.get();
        untracked(|| {
            let last = crumbs.len().saturating_sub(1);
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(4.0))
                .role(AccessibilityRole::Navigation)
                .label("Map level")
                .children(crumbs.into_iter().enumerate().flat_map(|(i, words)| {
                    let sep =
                        (i > 0).then(|| text("›").text_color(color(Token::Ink3)).into_element());
                    let crumb = if i == last {
                        text(words)
                            .test_id(&format!("crumb-{i}"))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color(Token::Ink))
                            .when(form != Form::Terminal, |t| {
                                t.font_family(FONT_MONO).text_size(px(12.5))
                            })
                            .into_element()
                    } else {
                        button(
                            app,
                            words,
                            ButtonKind::Quiet,
                            format!("crumb-{i}"),
                            move || app.system_act(Action::To(i)),
                        )
                    };
                    sep.into_iter().chain(Some(crumb))
                }))
                .into_element()
        })
    });
    let count = text(move || {
        data.with(|shown| {
            shown
                .matched
                .map(|(n, of)| format!("{n} of {of} here"))
                .unwrap_or_default()
        })
    })
    .number_of_lines(1u32)
    .text_color(color(Token::Ink3));
    let entry = copyable_input(app, None, move || app.system.with(|s| s.filter.clone()))
        .test_id("field-system")
        .label("Filter the map")
        .placeholder("Filter by name or path")
        .on_change(move |typed: &str| app.system_act(Action::Filter(typed.to_owned())))
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .bg(AdaptiveColor::TRANSPARENT);
    let filter = match form {
        Form::Terminal => view()
            .flex_row()
            .w(term::cells(36.0))
            .child(text("/ ").text_color(color(Token::Ink3)))
            .child(entry.h(term::rows(1.0)))
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .flex_row()
            .items_center()
            .w(px(if form == Form::Phone { 200.0 } else { 260.0 }))
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
        .flex_none()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(10.0))
        .when(form == Form::Terminal, |v| v.px(term::cells(1.0)))
        .when(form != Form::Terminal, |v| {
            v.px(px(16.0))
                .py(px(8.0))
                .border_b(px(1.0))
                .border_color(color(Token::Rule))
        })
        .child(view().flex_1().min_w(px(0.0)).child(trail))
        .child(filter)
        .child(count)
        .into_element()
}

/// Where the read stands, then the served notices: building, failed, an extension's error.
fn notices(app: AppState, data: Memo<Shown>) -> AnyElement {
    let said = computed(move || {
        data.with(|shown| {
            let mut out: Vec<(Hue, String)> = Vec::new();
            match &shown.status {
                Status::Loading if shown.scene.is_none() => {
                    out.push((Hue::Neutral, "Loading the map from corrald…".into()))
                }
                Status::Stale => out.push((
                    Hue::Held,
                    "Shown as last read; corrald has not confirmed it since.".into(),
                )),
                Status::Failed(why) => {
                    out.push((Hue::Fail, format!("The map could not be read: {why}")))
                }
                _ => {}
            }
            out.extend(shown.notices.iter().map(|n| (n.hue, n.words.clone())));
            if let Some(words) = shown.empty {
                out.push((Hue::Neutral, words.into()));
            }
            out
        })
    });
    dynamic(move || {
        let said = said.get();
        untracked(|| {
            view()
                .test_id("system-notices")
                .gap(px(6.0))
                .children(
                    said.into_iter()
                        .map(|(hue, words)| notice(app, hue, &vec![span(words)], None)),
                )
                .into_element()
        })
    })
    .into_element()
}

fn quiet(app: AppState, words: impl Into<SharedString>) -> Text {
    let t = text(words.into()).text_color(color(Token::Ink3));
    if app.form == Form::Terminal {
        t
    } else {
        t.font_family(FONT_TEXT).text_size(px(12.0))
    }
}

fn summary(app: AppState, data: Memo<Shown>) -> AnyElement {
    text(move || data.with(|shown| shown.summary.join("  ·  ")))
        .test_id("system-summary")
        .text_color(color(Token::Ink2))
        .when(app.form != Form::Terminal, |t| {
            t.font_family(FONT_TEXT).text_size(px(12.5))
        })
        .into_element()
}

fn provenance(app: AppState, data: Memo<Shown>) -> AnyElement {
    let words = text(move || data.with(|shown| shown.provenance.clone()))
        .test_id("system-provenance")
        .text_color(color(Token::Ink3));
    if app.form == Form::Terminal {
        words.into_element()
    } else {
        words
            .font_family(FONT_TEXT)
            .text_size(px(12.0))
            .into_element()
    }
}

/// A legend entry's sample: a coloured box, a line drawn as the map draws that edge kind.
fn swatch(app: AppState, swatch: Swatch) -> AnyElement {
    let (glyph, ink, dashed) = match swatch {
        Swatch::Language(slot) => ("■", hue_ink(slot_hue(slot)), false),
        Swatch::OtherLanguage => ("■", color(Token::Ink4), false),
        Swatch::Activity => ("■", hue_ink(Hue::Worker), false),
        Swatch::Read => ("■", color(Token::Ink2), false),
        Swatch::Unsupported => ("■", color(Token::Ink4), false),
        Swatch::Failed => ("■", hue_ink(Hue::Fail), false),
        Swatch::Edge(EdgeKind::DeclaredOnly) => ("┄", color(Token::Ink4), true),
        Swatch::Edge(EdgeKind::Boundary) => ("┄", color(Token::Ink3), true),
        Swatch::Edge(EdgeKind::ReferencedOnly) => ("━", hue_ink(Hue::Held), false),
        Swatch::Edge(EdgeKind::Starts) => ("━", hue_ink(Hue::Worker), false),
        Swatch::Edge(EdgeKind::DependsOn) => ("┄", color(Token::Ink3), true),
        Swatch::Edge(EdgeKind::Builds) => ("┄", hue_ink(Hue::Held), true),
        Swatch::Edge(_) => ("━", color(Token::Ink3), false),
        Swatch::Excluded => ("┆", color(Token::Ink4), true),
        Swatch::Folder => ("┆", color(Token::Rule2), true),
        Swatch::Spawn => ("◖", color(Token::Ink3), false),
    };
    if app.form == Form::Terminal {
        return text(glyph).text_color(ink).into_element();
    }
    let line = matches!(swatch, Swatch::Edge(_));
    let sample = view().flex_shrink_0().accessibility_elements_hidden(true);
    if line {
        sample
            .w(px(18.0))
            .h(px(0.0))
            .border_t(px(2.0))
            .border_color(ink)
            .when(dashed, |v| v.border_dashed())
            .into_element()
    } else {
        sample
            .w(px(10.0))
            .h(px(10.0))
            .rounded(px(2.0))
            .when(dashed, |v| v.border_1().border_dashed().border_color(ink))
            .when(!dashed, |v| v.bg(ink))
            .into_element()
    }
}

fn legend(app: AppState, data: Memo<Shown>) -> AnyElement {
    let items = computed(move || data.with(|shown| shown.legend.clone()));
    dynamic(move || {
        let items = items.get();
        untracked(|| {
            view()
                .test_id("system-legend")
                .flex_row()
                .flex_wrap()
                .gap(px(12.0))
                .children(items.into_iter().map(|(s, words)| {
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(if app.form == Form::Terminal { 8.0 } else { 6.0 }))
                        .child(swatch(app, s))
                        .child(quiet(app, words))
                        .into_element()
                }))
                .into_element()
        })
    })
    .into_element()
}

fn inspector(app: AppState, data: Memo<Shown>) -> AnyElement {
    let shown = computed(move || data.with(|shown| (shown.inspector.clone(), shown.hint)));
    dynamic(move || {
        let (inspector, hint) = shown.get();
        untracked(|| match inspector {
            None => quiet(app, hint).into_element(),
            Some(inspector) => inspected(app, inspector),
        })
    })
    .into_element()
}

fn inspected(app: AppState, inspector: Inspector) -> AnyElement {
    let form = app.form;
    let title = text(inspector.title.clone())
        .test_id("inspector-title")
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink))
        .when(form != Form::Terminal, |t| {
            t.font_family(FONT_MONO).text_size(px(15.0))
        });
    let mut n = 0usize;
    let sections: Vec<AnyElement> = inspector
        .sections
        .into_iter()
        .map(|section| {
            let rows: Vec<AnyElement> = section
                .rows
                .into_iter()
                .map(|row| {
                    n += 1;
                    let value = plain(app, row.value.clone()).into_element();
                    match row.pick {
                        Some(pick) => list_row(
                            app,
                            format!("{} {}", row.label, row.value),
                            format!("inspect-row-{n}"),
                            || false,
                            move || app.system_act(Action::Pick(pick.clone())),
                            vec![
                                row.swatch
                                    .map(|kind| swatch(app, Swatch::Edge(kind)))
                                    .unwrap_or_else(|| view().into_element()),
                                // The name stays whole: on its one terminal line, with the
                                // words after it giving way; elsewhere it wraps.
                                view()
                                    .when(app.form == Form::Terminal, |v| v.flex_shrink_0())
                                    .when(app.form != Form::Terminal, |v| v.min_w(px(0.0)))
                                    .child(
                                        plain(app, row.label)
                                            .when(app.form == Form::Terminal, |t| {
                                                t.number_of_lines(1u32)
                                            }),
                                    )
                                    .into_element(),
                                view()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .child(quiet(app, row.value).number_of_lines(1u32))
                                    .into_element(),
                            ],
                        ),
                        None => label_value(app, row.label, value),
                    }
                })
                .collect();
            view()
                .gap(px(5.0))
                .children(section.title.map(|t| h3(app, t, None)))
                .children(section.note.map(|note| plain(app, note).into_element()))
                .children(rows)
                .into_element()
        })
        .collect();
    let open = inspector.open.map(|(words, node)| {
        button(
            app,
            words,
            ButtonKind::Primary,
            "btn-system-open",
            move || app.system_act(Action::Open(node.clone())),
        )
    });
    view()
        .test_id("inspector-body")
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .child(quiet(app, inspector.kind))
        .child(title)
        .children(sections)
        .children(open)
        .into_element()
}
