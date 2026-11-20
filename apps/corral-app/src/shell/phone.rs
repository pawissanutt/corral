//! The phone shell (iOS, Android, phone browsers and small tablets): the health strip under the
//! status bar, the selected tab's page stack, the floating Ask button, and the tabs at the
//! bottom, or the top page's own bar in their place. iOS and Android show the system's tab bar;
//! everywhere else it takes no room, so the shell draws its own.
//!
//! Each tab is one stack: its root (a destination's screen, or the More list), then the pages
//! pushed over it. `nav` keeps the stack and the model's pages in step both ways. A phone held
//! sideways has little height, so its chrome closes up: the health strip keeps to one row and
//! the Ask button gives way to a page's own bar.

mod nav;

use nsh_base::{nav_page, nav_stack, tab, tabs};
use nsh_core::{
    element::StyleRefinement,
    mobile::{TabBarItem, tab_bar},
    platform::Os,
    prelude::*,
};

use self::nav::{Entry, Nav};
use super::{activity_pages, confirmation, lock, refusal, stale, toast};
use crate::{
    model::{
        Area, Availability, DataState, Destination, Live, Overlay, PhoneTab, Route,
        SettingsSection,
        nav::{ASK_BUTTON, ASK_BUTTON_LABEL, phone_tabs},
    },
    screens::{self, PhoneBar, PhonePage, settings},
    state::AppState,
    ui::{
        ask, atoms, composer,
        health_line::{HealthLayout, health_line},
        icons::{Icon, icon},
        kit::{
            notice,
            page::{head, pushed_head, search},
            page_phone::{follow_scroll, page_scroll, short_window},
            rows::list_row,
            scope::{ScopeFace, scope_switcher},
            text::plain,
        },
        palette,
        tokens::{FONT_TEXT, Token, color},
    },
};

/// Room left under content the shell scrolls, so its last row stays pressable above the Ask
/// button (`.phone:has(.askfab) .pbody.pv > .stalefx`).
const ASK_CLEARANCE: f32 = 64.0;
/// The floating Ask button (`.askfab`): height, its distance from the page's trailing and bottom
/// edges, its paddings, icon and label.
const ASK_HEIGHT: f32 = 46.0;
const ASK_RIGHT: f32 = 14.0;
const ASK_BOTTOM: f32 = 12.0;
const ASK_ICON: f32 = 20.0;
const ASK_LABEL: f32 = 15.0;
/// Narrower than this, the Ask button is its icon alone in a 46 pt circle, so it covers less of
/// the rows' trailing figures.
const ASK_NARROW: f32 = 380.0;
/// What the mock does to the board while corrald is down or this device offline (`.stalefx`).
const STALE_SATURATION: f32 = 0.35;
const STALE_OPACITY: f32 = 0.78;
/// A drawn tab (`.ptab button`): height, icon and label.
const TAB_HEIGHT: f32 = 50.0;
const TAB_ICON: f32 = 24.0;
const TAB_LABEL: f32 = 10.5;

pub fn shell(app: AppState) -> AnyElement {
    let nav = Nav::start(app);
    let bar = if matches!(os(), Os::Ios | Os::Android) {
        system_tabs(nav)
    } else {
        app_tabs(nav)
    };
    view()
        .flex_1()
        .relative()
        .bg(color(Token::Bg))
        .child(health_strip(app))
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .relative()
                .child(dynamic(move || {
                    let tab = nav.tab.get();
                    let _data = app.data.get();
                    // Rebuilt when the tab changes, not when what a page reads does.
                    untracked(move || tab_stack(nav, tab))
                })),
        )
        .child(
            view()
                .style_when(move || nav.covered(), |s| s.hidden())
                .child(bar),
        )
        .child(toast(app))
        .child(composer::ask_surface(app))
        .child(ask::share(app))
        .child(palette::palette(app))
        .child(crate::screens::agents::rewind_sheet(app))
        .child(crate::screens::agents::remove_sheet(app))
        .child(crate::screens::agents::start_sheet(app))
        .child(confirmation(app))
        .child(refusal(app))
        .child(crate::ui::kit::more::overlays(app))
        .child(when(
            move || app.lock.get(),
            move || {
                lock::lock_screen(app, move |live: Live| {
                    app.lock.set(false);
                    app.open_live(live);
                })
            },
        ))
        .into_element()
}

