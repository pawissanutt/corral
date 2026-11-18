//! The desktop and web shell: a top bar across the window, the sidebar of destinations
//! (folding to a rail of icons by choice or on a narrow window), and the content container that
//! holds the connection banner and the current page, with the Ask drawer, the palette, the
//! confirmation and the toast over it. On a touch tablet every shell control reaches the touch
//! target ([`control_height`]).

use nsh_core::AccessibilityCurrent;
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::health::waits_on_you;
use crate::model::nav::{
    ASK_HINT, ASK_KEY, ASK_LABEL, PALETTE_HINT, PALETTE_KEY, SIGNS_AS, THIS_DEVICE,
};
use crate::model::projects::PROJECTS_HEADING;
use crate::model::{Destination, SettingsSection, Workspace};
use crate::screens;
use crate::shell::{
    confirmation, destinations, projects, section_short, sidebar_collapsed, stale, toast,
};
use crate::state::AppState;
use crate::ui::ask;
use crate::ui::atoms::{
    control_height, count, destination_spoken, key_hint, nav_row, nav_row_icon,
};
use crate::ui::composer::ask_surface;
use crate::ui::health_line::{HealthLayout, health_line};
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::notice;
use crate::ui::palette::palette;
use crate::ui::tokens::{FONT_MONO, Token, color, row_height, text_size};

const TOP_BAR_HEIGHT: f32 = 38.0;
const SIDEBAR_WIDTH: f32 = 208.0;
/// The folded sidebar: room for a destination's icon and its count badge.
const RAIL_WIDTH: f32 = 56.0;
const RAIL_BUTTON: f32 = 40.0;
const RADIUS_CONTROL: f32 = 6.0;
const FOCUS_RING: f32 = 2.0;
/// Settings sections sit past the width of a destination's icon and the gap after it (16 + 10),
/// so their labels line up with the destinations' labels.
const SECTION_INDENT: f32 = 26.0;
/// The workspace path gives up its width long before the health line gives up any of its own:
/// flex shrinking is weighted by this factor against the health line's 1.
const PATH_SHRINK: f32 = 1000.0;
/// Narrower than this the path shows a few characters and an ellipsis, which says nothing.
const PATH_MIN_WIDTH: f32 = 56.0;
/// The board shown while corrald is down or this device offline is the last known one: it
/// recedes (the mock's `.is-stale .stalefx`, whose desaturation NSH has no filter for).
const STALE_OPACITY: f32 = 0.78;

pub fn shell(app: AppState) -> AnyElement {
    // Memos so a change elsewhere in the snapshot or the nav state (an overlay
    // opening) does not rebuild the sidebar rows or the workspace blocks. No destination row is
    // current while a Project page shows: its row in PROJECTS is.
    let current = computed(move || {
        projects::open_project(app)
            .is_none()
            .then(|| app.destination())
    });
    let shown = computed(move || destinations(app));
    let workspace = computed(move || app.snapshot().workspace);
    let collapsed = computed(move || sidebar_collapsed(app));
    view()
        .flex_1()
        .min_h(px(0.0))
        .bg(color(Token::Shell))
        .child(top_bar(app, workspace, collapsed))
        .child(
            view()
                .flex_1()
                .flex_row()
                .relative()
                .min_h(px(0.0))
                .child(dynamic(move || {
                    if collapsed.get() {
                        untracked(|| rail(app, current, shown))
                    } else {
                        untracked(|| sidebar(app, current, shown, workspace))
                    }
                }))
                .child(content(app))
                .child(when(
                    move || collapsed.get() && app.sidebar_peeking(),
                    move || untracked(|| peek(app, current, shown, workspace)),
                )),
        )
        .child(ask::hotkey(app))
        .into_element()
}

fn top_bar(app: AppState, workspace: Memo<Option<Workspace>>, collapsed: Memo<bool>) -> AnyElement {
    view()
        .h(px(control_height(TOP_BAR_HEIGHT)))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .pl(px(8.0))
        .pr(px(10.0))
        .bg(color(Token::Shell))
        .border_b(px(1.0))
        .border_b_color(color(Token::ShellInk).opacity(0.1))
        .child(
            view()
                .flex_row()
                .flex_shrink_0()
                .items_center()
                .gap(px(8.0))
                .child(sidebar_toggle(app, collapsed))
                .child(icon(Icon::Brand, 18.0, color(Token::ShellInk)))
                .child(
                    text("Corral")
                        .text_size(px(text_size(app.form) - 1.0))
                        .font_weight(FontWeight::BOLD)
                        .letter_spacing(px(0.1))
                        .text_color(color(Token::ShellInk)),
                ),
        )
        .child(dynamic(move || {
            workspace.get().map(|ws| workspace_path(app, ws))
        }))
        // Sized from the health line's own width (not from zero) so that, when the bar runs
        // short, the path shrinks first and the health line keeps its room.
        .child(
            view()
                .flex_auto()
                .min_w(px(0.0))
                .overflow_hidden()
                .child(health_line(app, HealthLayout::Bar)),
        )
        .into_element()
}

