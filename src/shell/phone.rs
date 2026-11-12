//! The phone shell (iOS, Android, phone and tablet browsers): the health
//! strip under the status bar, the selected tab's content, the floating Ask
//! button, and the tabs at the bottom. iOS and Android show the system's tab
//! bar; everywhere else it takes no room, so the shell draws its own.
//!
//! The model has no "More" destination, so which tab shows and which pages the
//! More stack holds are this shell's own state. They follow the model when its
//! destination changes (the palette, a restored destination) and leave it
//! alone otherwise, so opening Ask never moves the user off a tab.

use std::cell::Cell;

use nsh_base::{nav_page, nav_stack, tab, tabs};
use nsh_core::{
    element::StyleRefinement,
    mobile::{TabBarItem, tab_bar},
    platform::Os,
    prelude::*,
};

use crate::{
    model::{Destination, PhoneTab, SettingsSection},
    screens,
    state::AppState,
    ui::{
        atoms::{self, HealthLayout},
        composer,
        icons::{Icon, icon},
        palette,
        tokens::{FONT_TEXT, Token, color, row_height, text_size},
    },
};

/// The floating Ask button's height and its distance from the content's edges.
const ASK_HEIGHT: f32 = 52.0;
const ASK_INSET: f32 = 16.0;
/// Room left under scrolling content so its last row stays pressable above
/// the Ask button.
const ASK_CLEARANCE: f32 = ASK_HEIGHT + 2.0 * ASK_INSET;
/// Cards and grouped lists; controls.
const RADIUS_CARD: f32 = 14.0;
const RADIUS_CONTROL: f32 = 10.0;
/// The minimum touch target.
const TARGET: f32 = 44.0;
/// A drawn tab's icon and the label under it, as in the mock's tab bar.
const TAB_ICON: f32 = 22.0;
const TAB_LABEL: f32 = 12.0;

/// A page pushed on the More stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Page(Destination),
    Section(SettingsSection),
}

/// Where a pushed page's back control leads.
#[derive(Clone, Copy)]
enum Back {
    More,
    Settings,
}

pub fn shell(app: AppState) -> AnyElement {
    let start = untracked(|| app.destination());
    let current = signal(start.phone_tab());
    let routes = signal(match start.phone_tab() {
        PhoneTab::More => vec![Route::Page(start)],
        _ => Vec::new(),
    });
    follow_navigation(app, current, routes);
    let bar = if matches!(os(), Os::Ios | Os::Android) {
        system_tabs(app, current, routes)
    } else {
        app_tabs(app, current, routes)
    };
    view()
        .flex_1()
        .relative()
        .bg(color(Token::Bg))
        .child(health_strip(app))
        .child(
            view()
                .flex_1()
                .relative()
                .child(dynamic(move || {
                    let tab = current.get();
                    // Rebuilt when the tab changes, not when what a page reads does.
                    untracked(move || tab_content(app, tab, routes))
                }))
                .child(ask_button(app)),
        )
        .child(bar)
        .child(composer::ask_surface(app))
        .child(palette::palette(app))
        .into_element()
}

/// Moves the tab and the More stack to the model's destination when it
/// changes, and pushes a Settings section when the section changes while
/// Settings is shown.
fn follow_navigation(app: AppState, current: Signal<PhoneTab>, routes: Signal<Vec<Route>>) {
    let place = computed(move || app.nav.with(|nav| (nav.destination, nav.settings)));
    let seen = Cell::new(place.get_untracked());
    effect(move || {
        let (destination, section) = place.get();
        let (was, was_section) = seen.replace((destination, section));
        let section_moved = destination == Destination::Settings && section != was_section;
        if destination == was && !section_moved {
            return;
        }
        let tab = destination.phone_tab();
        current.set(tab);
        if tab != PhoneTab::More {
            return;
        }
        if section_moved {
            routes.set(vec![Route::Page(destination), Route::Section(section)]);
        } else if routes.with_untracked(|r| r.first() != Some(&Route::Page(destination))) {
            routes.set(vec![Route::Page(destination)]);
        }
    });
}