/// The head under the status bar: the health line (the two-row strip, or the one-row line
/// while the window is short, which keeps the reach path in it), then the scope switcher and
/// the gear that opens Settings.
fn health_strip(app: AppState) -> AnyElement {
    let short = computed(short_window);
    let health = dynamic(move || {
        if short.get() {
            untracked(|| {
                view()
                    .flex_row()
                    .child(health_line(app, HealthLayout::Bar))
                    .into_element()
            })
        } else {
            untracked(|| health_line(app, HealthLayout::Strip))
        }
    });
    let gear = view()
        .test_id("nav-settings")
        .size(px(atoms::TOUCH_TARGET))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(Area::Settings.label())
        .active(|s| s.opacity(0.7))
        .focus_visible(ring)
        .on_press(move || app.go(Destination::Settings))
        .child(icon(Icon::Settings, 20.0, color(Token::ShellInk)));
    view()
        .bg(color(Token::Shell))
        .pt_safe_area_plus(px(2.0))
        .pb(px(6.0))
        .pl_safe_area_plus(px(16.0))
        .pr_safe_area_plus(px(8.0))
        .style_when(move || short.get(), |s| s.pb(px(2.0)))
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(view().flex_1().min_w(px(0.0)).child(health))
        .child(scope_switcher(app, ScopeFace::Bar))
        .child(gear)
        .into_element()
}

/// The tab's stack: its root, then a page for each entry. Each page is keyed by depth and route,
/// so the pages below keep their state while others cover them.
fn tab_stack(nav: Nav, tab: PhoneTab) -> AnyElement {
    nav_stack()
        .flex_1()
        .child(root(nav, tab))
        .child(each(
            move || {
                nav.stack
                    .with(|s| s.iter().cloned().enumerate().collect::<Vec<_>>())
            },
            |item: &(usize, Entry)| nav::key_of(item.0, &item.1),
            move |item, _| {
                let (depth, entry) = item.get_untracked();
                pushed_page(nav, depth, entry)
            },
        ))
        .into_element()
}

/// Hides a page's part from sight, presses, focus and screen readers while a page above covers
/// it, once that page has slid in; stack `position` 0 is the tab's root.
fn unless_covered(nav: Nav, position: usize, part: View) -> View {
    let covered = move || !nav.drawn(position);
    part.style_when(covered, |s| s.invisible())
        .accessibility_elements_hidden(covered)
}

/// A tab's root carries its own large title on every platform, so no header shows over it; the
/// connection banner sits under that title.
fn root(nav: Nav, tab: PhoneTab) -> Screen {
    let app = nav.app;
    let body = match tab.destination() {
        Some(destination) => frame(
            app,
            None,
            screens::destination(app, destination, true),
            None,
            true,
            false,
            false,
        ),
        None => activity_list(nav),
    };
    nav_page()
        .key(tab.slug())
        .title(tab.label())
        .header_visible(false)
        .child(unless_covered(nav, 0, view().flex_1().min_h(px(0.0))).child(body))
}

/// The page an entry shows, and whether it starts with its own large-title head (which draws the
/// connection banner under the title). Against corrald or the labelled preview's feed a record
/// opens its domain's page, a destination its screen (or the frame saying why it is not served).
/// Without a connection every page is its destination's frame, and Settings keeps its sections
/// (browser pairing lives in Connection).
fn page_of(app: AppState, entry: Entry) -> (PhonePage, bool) {
    let pushed = |page: PhonePage| (page, false);
    let connected = untracked(|| app.data.get()) != DataState::Disconnected;
    match entry {
        Entry::Page(Route::Live(live))
            if connected && untracked(|| app.available(live.owner())) == Availability::Ready =>
        {
            pushed(screens::live_phone(app, live))
        }
        Entry::Page(route) => destination_page(app, route.owner()),
        Entry::Destination(destination) => destination_page(app, destination),
        Entry::Section(section) => pushed(settings::section_phone(app, section)),
    }
}

/// A destination pushed over a tab's root: its screen, which starts with its large-title head,
/// or the frame that says why it is unavailable.
fn destination_page(app: AppState, destination: Destination) -> (PhonePage, bool) {
    let page = PhonePage {
        title: destination.label().to_owned(),
        body: screens::destination(app, destination, false),
        trailing: None,
        bar: None,
        ask: true,
        scroll: false,
    };
    (page, true)
}