/// Shrinks with an ellipsis as the bar runs short, then hides once too narrow to read; it keeps
/// its place, so hiding it never changes the room the health line gets.
fn workspace_path(app: AppState, ws: Workspace) -> AnyElement {
    let cramped = signal(false);
    text(ws.path.into_owned())
        .flex_shrink(PATH_SHRINK)
        .min_w(px(0.0))
        .truncate()
        .font_family(FONT_MONO)
        .text_size(px(text_size(app.form) - 1.5))
        .text_color(color(Token::ShellInk2))
        .on_layout(move |frame: &Bounds<Points>| {
            cramped.set(frame.size.width.0 < PATH_MIN_WIDTH);
        })
        .style_when(move || cramped.get(), |s| s.invisible())
        .into_element()
}

/// The workspace header, then Ask and the palette, the destinations this workspace has, the
/// Settings sections under Settings while it is selected, the PROJECTS section, and this device.
fn sidebar(
    app: AppState,
    current: Memo<Option<Destination>>,
    shown: Memo<Vec<Destination>>,
    workspace: Memo<Option<Workspace>>,
) -> AnyElement {
    let rows = dynamic(move || {
        let shown = shown.get();
        untracked(move || {
            view()
                .gap(px(2.0))
                .children(shown.into_iter().map(|d| destination_row(app, current, d)))
                .into_element()
        })
    });
    view()
        .role(AccessibilityRole::Navigation)
        .w(px(SIDEBAR_WIDTH))
        .flex_shrink_0()
        .min_h(px(0.0))
        .pt(px(10.0))
        .pb(px(10.0))
        .px(px(8.0))
        .gap(px(2.0))
        .bg(color(Token::Shell))
        .child(dynamic(move || {
            workspace.get().map(|ws| workspace_header(app, ws))
        }))
        .child(ask_button(app))
        .child(palette_opener(app))
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .gap(px(2.0))
                    .child(rows)
                    .child(when(
                        move || current.get() == Some(Destination::Settings),
                        move || settings_sections(app),
                    ))
                    .child(projects::section(app)),
            ),
        )
        .child(dynamic(move || {
            workspace.get().map(|ws| device_footer(app, ws))
        }))
        .into_element()
}

/// Folds or unfolds the sidebar, drawn as a window with its sidebar (a back chevron would read
/// as leaving the page); it says whether the labels are shown. test_id "sidebar-toggle".
fn sidebar_toggle(app: AppState, collapsed: Memo<bool>) -> AnyElement {
    let ink = color(Token::ShellInk);
    view()
        .size(px(control_height(28.0)))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_CONTROL))
        .cursor_pointer()
        .transition_colors()
        .duration(120.0_f32)
        .hover(move |s| s.bg(ink.opacity(0.1)))
        .active(move |s| s.bg(ink.opacity(0.16)))
        .focus_visible(focus_ring)
        .role(AccessibilityRole::Button)
        .label("Show or hide sidebar labels")
        .accessibility_state(move || AccessibilityState {
            expanded: Some(!collapsed.get() || app.sidebar_peeking()),
            ..AccessibilityState::default()
        })
        .test_id("sidebar-toggle")
        .on_press(move || app.toggle_sidebar())
        .child(icon(Icon::Sidebar, 16.0, color(Token::ShellInk2)))
        .into_element()
}

/// The narrow layout's labels, opened from the rail over the content for the moment: a scrim
/// that closes them over the whole area under the top bar, and the full sidebar pinned to that
/// area's top and bottom edges. The panel is a row, as the layout row that holds the sidebar
/// when wide, so the sidebar stretches to the full height and its list scrolls inside it.
fn peek(
    app: AppState,
    current: Memo<Option<Destination>>,
    shown: Memo<Vec<Destination>>,
    workspace: Memo<Option<Workspace>>,
) -> AnyElement {
    view()
        .absolute()
        .inset_0()
        .test_id("sidebar-peek")
        .child(
            view()
                .absolute()
                .inset_0()
                .bg(color(Token::Shell).opacity(0.35))
                .focusable(false)
                .accessibility_elements_hidden(true)
                .on_press(move || {
                    app.close_sidebar_peek();
                }),
        )
        .child(
            view()
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .left(px(0.0))
                .flex_row()
                .bg(color(Token::Shell))
                .shadow_2xl()
                .shadow_color(color(Token::Shell))
                .child(sidebar(app, current, shown, workspace)),
        )
        .into_element()
}