/// The user chose `tab`. Choosing More again goes back to its list, as a
/// second tap on a selected tab does on the phones.
fn select(app: AppState, current: Signal<PhoneTab>, routes: Signal<Vec<Route>>, tab: PhoneTab) {
    if tab == PhoneTab::More && current.get_untracked() == PhoneTab::More {
        routes.set(Vec::new());
        return;
    }
    current.set(tab);
    match tab.destination() {
        Some(destination) => app.go(destination),
        None => match routes.with_untracked(|r| r.last().copied()) {
            Some(Route::Page(destination)) => app.go(destination),
            Some(Route::Section(section)) => app.go_settings(section),
            None => {}
        },
    }
}

fn health_strip(app: AppState) -> AnyElement {
    view()
        .bg(color(Token::Shell))
        .border_b_1()
        .border_b_color(color(Token::Rule))
        .pt_safe_area_plus(px(6.0))
        .pb(px(8.0))
        .pl_safe_area_plus(px(16.0))
        .pr_safe_area_plus(px(16.0))
        .child(atoms::health_line(app, HealthLayout::Strip))
        .into_element()
}

fn tab_content(app: AppState, tab: PhoneTab, routes: Signal<Vec<Route>>) -> AnyElement {
    match tab.destination() {
        Some(destination) => scroll()
            .flex_1()
            .child(
                view()
                    .pb(px(ASK_CLEARANCE))
                    .child(screens::destination(app, destination)),
            )
            .into_element(),
        None => more_stack(app, routes),
    }
}

/// The More list with the pages pushed over it. Back (the header's control,
/// the swipe, Android's back, the browser's) is the stack's own; a page the
/// user left natively takes its route and those above it along.
fn more_stack(app: AppState, routes: Signal<Vec<Route>>) -> AnyElement {
    nav_stack()
        .flex_1()
        .child(more_page(app, routes))
        .child(each(
            move || routes.get(),
            |route: &Route| format!("{route:?}"),
            move |route, index| {
                pushed_page(app, routes, route.get_untracked())
                    .on_dismiss(move || routes.update(|r| r.truncate(index.get_untracked())))
            },
        ))
        .into_element()
}

fn more_page(app: AppState, routes: Signal<Vec<Route>>) -> Screen {
    let rows = PhoneTab::MORE
        .iter()
        .map(|&destination| {
            let trailing = view()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .child(atoms::count(app, destination))
                .child(icon(Icon::Chevron, 16.0, color(Token::Ink3)))
                .into_element();
            atoms::nav_row_icon(
                app,
                Icon::destination(destination),
                destination.label(),
                format!("nav-{}", destination.slug()),
                || false,
                Some(trailing),
                move || {
                    routes.set(vec![Route::Page(destination)]);
                    app.go(destination);
                },
            )
        })
        .collect();
    // The list carries its own large title on every platform, as the other
    // tabs' pages do; the system header shows only over pushed pages.
    nav_page()
        .key("more")
        .title("More")
        .header_visible(false)
        .child(
            scroll().flex_1().child(
                view()
                    .px(px(16.0))
                    .pt(px(20.0))
                    .pb(px(ASK_CLEARANCE))
                    .gap(px(16.0))
                    .child(
                        text("More")
                            .role(AccessibilityRole::Header)
                            .font_family(FONT_TEXT)
                            .text_size(px(30.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::Ink)),
                    )
                    .child(card(rows)),
            ),
        )
}

fn pushed_page(app: AppState, routes: Signal<Vec<Route>>, route: Route) -> Screen {
    let (title, back, body) = match route {
        // Only the title heads the page, sized to itself, so the sections sit right under it.
        Route::Page(Destination::Settings) => (
            Destination::Settings.label(),
            Back::More,
            view()
                .child(screens::settings_root(app))
                .child(
                    view()
                        .px(px(16.0))
                        .pt(px(8.0))
                        .child(section_list(app, routes)),
                )
                .into_element(),
        ),
        Route::Page(destination) => (
            destination.label(),
            Back::More,
            screens::destination(app, destination),
        ),
        Route::Section(section) => (
            section.label(),
            Back::Settings,
            screens::settings_section(app, section),
        ),
    };
    nav_page()
        .title(title)
        .header(back_bar(routes, back))
        .header_style_with(|s| {
            s.bg(color(Token::Bg))
                .text_color(color(Token::Ink))
                .tint_color(color(Token::Ink))
        })
        .child(
            scroll()
                .flex_1()
                .child(view().pb(px(ASK_CLEARANCE)).child(body)),
        )
}

