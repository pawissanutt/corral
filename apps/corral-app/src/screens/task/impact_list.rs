//! Impact's list: the touched modules, then the Model impact categories with their items.
//! Selecting an item opens its node's level on the map beside it, selects the node and shows
//! its source anchor; "Open in System" opens Insight › Structure where its own map has the node,
//! or says why it cannot. Every word is `model::review::impact`'s.
//!
//! Test ids: "impact-item-<key>", "btn-open-in-system", "impact-unreachable",
//! "impact-cat-<n>", "btn-impact-more-<n>-<group>", "btn-impact-focus", "impact-lens",
//! "impact-source".

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::review::impact::{
    self, Action, CHECKING, FOCUS_ON_MAP, Group, HEADING, Item, LEAD, LIMIT, NO_ANCHOR, NO_MODULE,
    Reach, Section, Sign, TOUCHED,
};
use crate::model::system::count;
use crate::model::system::lens::{self, Touch};
use crate::model::{Form, Hue, Id};
use crate::screens::system::{lens_impact, lenses};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::rows::list_row_wrap;
use crate::ui::kit::text::{dim, mono, plain};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::parts::{block, quiet};

/// What the list shows: it does not change with the map's level or pick, so selecting keeps
/// the keyboard's place in it.
pub(super) type Listing = Option<(Id, Vec<Item>, Vec<Section>)>;

/// The task whose Impact this is, and its project.
#[derive(Clone)]
struct At {
    task: Id,
    project: Id,
    /// The map beside or above the list, brought into view when an item moves it.
    map: ElementRef,
}