/// The folded sidebar: Ask and the palette, then each destination as its icon with its count,
/// the Settings sections as short names under Settings, and one Projects mark that unfolds the
/// labels to reach the projects. Every control keeps its full name (and a destination its
/// count) for assistive technology, shows its name and key in a hint while the pointer rests on
/// it, and the palette lists them all.
fn rail(
    app: AppState,
    current: Memo<Option<Destination>>,
    shown: Memo<Vec<Destination>>,
) -> AnyElement {
    let ink = color(Token::ShellInk);
    let rows = dynamic(move || {
        let shown = shown.get();
        untracked(move || {
            view()
                .items_center()
                .gap(px(2.0))
                .children(shown.into_iter().map(|d| rail_destination(app, current, d)))
                .into_element()
        })
    });
    view()
        .role(AccessibilityRole::Navigation)
        .w(px(RAIL_WIDTH))
        .flex_shrink_0()
        .min_h(px(0.0))
        .items_center()
        .pt(px(10.0))
        .pb(px(10.0))
        .gap(px(2.0))
        .bg(color(Token::Shell))
        .child(
            rail_button(
                app,
                Hint {
                    name: ASK_LABEL,
                    key: Some(ASK_KEY.to_owned()),
                },
                move || SharedString::from(ASK_LABEL),
                "ask-open".into(),
                || false,
                move || app.open_ask(),
            )
            .bg(ink.opacity(0.1))
            .child(icon(Icon::Ask, 18.0, ink)),
        )
        .child(
            rail_button(
                app,
                Hint {
                    name: PALETTE_HINT,
                    key: Some(PALETTE_KEY.to_owned()),
                },
                move || SharedString::from(PALETTE_HINT),
                "palette-open".into(),
                || false,
                move || app.open_palette(),
            )
            .mb(px(10.0))
            .child(icon(Icon::Search, 18.0, color(Token::ShellInk2))),
        )
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .items_center()
                    .gap(px(2.0))
                    .child(rows)
                    .child(when(
                        move || current.get() == Some(Destination::Settings),
                        move || rail_sections(app),
                    ))
                    .child(rail_projects(app)),
            ),
        )
        .into_element()
}

fn rail_destination(
    app: AppState,
    current: Memo<Option<Destination>>,
    d: Destination,
) -> AnyElement {
    let ink = color(Token::ShellInk);
    rail_button(
        app,
        Hint {
            name: d.label(),
            key: Some(d.key().to_string()),
        },
        destination_spoken(app, d),
        format!("nav-{}", d.slug()),
        move || current.get() == Some(d),
        move || app.go(d),
    )
    .child(icon(Icon::destination(d), 18.0, ink))
    .child(
        view()
            .absolute()
            .top(px(1.0))
            .right(px(0.0))
            .child(count(app, d)),
    )
    .into_element()
}

/// The folded PROJECTS section: one mark, current while a Project page shows, with how many
/// things across the projects need the owner. It unfolds the labels (over the page while
/// narrow), where each project has its row. test_id "nav-projects".
fn rail_projects(app: AppState) -> AnyElement {
    let any = computed(move || has_projects(app));
    when(
        move || any.get(),
        move || {
            let needs = computed(move || projects::needs_total(app));
            rail_button(
                app,
                Hint {
                    name: PROJECTS_HEADING,
                    key: None,
                },
                move || match needs.get() {
                    0 => SharedString::from(PROJECTS_HEADING),
                    n => SharedString::from(format!("{PROJECTS_HEADING}, {}", waits_on_you(n))),
                },
                "nav-projects".into(),
                move || projects::open_project(app).is_some(),
                move || app.toggle_sidebar(),
            )
            .mt(px(10.0))
            .child(icon(Icon::Project, 18.0, color(Token::ShellInk)))
            .child(
                view()
                    .absolute()
                    .top(px(1.0))
                    .right(px(0.0))
                    .child(dynamic(move || {
                        let n = needs.get();
                        (n > 0).then(|| untracked(|| projects::rail_badge(n)))
                    })),
            )
            .into_element()
        },
    )
    .into_element()
}

/// Tracked: whether the PROJECTS section has any row.
fn has_projects(app: AppState) -> bool {
    app.feed.with(|feed| {
        !crate::model::projects::Projects::build(feed, &crate::model::ProjectFilter::All)
            .entries
            .is_empty()
    })
}

