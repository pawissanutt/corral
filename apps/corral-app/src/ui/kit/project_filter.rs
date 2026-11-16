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
//! second line under it, always in view. Terminal: the toggles a Cell
//! apart after a quiet caption; NSH bands the pressed one itself. test ids:
//! "seg-project-<all|unassigned|project id>" and "seg-group-<state|project>".

use std::rc::Rc;

use nsh_base::{toggle, toggle_group};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::projects::{self, FilterScreen, NowGroup, ProjectFilter};
use crate::state::AppState;
use crate::ui::term::cells;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The phone's least touch target.
const TARGET: f32 = 44.0;
const RING: f32 = 2.0;
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
            projects::chips(feed, &ProjectFilter::All, screen)
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
    let options = chips
        .iter()
        .map(|(label, filter)| {
            let chosen = filter.clone();
            let picked = filter.clone();
            let on = computed(move || current.with(|current| *current == chosen));
            option(
                form,
                label.clone(),
                format!("seg-project-{}", value(filter)),
                on,
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

/// Now's Group toggle: the list grouped by state (who acts next) or by project.
fn group_toggle(app: AppState) -> AnyElement {
    let form = app.form;
    let current = computed(move || app.now_group());
    let options = [
        (NowGroup::State, "State", "state"),
        (NowGroup::Project, "Project", "project"),
    ]
    .into_iter()
    .map(|(group, label, slug)| {
        let on = computed(move || current.get() == group);
        option(
            form,
            label.into(),
            format!("seg-group-{slug}"),
            on,
            move || app.set_now_group(group),
        )
    })
    .collect::<Vec<_>>();
    labelled(form, GROUP_CAPTION, options)
}

/// A caption and its toggles, one toolbar for screen readers named by the caption.
fn labelled(form: Form, caption: &'static str, options: Vec<AnyElement>) -> AnyElement {
    let words = text(caption)
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(Token::Ink3));
    let words = match form {
        Form::Terminal => words,
        Form::Desktop | Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 13.0 } else { 11.5 })),
    };
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(gap(form))
        .role(AccessibilityRole::Group)
        .label(caption)
        .child(words.accessibility_elements_hidden(true))
        .child(
            toggle_group()
                .flex_row()
                .items_center()
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

/// One pill: its words, pressed while `on` holds; pressing the pressed one keeps it, so the bar
/// always names one scope.
fn option(
    form: Form,
    label: String,
    test_id: String,
    on: Memo<bool>,
    pick: impl Fn() + 'static,
) -> AnyElement {
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
    toggle()
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
        .child(view().test_id(&test_id).child(words))
        .into_element()
}