fn section_list(app: AppState, routes: Signal<Vec<Route>>) -> AnyElement {
    let rows = SettingsSection::ALL
        .iter()
        .map(|&section| {
            atoms::nav_row(
                app,
                section.label(),
                format!("settings-{}", section.slug()),
                || false,
                Some(icon(Icon::Chevron, 16.0, color(Token::Ink3))),
                move || {
                    routes.set(vec![
                        Route::Page(Destination::Settings),
                        Route::Section(section),
                    ]);
                    app.go_settings(section);
                },
            )
        })
        .collect();
    card(rows)
}

/// The header of a pushed page on the web (iOS and Android show the
/// system's, with its back button): a back control naming the page below.
fn back_bar(routes: Signal<Vec<Route>>, back: Back) -> AnyElement {
    let (to, label) = match back {
        Back::More => ("More", "Back to More"),
        Back::Settings => ("Settings", "Back to Settings"),
    };
    view()
        .flex_row()
        .items_center()
        .h(px(52.0))
        .px(px(8.0))
        .bg(color(Token::Bg))
        .border_b_1()
        .border_b_color(color(Token::Rule))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .min_h(px(TARGET))
                .px(px(10.0))
                .rounded(px(RADIUS_CONTROL))
                .role(AccessibilityRole::Button)
                .label(label)
                .cursor_pointer()
                .hover(|s| s.bg(color(Token::Hover)))
                .active(|s| s.opacity(0.6))
                .focus_visible(ring)
                .on_press(move || {
                    routes.update(|r| {
                        r.pop();
                    })
                })
                .child(icon(Icon::Back, 20.0, color(Token::Ink)))
                .child(
                    text(to)
                        .font_family(FONT_TEXT)
                        .text_size(px(16.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color(Token::Ink)),
                ),
        )
        .into_element()
}

/// Rows in one rounded group, a hairline between them.
fn card(rows: Vec<AnyElement>) -> AnyElement {
    view()
        .bg(color(Token::Surface))
        .rounded(px(RADIUS_CARD))
        .border_1()
        .border_color(color(Token::Rule))
        .overflow_hidden()
        .children(rows.into_iter().enumerate().map(|(i, row)| {
            view()
                .when(i > 0, |v| v.border_t_1().border_t_color(color(Token::Rule)))
                .child(row)
        }))
        .into_element()
}

/// The keyboard focus ring.
fn ring(s: StyleRefinement) -> StyleRefinement {
    s.outline(px(2.0))
        .outline_color(color(Token::Focus))
        .outline_offset(px(2.0))
}

/// Ask floats over every tab at the trailing edge, above the tab bar.
fn ask_button(app: AppState) -> AnyElement {
    let ink = color(Token::BtnInk);
    view()
        .absolute()
        .right(px(ASK_INSET))
        .bottom(px(ASK_INSET))
        .mr_safe_area()
        .h(px(ASK_HEIGHT))
        .px(px(20.0))
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .rounded_full()
        .bg(color(Token::Btn))
        .shadow_lg()
        .role(AccessibilityRole::Button)
        .label("Ask")
        .test_id("ask-open")
        .cursor_pointer()
        .active(|s| s.opacity(0.85))
        .focus_visible(ring)
        .on_press(move || app.open_ask())
        .child(icon(Icon::Ask, 18.0, ink))
        .child(
            text("Ask")
                .font_family(FONT_TEXT)
                .text_size(px(text_size(app.form)))
                .font_weight(FontWeight::BOLD)
                .text_color(ink),
        )
        .into_element()
}