/// A pushed page: the head with its back control naming the page under it (the system's on iOS
/// and Android), then the page in its frame.
fn pushed_page(nav: Nav, depth: usize, entry: Entry) -> Screen {
    let app = nav.app;
    let (
        PhonePage {
            title,
            body,
            trailing,
            bar,
            ask,
            scroll,
        },
        headed,
    ) = page_of(app, entry.clone());
    nav.register(depth, &entry, &title, bar.as_ref().map(|bar| bar.shown));
    let position = depth + 1;
    let title: Reactive<SharedString> = SharedString::from(title).into_reactive();
    // Configuration with unsaved edits stays on a back press or swipe until the confirm sheet
    // has asked; every other page pops at once.
    let config = matches!(entry, Entry::Section(SettingsSection::Config));
    let holds = computed(move || config && app.settings.with(|state| state.config.dirty()));
    let page = nav_page()
        .title(title.clone())
        .header_style_with(|s| {
            s.bg(color(Token::Bg))
                .text_color(color(Token::Ink))
                .tint_color(color(Token::Ink2))
        })
        .pop_on_back(move || !holds.get())
        // The browser's back and Escape slide this page out before it is dismissed: the page
        // under it shows again first.
        .on_back_press(move || {
            if holds.get_untracked() {
                app.asks_before_leaving_config();
            } else {
                nav.uncover(depth);
            }
        })
        .on_dismiss(move || nav.back_to(depth));
    let page = if matches!(os(), Os::Ios | Os::Android) {
        match trailing {
            Some(trailing) => page.header_right(trailing),
            None => page,
        }
    } else {
        page.header(unless_covered(nav, position, view()).child(pushed_head(
            app,
            title,
            move || nav.back_label(depth),
            move || nav.back_to(depth),
            trailing,
        )))
    };
    page.child(
        unless_covered(nav, position, view().flex_1().min_h(px(0.0)))
            .child(frame(app, None, body, bar, ask, scroll, !headed)),
    )
}

/// What every page sits in: an optional head, the connection banner when the page has no
/// large-title head of its own to draw it under, the body (faded while the board is stale,
/// scrolled here when the page asks for it), the floating Ask button over its bottom edge, and
/// the page's own bar under it. The Ask button steps aside while the reader moves down the rows,
/// and on a short window while the page's bar shows.
fn frame(
    app: AppState,
    head: Option<AnyElement>,
    body: AnyElement,
    bar: Option<PhoneBar>,
    ask: bool,
    scroll: bool,
    banner: bool,
) -> AnyElement {
    let content = if scroll {
        follow_scroll(app.form, nsh_core::prelude::scroll())
            .flex_1()
            .min_h(px(0.0))
            .child(view().when(ask, |v| v.pb(px(ASK_CLEARANCE))).child(body))
            .into_element()
    } else {
        body
    };
    let bar_shown = bar.as_ref().map(|bar| bar.shown);
    let tucked = computed(move || {
        page_scroll().with(|s| s.reading)
            || (short_window() && bar_shown.is_some_and(|shown| shown.get()))
    });
    view()
        .flex_1()
        .min_h(px(0.0))
        .bg(color(Token::Bg))
        .children(head)
        .when(banner, |v| v.child(notice::connection(app)))
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .relative()
                .pl_safe_area()
                .pr_safe_area()
                .child(
                    view()
                        .flex_1()
                        .min_h(px(0.0))
                        .style_when(
                            move || stale(app),
                            |s| s.saturate(STALE_SATURATION).opacity(STALE_OPACITY),
                        )
                        .child(content),
                )
                .when(ask, |v| {
                    v.child(when(
                        move || !app.is_open(Overlay::Ask) && !app.share.get() && !tucked.get(),
                        move || ask_button(app),
                    ))
                }),
        )
        .children(bar.map(page_bar))
        .into_element()
}

/// A page's own bar in the tab bar's place, while it shows: riding on the software keyboard
/// while it is up (a docked composer's field stays in sight), clear of the home indicator and
/// of a sideways notch otherwise.
fn page_bar(bar: PhoneBar) -> AnyElement {
    let shown = bar.shown;
    view()
        .flex_none()
        .bg(color(Token::Bg))
        .style_when(move || !shown.get(), |s| s.hidden())
        .pb_keyboard()
        .child(
            view()
                .pl_safe_area()
                .pr_safe_area()
                .pb(move || {
                    if keyboard_height() > px(0.0) {
                        px(0.0)
                    } else {
                        safe_area().bottom
                    }
                })
                .child(bar.view),
        )
        .into_element()
}