/// The touched modules and the Model impact list of the task `task`.
pub(super) fn list(
    app: AppState,
    task: Memo<Id>,
    listing: Memo<Listing>,
    map: ElementRef,
) -> AnyElement {
    dynamic(move || {
        let (project, touched, sections) = listing.get()?;
        let at = At {
            task: untracked(|| task.get()),
            project,
            map,
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
    })
    .into_element()
}

fn category(app: AppState, at: &At, section: Section) -> AnyElement {
    let section = Rc::new(section);
    let open = {
        let (section, task) = (section.clone(), at.task.clone());
        computed(move || {
            app.impact.with(|state| {
                let state = (state.task.as_deref() == Some(task.as_str())).then_some(state);
                section.open(state)
            })
        })
    };
    let index = section.index;
    let foldable = section.foldable();
    let heading = category_heading(app, at.task.clone(), &section, open);
    let at = at.clone();
    view()
        .when(app.form != Form::Terminal, |v| {
            v.border_t_1().border_t_color(color(Token::Rule))
        })
        .child(heading)
        .child(
            view()
                .when(app.form != Form::Terminal, |v| v.px(px(16.0)).pb(px(4.0)))
                .children(section.summary.clone().map(|words| quiet(app, words)))
                .children(section.empty.clone().map(|words| quiet(app, words)))
                .children(section.notes.iter().cloned().map(|note| quiet(app, note)))
                .into_element(),
        )
        .child(dynamic(move || {
            if !foldable || !open.get() {
                return None;
            }
            Some(untracked(|| {
                view()
                    .children(
                        section
                            .groups
                            .iter()
                            .cloned()
                            .enumerate()
                            .map(|(n, group)| grouped(app, &at, index, n, group)),
                    )
                    .into_element()
            }))
        }))
        .into_element()
}

fn category_heading(app: AppState, task: Id, section: &Section, open: Memo<bool>) -> AnyElement {
    let form = app.form;
    let handle = element_ref();
    let index = section.index;
    let foldable = section.foldable();
    let header = view()
        .element_ref(&handle)
        .test_id(&format!("impact-cat-{index}"))
        .label(format!("{}, {}", section.title, section.figure))
        .flex_row()
        .items_center()
        .gap(if form == Form::Terminal {
            term::cells(1.0)
        } else {
            px(8.0)
        })
        .when(form != Form::Terminal, |v| {
            v.px(px(16.0))
                .py(px(4.0))
                .min_h(px(if form == Form::Phone { 44.0 } else { 30.0 }))
        })
        .when(!foldable, |v| v.role(AccessibilityRole::Header))
        .when(foldable, |v| {
            v.role(AccessibilityRole::Button)
                .accessibility_state(move || AccessibilityState {
                    expanded: Some(open.get()),
                    ..AccessibilityState::default()
                })
                .cursor_pointer()
                .on_press(move || {
                    app.impact_act(
                        task.as_str(),
                        Action::Category {
                            index,
                            open: !open.get(),
                        },
                    )
                })
                .when(form != Form::Terminal, |v| {
                    v.hover(|s| s.bg(color(Token::Hover))).focus_visible(|s| {
                        s.outline(px(2.0))
                            .outline_color(color(Token::Focus))
                            .outline_offset(px(-2.0))
                    })
                })
        });
    header
        .children((form == Form::Terminal).then(|| term::focus_cell(term::focus_of(handle))))
        .child(
            plain(app, section.title)
                .font_weight(FontWeight::BOLD)
                .flex_1()
                .min_w(px(0.0)),
        )
        .child(
            mono(app, section.figure.clone())
                .text_color(color(Token::Ink3))
                .max_w(relative(0.55))
                .min_w(px(0.0)),
        )
        .children(foldable.then(|| {
            text(move || if open.get() { "▾" } else { "▸" })
                .flex_shrink_0()
                .text_color(color(Token::Ink3))
        }))
        .into_element()
}

fn grouped(app: AppState, at: &At, category: usize, index: usize, group: Group) -> AnyElement {
    let hidden = group.items.len().saturating_sub(LIMIT);
    let heading = group.heading.clone().map(|(title, figure)| {
        view()
            .flex_row()
            .items_start()
            .gap(if app.form == Form::Terminal {
                term::cells(1.0)
            } else {
                px(8.0)
            })
            .when(app.form != Form::Terminal, |v| v.px(px(16.0)).pt(px(4.0)))
            .child(mono(app, title).min_w(px(0.0)))
            .child(dim(app, figure).flex_shrink_0())
            .into_element()
    });
    let first = items(app, at, group.items.iter().take(LIMIT).cloned().collect());
    if hidden == 0 {
        return view().children(heading).child(first).into_element();
    }
    let group = Rc::new(group);
    let all = {
        let (group, task) = (group.clone(), at.task.clone());
        computed(move || {
            app.impact.with(|state| {
                let state = (state.task.as_deref() == Some(task.as_str())).then_some(state);
                group.show_all(state, category, index)
            })
        })
    };
    let at = at.clone();
    view()
        .children(heading)
        .child(first)
        .child(dynamic(move || {
            let all = all.get();
            untracked(|| {
                if all {
                    items(app, &at, group.items.iter().skip(LIMIT).cloned().collect())
                } else {
                    let task = at.task.clone();
                    button(
                        app,
                        format!("Show {} more", count(hidden as u64)),
                        ButtonKind::Quiet,
                        format!("btn-impact-more-{category}-{index}"),
                        move || app.impact_act(task.as_str(), Action::More(category, index)),
                    )
                }
            })
        }))
        .into_element()
}

fn sign_hue(sign: Sign) -> Hue {
    match sign {
        Sign::Added => Hue::Ok,
        Sign::Removed => Hue::Fail,
        Sign::Changed => Hue::You,
        Sign::Dependent => Hue::Corral,
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
    let At { task, project, map } = at;
    let selected = {
        let (key, task) = (item.key.clone(), task.clone());
        computed(move || {
            app.impact.with(|state| {
                state.task.as_deref() == Some(task.as_str()) && state.item.as_ref() == Some(&key)
            })
        })
    };
    let label = format!(
        "{} {} {} {}",
        item.sign.word(),
        item.main,
        item.right,
        item.sub.as_deref().unwrap_or_default()
    );
    let (key, target) = (item.key.clone(), item.target.clone());
    let press_task = task.clone();
    let line = list_row_wrap(
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
            );
            // Where the map is not beside the list, it comes into view to show the move.
            if target.is_some() {
                reveal(map);
            }
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
                .child(
                    view()
                        .flex_row()
                        .items_start()
                        .gap(if form == Form::Terminal {
                            term::cells(1.0)
                        } else {
                            px(8.0)
                        })
                        .child(plain(app, item.main.clone()).flex_1().min_w(px(0.0)))
                        .children(
                            (!item.right.is_empty())
                                .then(|| dim(app, item.right.clone()).max_w(relative(0.48))),
                        ),
                )
                .children(item.sub.clone().map(|sub| quiet(app, sub)))
                .into_element(),
        ],
    );
    let detail = dynamic(move || {
        if !selected.get() {
            return None;
        }
        let (anchor, note) = (item.anchor.clone(), item.note.clone());
        let lined = matches!(item.source, Some(Some(_)));
        let excerpt = item.source.clone().map(|lines| {
            untracked(|| lenses::source(app, lines, "impact-source", impact::SOURCE_UNAVAILABLE))
        });
        let drawn = item.lens.clone().map(|touched| {
            let head = lens::lens_request(project.as_str(), &touched.file, Some(&touched.head));
            let base = lens::lens_request(project.as_str(), &touched.file, Some(&touched.base));
            // Each side's names are that side's own numbering of repeats.
            let at_head = Touch {
                touched: touched.touched,
                added: touched.added,
                removed: Vec::new(),
            };
            let at_base = Touch {
                touched: touched.touched_base,
                added: Vec::new(),
                removed: touched.removed,
            };
            untracked(|| lens_impact::touched_lens(app, head, base, at_head, at_base))
        });
        // A lens has no node on the map; it is drawn here instead.
        let to_system = if item.lens.is_some() {
            view().into_element()
        } else {
            to_system(app, project.clone(), item.target.clone())
        };
        let focus = item.target.clone().map(|target| {
            let task = task.clone();
            button(
                app,
                FOCUS_ON_MAP,
                ButtonKind::Quiet,
                "btn-impact-focus",
                move || {
                    app.impact_act(task.as_str(), Action::Focus(target.clone()));
                    reveal(map);
                },
            )
        });
        Some(untracked(|| {
            view()
                .when(form == Form::Terminal, |v| v.pl(term::cells(3.0)))
                .when(form != Form::Terminal, |v| {
                    v.pl(px(28.0)).py(px(4.0)).gap(px(4.0)).items_start()
                })
                // Served lines carry the anchor as their heading.
                .children((!lined).then(|| {
                    plain(
                        app,
                        anchor.map_or_else(|| NO_ANCHOR.to_owned(), |a| a.words()),
                    )
                }))
                .children(note.map(|note| quiet(app, note)))
                .children(excerpt)
                .children(drawn)
                .children(focus)
                .child(to_system)
                .into_element()
        }))
    });
    view().child(line).child(detail).into_element()
}

fn reveal(map: ElementRef) {
    let mut options = RevealOptions::default();
    options.vertical = ScrollAlignment::Nearest;
    map.reveal(options);
}

/// "Open in System" where Insight › Structure's own map has the item's node (or the level that
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
                .when(app.form != Form::Terminal, |v| v.gap(px(4.0)))
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
