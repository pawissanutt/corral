//! The project scope over a list screen: All, each project, then Unassigned, as one row of
//! toggles, one pressed. Each screen keeps its own scope (`AppState::project_filter`), so the bar
//! takes the screen it scopes. The model omits the chips when a workspace has fewer than two
//! projects and no unassigned task; the bar is then absent. Now adds its Group toggle (State or
//! Project) at the trailing end of the same bar.
//!
//! Desktop: a 32 pt band, 16 pt in like the list rows, pill toggles 22 pt high in 12 pt; the
//! pressed one carries the list's selection fill with a rule-2 edge and semibold ink, the others
//! ink-2 with a hairline edge. Phone: one horizontally scrolling line of 32 pt pills (44 pt
//! targets) in 13.5 pt, so many projects never wrap the list down; Now's Group toggle sits on a
//! second line under it, always in view. On desktop and in a terminal the toggles wrap onto
//! further lines within a narrow pane, so none is out of reach. Terminal: the toggles a Cell
//! apart after a quiet caption; NSH bands the pressed one itself. A project's chip carries the
//! count of what waits on the owner there (`corral 3`, bold mono in the needs-you colour) when
//! the daemon reports it, and its hint (`3 wait on you in corral · …`) shows on pointer rest or
//! keyboard focus (desktop) or on the status line (terminal); the phone has no hint and screen
//! readers hear the count with the name. test ids:
//! "seg-project-<all|unassigned|project id>" and "seg-group-<state|project>".

use std::rc::Rc;

use nsh_base::{toggle, toggle_group};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::projects::{self, FilterScreen, NowGroup, ProjectFilter};
use crate::state::AppState;
use crate::ui::kit::hint::hint_on_live;
use crate::ui::term::cells;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The phone's least touch target.
const TARGET: f32 = 44.0;
const RING: f32 = 2.0;
/// What a project chip says beyond its name, which follows the feed while the bar stays put.
#[derive(Debug, Clone, PartialEq, Default)]
struct Facts {
    needs: Option<String>,
    hint: String,
    spoken: String,
}

/// What the bar scopes, for its caption and screen readers.
const PROJECT_CAPTION: &str = "Project";
const GROUP_CAPTION: &str = "Group";

type Trailing = Option<Rc<dyn Fn() -> AnyElement>>;

/// The project scope of `screen`'s list, or nothing while the model offers no chips.
pub fn project_filter(app: AppState, screen: FilterScreen) -> AnyElement {
    bar(app, screen, None)
}

/// Now's project scope with its Group toggle after the chips.
pub fn project_filter_grouped(app: AppState) -> AnyElement {
    bar(
        app,
        FilterScreen::Now,
        Some(Rc::new(move || group_toggle(app))),
    )
}

/// The id suffix of a scope: the project's own id, or the word for All and Unassigned.
fn value(filter: &ProjectFilter) -> &str {
    match filter {
        ProjectFilter::All => "all",
        ProjectFilter::Unassigned => "unassigned",
        ProjectFilter::Project(id) => id.as_str(),
    }
}

fn bar(app: AppState, screen: FilterScreen, trailing: Trailing) -> AnyElement {
    let current = computed(move || app.project_filter(screen));
    // Which scopes exist, apart from which is chosen: choosing one restyles the toggles in place
    // and keeps the pressed one focused.
    let shape = computed(move || {
        app.feed.with(|feed| {
            projects::chips(feed, &app.project_filter(screen), screen)
                .into_iter()
                .map(|chip| (chip.label, chip.filter))
                .collect::<Vec<_>>()
        })
    });
    dynamic(move || {
        shape.with(|chips| {
            (!chips.is_empty()).then(|| {
                let trailing = trailing.clone();
                untracked(|| strip(app, screen, chips, current, trailing))
            })
        })
    })
    .into_element()
}