/// iOS and Android: the system's bar, in flow so it takes its own height,
/// the home indicator's or navigation bar's inset included.
fn system_tabs(app: AppState, current: Signal<PhoneTab>, routes: Signal<Vec<Route>>) -> AnyElement {
    let ios = matches!(os(), Os::Ios);
    let items: Vec<TabBarItem> = PhoneTab::ALL
        .iter()
        .map(|&tab| {
            let item = TabBarItem::new(tab.label());
            // Android icons are the app's own drawables, which this build has none of.
            if ios { item.icon(symbol(tab)) } else { item }
        })
        .collect();
    tab_bar()
        .items(items)
        .selected_index(move || current.get().index())
        .on_change(move |index: &u32| {
            if let Some(tab) = PhoneTab::from_index(*index) {
                select(app, current, routes, tab);
            }
        })
        .into_element()
}

/// The SF Symbol of a tab on iOS.
fn symbol(tab: PhoneTab) -> &'static str {
    match tab {
        PhoneTab::Inbox => "tray",
        PhoneTab::Now => "bolt",
        PhoneTab::Agents => "terminal",
        PhoneTab::Plans => "list.bullet.rectangle",
        PhoneTab::More => "ellipsis",
    }
}

/// Every other platform: the tabs in the app's look, above the bottom inset.
fn app_tabs(app: AppState, current: Signal<PhoneTab>, routes: Signal<Vec<Route>>) -> AnyElement {
    tabs()
        .flex_row()
        .bg(color(Token::Shell))
        .border_t_1()
        .border_t_color(color(Token::Rule))
        .pb_safe_area()
        .pl_safe_area()
        .pr_safe_area()
        .indicator_style(|s| {
            s.top(px(0.0))
                .h(px(3.0))
                .mx(px(22.0))
                .rounded(px(2.0))
                .bg(color(Token::ShellInk))
        })
        .children(
            PhoneTab::ALL
                .iter()
                .map(|&tab| app_tab(app, current, routes, tab)),
        )
        .into_element()
}

/// One drawn tab: the selected one has the bar above it, a bolder label and
/// the strong ink on icon and label, so colour is never the only cue.
fn app_tab(
    app: AppState,
    current: Signal<PhoneTab>,
    routes: Signal<Vec<Route>>,
    which: PhoneTab,
) -> AnyElement {
    let selected = move || current.get() == which;
    let ink = move || {
        if selected() {
            color(Token::ShellInk)
        } else {
            color(Token::ShellInk2)
        }
    };
    let glyph = view()
        .relative()
        .size(px(TAB_ICON))
        .child(dynamic(move || {
            icon(Icon::phone_tab(which), TAB_ICON, ink())
        }))
        .when(which == PhoneTab::Inbox, |v| v.child(inbox_badge(app)));
    // The test id sits on the icon and label; a press there reaches the tab.
    let face = view()
        .test_id(&format!("tab-{}", which.slug()))
        .items_center()
        .gap(px(3.0))
        .pt(px(8.0))
        .pb(px(4.0))
        .child(glyph)
        .child(
            text(which.label())
                .font_family(FONT_TEXT)
                .text_size(px(TAB_LABEL))
                .font_weight(FontWeight::MEDIUM)
                .style_when(selected, |s| s.font_weight(FontWeight::BOLD))
                .text_color(ink),
        );
    tab()
        .selected(selected)
        .on_press(move |_: &()| select(app, current, routes, which))
        .flex_1()
        .min_h(px(row_height(app.form).max(TARGET)))
        .items_center()
        .justify_center()
        .cursor_pointer()
        .active(|s| s.opacity(0.7))
        .focus_visible(ring)
        .child(face)
        .into_element()
}

/// The Inbox count as a badge on the icon's top trailing corner, ringed in
/// the bar's colour so the glyph's strokes do not run into it.
fn inbox_badge(app: AppState) -> AnyElement {
    view()
        .absolute()
        .top(px(-6.0))
        .left(px(12.0))
        .rounded_full()
        .border(px(2.0))
        .border_color(color(Token::Shell))
        .child(atoms::count(app, Destination::Inbox))
        .into_element()
}