/// The Activity tab's root: the large title with search, then a row per page this workspace
/// has (Feed, Timeline, Usage, Artifacts) with its icon and name, each pushing its page.
fn activity_list(nav: Nav) -> AnyElement {
    let app = nav.app;
    let listed = computed(move || activity_pages(app));
    let list = dynamic(move || {
        let pages = listed.get();
        untracked(move || {
            view()
                .mt(px(6.0))
                .border_t_1()
                .border_t_color(color(Token::Rule))
                .children(pages.into_iter().map(|page| page_row(nav, page)))
                .into_element()
        })
    });
    frame(
        app,
        Some(head(app, Area::Activity.label(), None, vec![search(app)])),
        follow_scroll(app.form, nsh_core::prelude::scroll())
            .test_id("activity-list")
            .flex_1()
            .min_h(px(0.0))
            .child(view().pb(px(ASK_CLEARANCE)).child(list))
            .into_element(),
        None,
        true,
        false,
        false,
    )
}

/// One row of Activity's list (`.prow` at 28 / 1fr / 20 columns, at least 56 pt high).
fn page_row(nav: Nav, page: Destination) -> AnyElement {
    let app = nav.app;
    list_row(
        app,
        page.label(),
        format!("nav-{}", page.slug()),
        || false,
        move || nav.open_page(page),
        vec![
            view()
                .w(px(28.0))
                .flex_shrink_0()
                .child(icon(Icon::destination(page), 20.0, color(Token::Ink)))
                .into_element(),
            plain(app, page.label())
                .truncate()
                .flex_1()
                .min_w(px(0.0))
                .into_element(),
            icon(Icon::Chevron, 16.0, color(Token::Ink3)),
        ],
    )
}

/// The keyboard focus ring.
fn ring(s: StyleRefinement) -> StyleRefinement {
    s.outline(px(2.0))
        .outline_color(color(Token::Focus))
        .outline_offset(px(2.0))
}

/// Ask floats at the trailing edge just above the tab bar or the page's bar (`.askfab`); on a
/// narrow phone it is the icon alone, named the same for screen readers.
fn ask_button(app: AppState) -> AnyElement {
    let ink = color(Token::BtnInk);
    let narrow = move || window().width.0 < ASK_NARROW;
    view()
        .absolute()
        .right(px(ASK_RIGHT))
        .bottom(px(ASK_BOTTOM))
        .mr_safe_area()
        .h(px(ASK_HEIGHT))
        .pl(px(14.0))
        .pr(px(18.0))
        .style_when(narrow, |s| s.px(px((ASK_HEIGHT - ASK_ICON) / 2.0)))
        .flex_row()
        .items_center()
        .gap(px(7.0))
        .rounded_full()
        .bg(color(Token::Btn))
        .shadow_lg()
        .shadow_color(color(Token::Shell))
        .role(AccessibilityRole::Button)
        .label(ASK_BUTTON_LABEL)
        .test_id("ask-open")
        .cursor_pointer()
        .active(|s| s.opacity(0.85))
        .focus_visible(ring)
        .on_press(move || app.open_ask())
        .child(icon(Icon::Ask, ASK_ICON, ink))
        .child(
            text(ASK_BUTTON)
                .font_family(FONT_TEXT)
                .text_size(px(ASK_LABEL))
                .font_weight(FontWeight::BOLD)
                .text_color(ink)
                .style_when(narrow, |s| s.hidden()),
        )
        .into_element()
}

/// Tracked: the tabs this workspace has (`nav::phone_tabs`): the four places, or without
/// projects Home (the landing), Sessions and Activity.
fn tab_items(nav: Nav) -> Memo<Vec<PhoneTab>> {
    computed(move || nav.app.feed.with(phone_tabs))
}