fn strip(
    app: AppState,
    screen: FilterScreen,
    chips: &[(String, ProjectFilter)],
    current: Memo<ProjectFilter>,
    trailing: Trailing,
) -> AnyElement {
    let form = app.form;
    let facts = computed(move || {
        app.feed.with(|feed| {
            projects::chips(feed, &app.project_filter(screen), screen)
                .into_iter()
                .map(|chip| {
                    let facts = Facts {
                        needs: chip.needs,
                        hint: chip.hint,
                        spoken: chip.spoken,
                    };
                    (chip.filter, facts)
                })
                .collect::<Vec<_>>()
        })
    });
    let options = chips
        .iter()
        .map(|(label, filter)| {
            let chosen = filter.clone();
            let picked = filter.clone();
            let mine = filter.clone();
            let on = computed(move || current.with(|current| *current == chosen));
            let said = computed(move || {
                facts.with(|all| {
                    all.iter()
                        .find(|(filter, _)| *filter == mine)
                        .map(|(_, facts)| facts.clone())
                        .unwrap_or_default()
                })
            });
            option(
                app,
                label.clone(),
                format!("seg-project-{}", value(filter)),
                on,
                Some(said),
                move || app.set_project_filter(screen, picked.clone()),
            )
        })
        .collect::<Vec<_>>();
    let scopes = labelled(form, PROJECT_CAPTION, options);
    let row = view()
        .flex_row()
        .items_center()
        .gap(gap(form))
        .child(scopes);
    if form == Form::Phone {
        // The chips scroll as one line; the Group toggle sits on a line of its own under them,
        // never off the edge a scrolled line might hide it past.
        let chips = scroll()
            .horizontal()
            .flex_shrink_0()
            .child(row.flex_shrink_0().px(px(18.0)).pt(px(4.0)).pb(px(10.0)));
        let line = trailing.map(|trailing| {
            view()
                .flex_row()
                .items_center()
                .px(px(18.0))
                .pb(px(10.0))
                .child(trailing())
        });
        return view()
            .flex_shrink_0()
            .child(chips)
            .children(line)
            .into_element();
    }
    let row = match trailing {
        Some(trailing) => row.child(view().flex_1().min_w(px(0.0))).child(trailing()),
        None => row,
    };
    match form {
        Form::Phone => row.into_element(),
        Form::Desktop => row
            .flex_wrap()
            .min_h(px(32.0))
            .pl(px(16.0))
            .pr(px(14.0))
            .py(px(4.0))
            .into_element(),
        Form::Terminal => row.flex_wrap().px(cells(1.0)).into_element(),
    }
}

/// Now's Group toggle: the list grouped by actor (who acts next) or by project.
fn group_toggle(app: AppState) -> AnyElement {
    let form = app.form;
    let current = computed(move || app.now_group());
    let options = [
        (NowGroup::State, "Actor", "state"),
        (NowGroup::Project, "Project", "project"),
    ]
    .into_iter()
    .map(|(group, label, slug)| {
        let on = computed(move || current.get() == group);
        option(
            app,
            label.into(),
            format!("seg-group-{slug}"),
            on,
            None,
            move || app.set_now_group(group),
        )
    })
    .collect::<Vec<_>>();
    labelled(form, GROUP_CAPTION, options)
}

/// A caption and its toggles, one toolbar for screen readers named by the caption. On desktop
/// and in a terminal the group shrinks to the room it has and its toggles wrap onto further
/// lines, so a narrow pane (the 280 pt compact list) never pushes a chip out of reach; on the
/// phone the caller scrolls the line, so the group keeps its natural width.
fn labelled(form: Form, caption: &'static str, options: Vec<AnyElement>) -> AnyElement {
    let wraps = form != Form::Phone;
    let words = text(caption)
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(Token::Ink3));
    let words = match form {
        Form::Terminal => words.accessibility_elements_hidden(true).into_element(),
        Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(13.0))
            .accessibility_elements_hidden(true)
            .into_element(),
        // As tall as a pill, so the caption stays level with the first line of toggles.
        Form::Desktop => view()
            .h(px(22.0))
            .flex_shrink_0()
            .justify_center()
            .child(
                words
                    .font_family(FONT_TEXT)
                    .text_size(px(11.5))
                    .accessibility_elements_hidden(true),
            )
            .into_element(),
    };
    view()
        .flex_row()
        .when(wraps, |group| {
            group.items_start().flex_shrink(1.0).min_w(px(0.0))
        })
        .when(!wraps, |group| group.items_center().flex_shrink_0())
        .gap(gap(form))
        .role(AccessibilityRole::Group)
        .label(caption)
        .child(words)
        .child(
            toggle_group()
                .flex_row()
                .items_center()
                .when(wraps, |toggles| {
                    toggles.flex_wrap().flex_shrink(1.0).min_w(px(0.0))
                })
                .gap(match form {
                    Form::Terminal => cells(1.0),
                    Form::Desktop => px(4.0),
                    Form::Phone => px(6.0),
                })
                .children(options),
        )
        .into_element()
}