/// The Settings sections in the rail, each its short name under its full one for assistive
/// technology.
fn rail_sections(app: AppState) -> AnyElement {
    view()
        .items_center()
        .gap(px(2.0))
        .children(SettingsSection::ALL.map(|section| {
            let selected = move || {
                app.nav
                    .with(|nav| nav.destination == Destination::Settings && nav.settings == section)
            };
            let name = section.label();
            rail_button(
                app,
                Hint { name, key: None },
                move || SharedString::from(name),
                format!("settings-{}", section.slug()),
                selected,
                move || app.go_settings(section),
            )
            .h(px(control_height(row_height(app.form))))
            .child(
                text(section_short(section))
                    .text_size(px(text_size(app.form) - 2.5))
                    .text_color(move || {
                        if selected() {
                            color(Token::ShellInk)
                        } else {
                            color(Token::ShellInk2)
                        }
                    }),
            )
            .into_element()
        }))
        .into_element()
}

/// What a rail control's hint shows: its full name and, where it has one, its key.
struct Hint {
    name: &'static str,
    key: Option<String>,
}

/// One square control of the rail, tinted while selected (the current page for assistive
/// technology), named by `spoken`, with its [`Hint`] while the pointer rests on it.
fn rail_button(
    app: AppState,
    hint: Hint,
    spoken: impl Fn() -> SharedString + 'static,
    test_id: String,
    selected: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
) -> View {
    let ink = color(Token::ShellInk);
    let selected = computed(selected);
    view()
        .relative()
        .w(px(control_height(RAIL_BUTTON)))
        .h(px(control_height(row_height(app.form) + 8.0)))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_CONTROL))
        .cursor_pointer()
        .transition_colors()
        .duration(120.0_f32)
        .bg(move || {
            if selected.get() {
                ink.opacity(0.14)
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(move |s| s.bg(ink.opacity(0.1)))
        .active(move |s| s.bg(ink.opacity(0.18)))
        .focus_visible(focus_ring)
        .role(AccessibilityRole::Button)
        .label(spoken)
        .accessibility_state(move || AccessibilityState {
            current: selected.get().then_some(AccessibilityCurrent::Page),
            ..AccessibilityState::default()
        })
        .test_id(&test_id)
        .tooltip(rail_hint(hint))
        .on_press(on_press)
}

/// The rail's hint (as the timeline chart's): the name in ink on the surface, the key after it
/// in the quiet mono. It repeats the control's name, so screen readers skip it.
fn rail_hint(hint: Hint) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule2))
        .accessibility_elements_hidden(true)
        .child(
            text(hint.name)
                .number_of_lines(1u32)
                .text_size(px(11.5))
                .text_color(color(Token::Ink)),
        )
        .children(hint.key.map(|key| {
            text(key)
                .number_of_lines(1u32)
                .font_family(FONT_MONO)
                .text_size(px(10.5))
                .text_color(color(Token::Ink3))
        }))
        .into_element()
}

/// The workspace this sidebar shows: its name and autopilot state. A header of its own, apart
/// from the destinations and projects under it: switching between daemons' workspaces goes
/// here. test_id "workspace-header".
fn workspace_header(app: AppState, ws: Workspace) -> AnyElement {
    let base = text_size(app.form);
    view()
        .test_id("workspace-header")
        .pt(px(6.0))
        .pb(px(12.0))
        .px(px(8.0))
        .gap(px(2.0))
        .child(
            text(ws.name.into_owned())
                .truncate()
                .text_size(px(base))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::ShellInk)),
        )
        .when(!ws.autopilot.is_empty(), |v| {
            v.child(
                text(ws.autopilot)
                    .number_of_lines(2u32)
                    .text_size(px(base - 2.0))
                    .text_color(color(Token::ShellInk2)),
            )
        })
        .into_element()
}