/// iOS and Android: the system's bar, in flow so it takes its own height, the home indicator's
/// or navigation bar's inset included.
fn system_tabs(nav: Nav) -> AnyElement {
    let ios = matches!(os(), Os::Ios);
    let items = tab_items(nav);
    dynamic(move || {
        let shown = items.get();
        untracked(move || {
            let bar: Vec<TabBarItem> = shown
                .iter()
                .map(|tab| {
                    let entry = TabBarItem::new(tab.label());
                    // Android icons are the app's own drawables, which this build has none of.
                    if ios { entry.icon(symbol(*tab)) } else { entry }
                })
                .collect();
            tab_bar()
                .items(bar)
                .selected_index(move || {
                    let now = nav.tab.get();
                    items.with(|items| {
                        items
                            .iter()
                            .position(|tab| *tab == now)
                            .and_then(|at| u32::try_from(at).ok())
                            .unwrap_or(0)
                    })
                })
                .on_change(move |index: &u32| {
                    let tab = items.with_untracked(|items| {
                        usize::try_from(*index)
                            .ok()
                            .and_then(|at| items.get(at))
                            .copied()
                    });
                    if let Some(tab) = tab {
                        nav.select(tab);
                    }
                })
                .into_element()
        })
    })
}

/// The SF Symbol of a tab on iOS.
const fn symbol(tab: PhoneTab) -> &'static str {
    match tab {
        PhoneTab::Home => "house",
        PhoneTab::Tasks => "list.bullet.rectangle",
        PhoneTab::Sessions => "terminal",
        PhoneTab::Activity => "waveform.path.ecg",
    }
}

/// Every other platform: the tabs in the mock's look (`.ptab`), above the bottom inset, on the
/// page's ground behind a hairline.
fn app_tabs(nav: Nav) -> AnyElement {
    let items = tab_items(nav);
    dynamic(move || {
        let shown = items.get();
        untracked(move || {
            tabs()
                .flex_row()
                .bg(color(Token::Bg).opacity(0.92))
                .border_t_1()
                .border_t_color(color(Token::Rule))
                .pt(px(6.0))
                .px(px(4.0))
                .pb_safe_area()
                .pl_safe_area()
                .pr_safe_area()
                // The selected tab reads by its ink and weight, as in the mock; no bar marks it.
                .indicator_style(|s| s.h(px(0.0)).opacity(0.0))
                .children(shown.into_iter().map(|item| app_tab(nav, item)))
                .into_element()
        })
    })
}

/// One drawn tab: the selected one has the strong ink and a bold label, so colour is never the
/// only cue.
fn app_tab(nav: Nav, item: PhoneTab) -> AnyElement {
    let app = nav.app;
    let label = item.label();
    let selected = computed(move || nav.tab.get() == item);
    let ink = move || {
        if selected.get() {
            color(Token::Ink)
        } else {
            color(Token::Ink3)
        }
    };
    let glyph = Icon::phone_tab(item);
    let home_tab = item == PhoneTab::Home;
    let glyph = view()
        .relative()
        .size(px(TAB_ICON))
        .child(dynamic(move || icon(glyph, TAB_ICON, ink())))
        .when(home_tab, |v| v.child(needs_badge(app)));
    // The test id sits on the icon and label; a press there reaches the tab.
    let face = view()
        .test_id(&format!("tab-{}", item.slug()))
        .items_center()
        .gap(px(3.0))
        .child(glyph)
        .child(
            text(label)
                .font_family(FONT_TEXT)
                .text_size(px(TAB_LABEL))
                .style_when(selected, |s| s.font_weight(FontWeight::BOLD))
                .text_color(ink),
        );
    // The tab's own name, so it is announced even while a page's bar hides the face; Home
    // carries its needs-you count, which the badge otherwise announces alone.
    let name = move || {
        Some(SharedString::from(
            match (home_tab, app.snapshot().count_words(Destination::Home)) {
                (true, Some(words)) => format!("{label}, {words}"),
                _ => label.to_owned(),
            },
        ))
    };
    tab()
        .selected(selected)
        .accessibility_label(name)
        .on_press(move |_: &()| nav.select(item))
        .flex_1()
        .h(px(TAB_HEIGHT))
        .items_center()
        .cursor_pointer()
        .active(|s| s.opacity(0.7))
        .focus_visible(ring)
        .child(face)
        .into_element()
}

/// Home's needs-you count as a badge at the icon's top trailing corner (`.ptab .badge`).
fn needs_badge(app: AppState) -> AnyElement {
    view()
        .absolute()
        .top(px(-2.0))
        .left(px(TAB_ICON / 2.0 + 6.0))
        .child(atoms::count(app, Destination::Home))
        .into_element()
}
