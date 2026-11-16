//! The sidebar's PROJECTS section on desktop and in a terminal: under the destinations, a row per
//! configured project (its name, then what needs the owner, what runs and what is open) and an
//! Unassigned row while tasks without a project exist. A project row opens its Project page and
//! stays selected while that page shows; Unassigned opens Now scoped to those tasks. The section
//! is absent while corrald lists no project.

use nsh_core::prelude::*;

use crate::model::projects::{PROJECTS_HEADING, ProjectEntry, Projects};
use crate::model::{Form, Hue, Id, ProjectFilter, Route};
use crate::screens::projects::open_entry;
use crate::state::AppState;
use crate::ui::atoms::nav_row_icon;
use crate::ui::icons::Icon;
use crate::ui::kit::marks::{hue_fill, hue_ink};
use crate::ui::term::{self, cells};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// Tracked: the project whose page is on top, if one is.
pub fn open_project(app: AppState) -> Option<Id> {
    app.nav.with(|nav| match nav.pages.last() {
        Some(Route::Project(id)) => Some(id.clone()),
        _ => None,
    })
}

/// Tracked: the section's rows, the open project's selected.
pub fn entries(app: AppState) -> Memo<Vec<ProjectEntry>> {
    computed(move || {
        let filter = open_project(app).map_or(ProjectFilter::All, ProjectFilter::Project);
        app.feed.with(|feed| Projects::build(feed, &filter).entries)
    })
}

/// The section: its heading and rows, or nothing while there are none. Rows are keyed by their
/// id, so a count moving redraws its figures and never takes a focused row's focus.
pub fn section(app: AppState) -> AnyElement {
    let entries = entries(app);
    let ids = computed(move || {
        entries.with(|list| list.iter().map(|e| e.id.clone()).collect::<Vec<_>>())
    });
    dynamic(move || {
        let ids = ids.get();
        if ids.is_empty() {
            return None;
        }
        untracked(move || {
            let terminal = app.form == Form::Terminal;
            Some(
                view()
                    .test_id("projects")
                    .when(!terminal, |v| v.gap(px(2.0)))
                    .when(terminal, |v| v.child(term::hrule()))
                    .child(heading(app))
                    .children(ids.into_iter().map(|id| row(app, entries, id)))
                    .into_element(),
            )
        })
    })
}

/// The section's name: small capitals on the shell, quiet words in a terminal.
fn heading(app: AppState) -> AnyElement {
    if app.form == Form::Terminal {
        return view()
            .px(cells(1.0))
            .child(term::quiet(PROJECTS_HEADING))
            .into_element();
    }
    text(PROJECTS_HEADING.to_uppercase())
        .role(AccessibilityRole::Header)
        .number_of_lines(1u32)
        .pt(px(16.0))
        .pb(px(4.0))
        .px(px(8.0))
        .font_family(FONT_TEXT)
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .letter_spacing(px(0.6))
        .text_color(color(Token::ShellInk2))
        .into_element()
}

fn row(app: AppState, entries: Memo<Vec<ProjectEntry>>, id: String) -> AnyElement {
    let probe = id.clone();
    let entry = computed(move || entries.with(|list| list.iter().find(|e| e.id == probe).cloned()));
    let label = entry
        .with_untracked(|entry| entry.as_ref().map(|e| e.label.clone()))
        .unwrap_or_default();
    nav_row_icon(
        app,
        Some(Icon::Project),
        label,
        move || {
            SharedString::from(
                entry.with(|e| e.as_ref().map(ProjectEntry::spoken).unwrap_or_default()),
            )
        },
        id,
        move || entry.with(|e| e.as_ref().is_some_and(|e| e.selected)),
        Some(dynamic(move || {
            let entry = entry.get();
            untracked(move || entry.map(|entry| figures(app.form, &entry)))
        })),
        move || {
            if let Some(entry) = entry.get_untracked() {
                open_entry(app, &entry);
            }
        },
    )
}

/// The figures after a project's name: what needs the owner as a badge, what runs after a dot
/// in the worker colour, and what is open, quiet; each only when it is known and above zero.
/// The row's name carries them in words for assistive technology, so they are hidden from it.
fn figures(form: Form, entry: &ProjectEntry) -> AnyElement {
    let needs = entry.needs_me.filter(|n| *n > 0);
    let running = entry.running.filter(|n| *n > 0);
    let open = entry.open.filter(|n| *n > 0);
    if form == Form::Terminal {
        let column = |content: Option<AnyElement>| {
            view()
                .w(cells(2.0))
                .flex_row()
                .justify_end()
                .children(content)
                .into_element()
        };
        return view()
            .flex_row()
            .flex_shrink_0()
            .accessibility_elements_hidden(true)
            .child(column(needs.map(|n| {
                text(n.to_string())
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::You))
                    .into_element()
            })))
            .child(term::space(1.0))
            .child(column(running.map(|n| {
                text(format!("●{n}"))
                    .text_color(hue_ink(Hue::Worker))
                    .into_element()
            })))
            .child(term::space(1.0))
            .child(column(open.map(|n| {
                text(n.to_string())
                    .text_color(color(Token::Ink))
                    .into_element()
            })))
            .child(term::space(1.0))
            .into_element();
    }
    let figure = |n: u32| {
        text(n.to_string())
            .number_of_lines(1u32)
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(11.0))
    };
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(7.0))
        .accessibility_elements_hidden(true)
        .children(needs.map(|n| {
            view()
                .h(px(16.0))
                .min_w(px(16.0))
                .px(px(5.0))
                .rounded(px(8.0))
                .items_center()
                .justify_center()
                .bg(on_shell(hue_ink(Hue::You)))
                .child(
                    figure(n)
                        .font_weight(FontWeight::BOLD)
                        .text_color(on_shell(color(Token::BtnInk))),
                )
                .into_element()
        }))
        .children(running.map(|n| {
            view()
                .flex_row()
                .items_center()
                .gap(px(3.0))
                .child(
                    view()
                        .size(px(5.0))
                        .rounded(px(2.5))
                        .bg(on_shell(hue_ink(Hue::Worker))),
                )
                .child(figure(n).text_color(on_shell(hue_ink(Hue::Worker))))
                .into_element()
        }))
        .children(open.map(|n| figure(n).text_color(color(Token::ShellInk2)).into_element()))
        .into_element()
}

/// A tone's dark value in both appearances: the shell stays dark (as `ui::atoms` draws its own).
fn on_shell(tone: AdaptiveColor) -> AdaptiveColor {
    AdaptiveColor {
        light: tone.dark,
        dark: tone.dark,
    }
}

/// Tracked: what the folded rail's Projects mark says: the needs-me total across projects, so
/// the folded sidebar still shows where the owner is wanted.
pub fn needs_total(app: AppState) -> u32 {
    app.feed.with(|feed| {
        Projects::build(feed, &ProjectFilter::All)
            .entries
            .iter()
            .filter_map(|e| e.needs_me)
            .sum()
    })
}

/// The badge the rail's Projects mark carries while a project needs the owner.
pub fn rail_badge(n: u32) -> AnyElement {
    view()
        .h(px(14.0))
        .min_w(px(14.0))
        .px(px(4.0))
        .rounded(px(7.0))
        .items_center()
        .justify_center()
        .bg(on_shell(hue_fill(Hue::You)))
        .child(
            text(n.to_string())
                .font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(10.0))
                .font_weight(FontWeight::BOLD)
                .text_color(on_shell(hue_ink(Hue::You))),
        )
        .into_element()
}