fn gap(form: Form) -> Points {
    match form {
        Form::Terminal => cells(2.0),
        Form::Desktop => px(10.0),
        Form::Phone => px(8.0),
    }
}

/// One pill: its words, and for a project scope the count waiting on the owner after them and
/// the scope's hint; pressed while `on` holds; pressing the pressed one keeps it, so the bar
/// always names one scope.
fn option(
    app: AppState,
    label: String,
    test_id: String,
    on: Memo<bool>,
    facts: Option<Memo<Facts>>,
    pick: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let (height, size, pad) = match form {
        Form::Desktop => (22.0, 12.0, 9.0),
        Form::Phone => (32.0, 13.5, 12.0),
        Form::Terminal => (16.0, 0.0, 0.0),
    };
    let words = text(label)
        .number_of_lines(1u32)
        .text_color(move || {
            if on.get() || form == Form::Terminal {
                color(Token::Ink)
            } else {
                color(Token::Ink2)
            }
        })
        .style_when(move || on.get(), |s| s.font_weight(FontWeight::SEMIBOLD))
        .when(form != Form::Terminal, |t| {
            t.font_family(FONT_TEXT).text_size(px(size))
        });
    let pill = toggle()
        .pressed(move || on.get())
        .on_change(move |_: &bool| pick())
        .flex_shrink_0()
        .h(px(height))
        .items_center()
        .justify_center()
        .cursor_pointer()
        // In a terminal NSH bands a pressed toggle itself; a fill of the app's would be a second.
        .when(form != Form::Terminal, |t| {
            t.px(px(pad))
                .rounded(px(height / 2.0))
                .border_1()
                .bg(move || {
                    if on.get() {
                        color(Token::Sel)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .border_color(move || {
                    if on.get() {
                        color(Token::Rule2)
                    } else {
                        color(Token::Rule)
                    }
                })
                .hover(|s| s.bg(color(Token::Hover)))
                .focus_visible(|s| {
                    s.outline(px(RING))
                        .outline_color(color(Token::Focus))
                        .outline_offset(px(RING))
                })
        })
        // The 32 pt pill reaches the 44 pt target above and below, never into its neighbours.
        .when(form == Form::Phone, |t| {
            let slop = px((TARGET - height) / 2.0);
            t.active(|s| s.bg(color(Token::Hover)))
                .hit_slop(EdgeInsets {
                    top: slop,
                    right: px(0.0),
                    bottom: slop,
                    left: px(0.0),
                })
        })
        .when_some(facts, |t, facts| {
            t.accessibility_label(move || {
                facts.with(|facts| Some(SharedString::from(facts.spoken.clone())))
            })
        })
        .child(
            view()
                .test_id(&test_id)
                .flex_row()
                .items_center()
                .gap(match form {
                    Form::Terminal => cells(1.0),
                    Form::Desktop | Form::Phone => px(6.0),
                })
                .child(words)
                .children(facts.map(|facts| {
                    dynamic(move || {
                        facts.with(|facts| facts.needs.clone().map(|n| needs_count(form, n)))
                    })
                })),
        );
    match facts {
        // A toggle takes no tooltip of its own: the view around it does, and the focus on the
        // toggle inside still shows it.
        Some(facts) if form != Form::Phone => {
            hint_on_live(app, view().flex_shrink_0().child(pill), move || {
                facts.with(|facts| SharedString::from(facts.hint.clone()))
            })
            .into_element()
        }
        _ => pill.into_element(),
    }
}

/// The figures of what waits on the owner in a project (`.pc b`): bold mono in the needs-you
/// colour, desktop 11.5 pt, phone 13 pt; a terminal's in bold.
fn needs_count(form: Form, needs: String) -> AnyElement {
    let figures = text(needs)
        .number_of_lines(1u32)
        .flex_shrink_0()
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::You));
    match form {
        Form::Terminal => figures.into_element(),
        Form::Desktop | Form::Phone => figures
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(if form == Form::Phone { 13.0 } else { 11.5 }))
            .into_element(),
    }
}
