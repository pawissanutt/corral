//! The desktop and web shell: a top bar across the window, the sidebar (the scope switcher,
//! the four places with Activity's pages under it while one shows, then Agents, Go to and
//! Settings at its foot; folding to a rail of icons by choice or on a narrow window), and the
//! content container that holds the connection banner and the current page, with the Ask
//! drawer, the palette, the confirmation and the toast over it. On a touch tablet every shell
//! control reaches the touch target ([`control_height`]).

use nsh_core::AccessibilityCurrent;
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::agents::switcher::{AGENTS_KEY, AGENTS_LABEL};
use crate::model::commands::area_key;
use crate::model::nav::{PALETTE_HINT, PALETTE_KEY, SIGNS_AS, THIS_DEVICE};
use crate::model::{Area, Destination, SettingsSection, Workspace};
use crate::screens;
use crate::shell::{
    activity_pages, confirmation, current_area, places, section_short, sidebar_collapsed, stale,
    toast,
};
use crate::state::AppState;
use crate::ui::ask;
use crate::ui::atoms::{area_spoken, control_height, count, key_hint, nav_row, nav_row_icon};
use crate::ui::composer::ask_surface;
use crate::ui::health_line::{HealthLayout, health_line};
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::notice;
use crate::ui::kit::scope::{ScopeFace, scope_switcher};
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
    // opening) does not rebuild the sidebar rows or the workspace blocks.
    let current = computed(move || current_area(app));
    let shown = computed(move || places(app));
    let workspace = computed(move || app.snapshot().workspace);
    let collapsed = computed(move || sidebar_collapsed(app));
    view()
        .flex_1()
        .min_h(px(0.0))
        .bg(color(Token::Shell))
        .child(when(
            move || {
                !matches!(app.live_page(), Some(crate::model::Live::Session(_)))
                    || app
                        .feed
                        .with(|feed| feed.link == crate::model::daemon::Link::Down)
            },
            move || top_bar(app, workspace, collapsed),
        ))
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
        .border_b_color(color(Token::ShellRule))
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
        .child(scope_switcher(app, ScopeFace::Bar))
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