fn ask_button(app: AppState) -> AnyElement {
    let ink = color(Token::ShellInk);
    view()
        .h(px(control_height(row_height(app.form) + 4.0)))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(8.0))
        .mb(px(6.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(ink.opacity(0.1))
        .cursor_pointer()
        .transition_colors()
        .duration(120.0_f32)
        .hover(move |s| s.bg(ink.opacity(0.16)))
        .active(move |s| s.bg(ink.opacity(0.2)))
        .focus_visible(focus_ring)
        .role(AccessibilityRole::Button)
        .label(ASK_LABEL)
        .test_id("ask-open")
        .on_press(move || app.open_ask())
        .child(icon(Icon::Ask, 16.0, ink))
        .child(
            text(ASK_HINT)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(text_size(app.form)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ink),
        )
        .child(key_hint(ASK_KEY))
        .into_element()
}

fn palette_opener(app: AppState) -> AnyElement {
    let ink = color(Token::ShellInk);
    let quiet = color(Token::ShellInk2);
    view()
        .h(px(control_height(row_height(app.form))))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(8.0))
        .mb(px(10.0))
        .rounded(px(RADIUS_CONTROL))
        .border(px(1.0))
        .border_color(ink.opacity(0.16))
        .cursor_pointer()
        .transition_colors()
        .duration(120.0_f32)
        .hover(move |s| s.bg(ink.opacity(0.06)).border_color(ink.opacity(0.28)))
        .active(move |s| s.bg(ink.opacity(0.1)))
        .focus_visible(focus_ring)
        .role(AccessibilityRole::Button)
        .label(PALETTE_HINT)
        .test_id("palette-open")
        .on_press(move || app.open_palette())
        .child(icon(Icon::Search, 16.0, quiet))
        .child(
            text(PALETTE_HINT)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(text_size(app.form)))
                .text_color(quiet),
        )
        .child(key_hint(PALETTE_KEY))
        .into_element()
}

fn destination_row(
    app: AppState,
    current: Memo<Option<Destination>>,
    d: Destination,
) -> AnyElement {
    let trailing = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(count(app, d))
        .child(
            text(d.key().to_string())
                .w(px(10.0))
                .font_family(FONT_MONO)
                .text_size(px(text_size(app.form) - 3.0))
                .text_color(color(Token::ShellInk2))
                .accessibility_elements_hidden(true),
        )
        .into_element();
    nav_row_icon(
        app,
        Some(Icon::destination(d)),
        d.label(),
        destination_spoken(app, d),
        format!("nav-{}", d.slug()),
        move || current.get() == Some(d),
        Some(trailing),
        move || app.go(d),
    )
}

fn settings_sections(app: AppState) -> AnyElement {
    view()
        .pl(px(SECTION_INDENT))
        .gap(px(2.0))
        .children(SettingsSection::ALL.map(|section| {
            nav_row(
                app,
                section.label(),
                format!("settings-{}", section.slug()),
                move || {
                    app.nav.with(|nav| {
                        nav.destination == Destination::Settings && nav.settings == section
                    })
                },
                None,
                move || app.go_settings(section),
            )
        }))
        .into_element()
}

fn device_footer(app: AppState, ws: Workspace) -> AnyElement {
    let small = text_size(app.form) - 1.5;
    view()
        .flex_shrink_0()
        .mt(px(8.0))
        .pt(px(10.0))
        .pb(px(2.0))
        .px(px(8.0))
        .gap(px(6.0))
        .border_t(px(1.0))
        .border_t_color(color(Token::ShellInk).opacity(0.12))
        .child(
            text(format!("{THIS_DEVICE} "))
                .text_size(px(small))
                .text_color(color(Token::ShellInk2))
                .child(
                    text(ws.device.into_owned())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(Token::ShellInk)),
                ),
        )
        .child(
            text(format!("{SIGNS_AS} "))
                .text_size(px(small))
                .text_color(color(Token::ShellInk2))
                .child(text(ws.via.into_owned()).font_family(FONT_MONO)),
        )
        .into_element()
}

/// The connection banner over the page, then the page, dimmed while the board is stale; over
/// them the Ask drawer, the palette, the confirmation and the toast, which the contract wants
/// as the container's last children so they cover the page and nothing else. The page is a
/// landmark named by its destination (NSH has no main role), beside the sidebar's navigation.
fn content(app: AppState) -> AnyElement {
    view()
        .flex_1()
        .relative()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_hidden()
        .bg(color(Token::Bg))
        .child(notice::connection(app))
        .child(
            view()
                .role(AccessibilityRole::Region)
                .label(move || {
                    SharedString::from(
                        projects::open_project(app).map_or_else(
                            || app.destination().label().to_owned(),
                            |id| id.to_string(),
                        ),
                    )
                })
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .style_when(move || stale(app), |s| s.opacity(STALE_OPACITY))
                .child(screens::content(app)),
        )
        .child(ask_surface(app))
        .child(palette(app))
        .child(crate::screens::agents::rewind_sheet(app))
        .child(confirmation(app))
        .child(crate::shell::refusal(app))
        .child(crate::screens::now::verb_composer(app))
        .child(toast(app))
        .into_element()
}

fn focus_ring(s: StyleRefinement) -> StyleRefinement {
    s.outline(px(FOCUS_RING))
        .outline_color(color(Token::Focus))
        .outline_offset(px(1.0))
}