/// The scope switcher, then the places this workspace has with Activity's pages under
/// Activity while one of them shows; at the foot Go to and Settings with its sections
/// under it while it is selected, and this device.
fn sidebar(
    app: AppState,
    current: Memo<Area>,
    shown: Memo<Vec<Area>>,
    workspace: Memo<Option<Workspace>>,
) -> AnyElement {
    let rows = dynamic(move || {
        let shown = shown.get();
        untracked(move || {
            view()
                .gap(px(2.0))
                .children(shown.into_iter().map(|area| place_row(app, current, area)))
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
        .child(scope_switcher(app, ScopeFace::Sidebar))
        .child(scroll().flex_1().min_h(px(0.0)).child(rows))
        .child(agents_opener(app))
        .child(palette_opener(app))
        .child(place_row(app, current, Area::Settings))
        .child(when(
            move || current.get() == Area::Settings,
            move || settings_sections(app),
        ))
        .child(dynamic(move || {
            workspace.get().map(|ws| device_footer(app, ws))
        }))
        .into_element()
}

/// Folds or unfolds the sidebar, drawn as a window with its sidebar (a back chevron would read
/// as leaving the page); it says whether the labels are shown. test_id "sidebar-toggle".
pub(crate) fn sidebar_toggle(app: AppState, collapsed: Memo<bool>) -> AnyElement {
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
    current: Memo<Area>,
    shown: Memo<Vec<Area>>,
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

/// The folded sidebar: the scope switcher, each place as its icon with its count, Activity's
/// pages as short names under Activity while one shows; at the foot the palette and
/// Settings with its sections as short names. Every control keeps its full name (and a place
/// its count) for assistive technology, shows its name and key in a hint while the pointer
/// rests on it, and the palette lists them all.
fn rail(app: AppState, current: Memo<Area>, shown: Memo<Vec<Area>>) -> AnyElement {
    let rows = dynamic(move || {
        let shown = shown.get();
        untracked(move || {
            view()
                .items_center()
                .gap(px(2.0))
                .children(shown.into_iter().map(|area| rail_place(app, current, area)))
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
        .child(scope_switcher(app, ScopeFace::Rail))
        .child(scroll().flex_1().min_h(px(0.0)).child(rows))
        .child(
            rail_button(
                app,
                Hint {
                    name: AGENTS_LABEL,
                    key: Some(AGENTS_KEY.to_owned()),
                },
                move || SharedString::from(AGENTS_LABEL),
                "agents-open".into(),
                move || app.switcher.get(),
                move || app.open_agents(),
            )
            .child(icon(Icon::Agents, 18.0, color(Token::ShellInk2))),
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
            .child(icon(Icon::Search, 18.0, color(Token::ShellInk2))),
        )
        .child(rail_place(app, current, Area::Settings))
        .child(when(
            move || current.get() == Area::Settings,
            move || rail_sections(app),
        ))
        .into_element()
}

/// A place in the rail: its icon and count, and under Activity, while one of its pages
/// shows, the pages as short names.
fn rail_place(app: AppState, current: Memo<Area>, area: Area) -> AnyElement {
    let ink = color(Token::ShellInk);
    let button = rail_button(
        app,
        Hint {
            name: area.label(),
            key: Some(area_key(area).to_owned()),
        },
        area_spoken(app, area),
        format!("nav-{}", area.slug()),
        move || current.get() == area,
        move || app.go_area(area),
    )
    .child(icon(Icon::area(area), 18.0, ink))
    .children(area.counted().map(|counted| {
        view()
            .absolute()
            .top(px(1.0))
            .right(px(0.0))
            .child(count(app, counted))
    }))
    .into_element();
    if area != Area::Activity {
        return button;
    }
    view()
        .items_center()
        .gap(px(2.0))
        .child(button)
        .child(when(
            move || current.get() == Area::Activity,
            move || rail_pages(app),
        ))
        .into_element()
}

/// Activity's pages in the rail, each its short name under its full one for assistive
/// technology.
fn rail_pages(app: AppState) -> AnyElement {
    let pages = computed(move || activity_pages(app));
    dynamic(move || {
        let pages = pages.get();
        untracked(move || {
            view()
                .items_center()
                .gap(px(2.0))
                .children(pages.into_iter().map(|page| {
                    let selected = move || app.destination() == page;
                    let name = page.label();
                    rail_button(
                        app,
                        Hint { name, key: None },
                        move || SharedString::from(name),
                        format!("nav-{}", page.slug()),
                        selected,
                        move || app.go(page),
                    )
                    .h(px(control_height(row_height(app.form))))
                    .child(
                        text(page_short(page))
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
        })
    })
}

/// An Activity page's short name in the folded sidebar.
const fn page_short(page: Destination) -> &'static str {
    match page {
        Destination::Timeline => "Time",
        Destination::Usage => "Use",
        Destination::Artifacts => "Files",
        Destination::System => "Map",
        _ => "Feed",
    }
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

/// The Agents switcher's row above Go to, with its chord.
fn agents_opener(app: AppState) -> AnyElement {
    let ink = color(Token::ShellInk);
    let quiet = color(Token::ShellInk2);
    view()
        .h(px(control_height(row_height(app.form))))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(8.0))
        .rounded(px(RADIUS_CONTROL))
        .cursor_pointer()
        .transition_colors()
        .duration(120.0_f32)
        .hover(move |s| s.bg(ink.opacity(0.1)))
        .active(move |s| s.bg(ink.opacity(0.16)))
        .focus_visible(focus_ring)
        .role(AccessibilityRole::Button)
        .label(AGENTS_LABEL)
        .test_id("agents-open")
        .on_press(move || app.open_agents())
        .child(icon(Icon::Agents, 16.0, quiet))
        .child(
            text(AGENTS_LABEL)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(text_size(app.form)))
                .text_color(quiet),
        )
        .child(key_hint(AGENTS_KEY))
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
        .gap(px(10.0))
        .px(px(8.0))
        .rounded(px(RADIUS_CONTROL))
        .cursor_pointer()
        .transition_colors()
        .duration(120.0_f32)
        .hover(move |s| s.bg(ink.opacity(0.1)))
        .active(move |s| s.bg(ink.opacity(0.16)))
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

/// A place's row: its icon, name, count and key. Activity brings its pages under it while one
/// of them shows.
fn place_row(app: AppState, current: Memo<Area>, area: Area) -> AnyElement {
    let trailing = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .children(area.counted().map(|counted| count(app, counted)))
        .child(
            text(area_key(area))
                .w(px(10.0))
                .font_family(FONT_MONO)
                .text_size(px(text_size(app.form) - 3.0))
                .text_color(color(Token::ShellInk2))
                .accessibility_elements_hidden(true),
        )
        .into_element();
    let row = nav_row_icon(
        app,
        Some(Icon::area(area)),
        area.label(),
        area_spoken(app, area),
        format!("nav-{}", area.slug()),
        move || current.get() == area,
        Some(trailing),
        move || app.go_area(area),
    );
    if area != Area::Activity {
        return row;
    }
    view()
        .gap(px(2.0))
        .child(row)
        .child(when(
            move || current.get() == Area::Activity,
            move || activity_rows(app),
        ))
        .into_element()
}

/// Activity's pages under its row, set in under the places' labels.
fn activity_rows(app: AppState) -> AnyElement {
    let pages = computed(move || activity_pages(app));
    dynamic(move || {
        let pages = pages.get();
        untracked(move || {
            view()
                .pl(px(SECTION_INDENT))
                .gap(px(2.0))
                .children(pages.into_iter().map(|page| {
                    nav_row(
                        app,
                        page.label(),
                        format!("nav-{}", page.slug()),
                        move || app.destination() == page,
                        None,
                        move || app.go(page),
                    )
                }))
                .into_element()
        })
    })
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
        .border_t_color(color(Token::ShellRule))
        .child(
            text(format!("{THIS_DEVICE} "))
                .text_size(px(small))
                .text_color(color(Token::ShellInk2))
                .children(ws.device.map(|name| {
                    text(name.into_owned())
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(Token::ShellInk))
                })),
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
                .label(move || SharedString::from(app.destination().label()))
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .style_when(move || stale(app), |s| s.opacity(STALE_OPACITY))
                .child(screens::content(app)),
        )
        .child(ask_surface(app))
        .child(palette(app))
        .child(crate::screens::agents::rewind_sheet(app))
        .child(crate::screens::agents::remove_sheet(app))
        .child(crate::screens::agents::start_sheet(app))
        .child(confirmation(app))
        .child(crate::shell::refusal(app))
        .child(crate::shell::switcher(app))
        .child(crate::ui::kit::more::overlays(app))
        .child(toast(app))
        .into_element()
}

fn focus_ring(s: StyleRefinement) -> StyleRefinement {
    s.outline(px(FOCUS_RING))
        .outline_color(color(Token::Focus))
        .outline_offset(px(1.0))
}
