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
use super::{confirmation, destinations, lock, refusal, stale, toast};
use crate::{
    model::{
        Availability, DataState, Destination, Live, Overlay, PhoneTab, ProjectFilter, Route,
        nav::{ASK_BUTTON, ASK_BUTTON_LABEL, NavigationItem, NavigationTarget, phone_navigation},
        projects::{PROJECTS_HEADING, PROJECTS_MORE_SUBTITLE, Projects},
        work::{self, MoreRow},
    },
    screens::{
        self, PhoneBar, PhonePage, agents, assistant, now, plans, projects, review, settings,
    },
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
            text::{dim, plain},
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
        .child(confirmation(app))
        .child(refusal(app))
        .child(crate::screens::now::verb_composer(app))
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

/// The health line under the status bar: the two-row strip, or the one-row line while the
/// window is short, which keeps the reach path in it.
fn health_strip(app: AppState) -> AnyElement {
    let short = computed(short_window);
    view()
        .bg(color(Token::Shell))
        .pt_safe_area_plus(px(2.0))
        .pb(px(6.0))
        .pl_safe_area_plus(px(16.0))
        .pr_safe_area_plus(px(16.0))
        .style_when(move || short.get(), |s| s.pb(px(2.0)))
        .child(dynamic(move || {
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
        }))
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
        None => more_list(nav),
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
            pushed(live_page(app, live))
        }
        Entry::Page(Route::Project(id)) if connected => pushed(projects::project_phone(app, id)),
        Entry::Page(route) => destination_page(app, route.owner()),
        Entry::Destination(destination) => destination_page(app, destination),
        Entry::Section(section) => pushed(settings::section_phone(app, section)),
        Entry::Projects => pushed(projects::phone_list(app)),
    }
}

/// A live record's pushed page, from its domain.
fn live_page(app: AppState, live: Live) -> PhonePage {
    match live {
        Live::Task(id) => now::live_task_phone(app, id),
        Live::Plan(id) => plans::live_plan_phone(app, id),
        Live::Session(id) => agents::live_session_phone(app, id),
        Live::Thread(id) => assistant::live_thread_phone(app, id),
        Live::Review(task) => review::live_review_phone(app, task),
    }
}

/// A destination pushed over More: its screen, which starts with its large-title head, or the
/// frame that says why it is unavailable.
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
    // A served assistant thread's subject moves while it stays pushed (the quick thread rolls
    // to its next session): its head follows the thread; every other page keeps its title.
    let title: Reactive<SharedString> = match &entry {
        Entry::Page(Route::Live(Live::Thread(id)))
            if untracked(|| app.data.get()) != DataState::Disconnected =>
        {
            let live = assistant::live_thread_title(app, id.clone());
            (move || SharedString::from(live.get())).into_reactive()
        }
        _ => SharedString::from(title).into_reactive(),
    };
    let page = nav_page()
        .title(title.clone())
        .header_style_with(|s| {
            s.bg(color(Token::Bg))
                .text_color(color(Token::Ink))
                .tint_color(color(Token::Ink2))
        })
        // The browser's back and Escape slide this page out before it is dismissed: the page
        // under it shows again first.
        .on_back_press(move || nav.uncover(depth))
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

/// The More tab's root (`SCREENS.more`): the large title with search, then the Projects row
/// while corrald lists any, and a row per destination this workspace has with its icon, label
/// and the feed's subtitle, each pushing its destination.
fn more_list(nav: Nav) -> AnyElement {
    let app = nav.app;
    let more = app.feed.with_untracked(work::daemon_more);
    let rows = more.rows;
    let listed = computed(move || {
        let projects = app.feed.with(|feed| {
            !Projects::build(feed, &ProjectFilter::All)
                .entries
                .is_empty()
        });
        (destinations(app), projects)
    });
    let list = dynamic(move || {
        let (shown, projects) = listed.get();
        untracked(move || {
            view()
                .mt(px(6.0))
                .border_t_1()
                .border_t_color(color(Token::Rule))
                .children(projects.then(|| projects_row(nav)))
                .children(
                    rows.iter()
                        .filter(|row| shown.contains(&row.destination))
                        .map(|&row| more_row(nav, row)),
                )
                .into_element()
        })
    });
    frame(
        app,
        Some(head(app, more.title, None, vec![search(app)])),
        follow_scroll(app.form, nsh_core::prelude::scroll())
            .test_id("more-list")
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

/// More's Projects row, first: every project's open work, a page of its own.
fn projects_row(nav: Nav) -> AnyElement {
    let app = nav.app;
    list_row(
        app,
        format!("{PROJECTS_HEADING}, {PROJECTS_MORE_SUBTITLE}"),
        "nav-projects",
        || false,
        move || nav.open_projects(),
        vec![
            view()
                .w(px(28.0))
                .flex_shrink_0()
                .child(icon(Icon::Project, 20.0, color(Token::Ink)))
                .into_element(),
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(plain(app, PROJECTS_HEADING).truncate())
                .child(dim(app, PROJECTS_MORE_SUBTITLE).truncate())
                .into_element(),
            icon(Icon::Chevron, 16.0, color(Token::Ink3)),
        ],
    )
}

/// One More row (`.prow` at 28 / 1fr / 20 columns, at least 56 pt high).
fn more_row(nav: Nav, row: MoreRow) -> AnyElement {
    let app = nav.app;
    list_row(
        app,
        if row.subtitle.is_empty() {
            row.label.to_owned()
        } else {
            format!("{}, {}", row.label, row.subtitle)
        },
        format!("nav-{}", row.destination.slug()),
        || false,
        move || nav.open_more(row),
        vec![
            view()
                .w(px(28.0))
                .flex_shrink_0()
                .child(icon(
                    Icon::destination(row.destination),
                    20.0,
                    color(Token::Ink),
                ))
                .into_element(),
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(2.0))
                .child(plain(app, row.label).truncate())
                .when(!row.subtitle.is_empty(), |v| {
                    v.child(dim(app, row.subtitle).truncate())
                })
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

/// Tracked: whether a tab is the one shown. An assistant workspace's tabs are destinations, and
/// its Ask tab is its landing (which stands in Now's place).
fn tab_selected(nav: Nav, target: &NavigationTarget) -> bool {
    match target {
        NavigationTarget::Tab(tab) => nav.tab.get() == *tab,
        NavigationTarget::Destination(destination) => nav.showing() == Some(*destination),
        NavigationTarget::Ask => nav.showing() == Some(Destination::Now),
    }
}

/// Choosing a tab shows its root, the selected one included; the Ask tab shows the landing and
/// opens Ask over it.
fn tab_press(nav: Nav, target: &NavigationTarget) {
    match target {
        NavigationTarget::Tab(tab) => nav.select(*tab),
        NavigationTarget::Destination(destination) => nav.app.go(*destination),
        NavigationTarget::Ask => {
            nav.app.go(Destination::Now);
            nav.app.open_ask();
        }
    }
}

fn tab_icon(target: &NavigationTarget) -> Icon {
    match target {
        NavigationTarget::Tab(tab) => Icon::phone_tab(*tab),
        NavigationTarget::Destination(destination) => Icon::destination(*destination),
        NavigationTarget::Ask => Icon::Ask,
    }
}

/// Tracked: the tabs this workspace has (`nav::phone_navigation`).
fn tab_items(nav: Nav) -> Memo<Vec<NavigationItem>> {
    computed(move || nav.app.feed.with(phone_navigation))
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
                .map(|item| {
                    let entry = TabBarItem::new(item.label);
                    // Android icons are the app's own drawables, which this build has none of.
                    if ios {
                        entry.icon(symbol(&item.target))
                    } else {
                        entry
                    }
                })
                .collect();
            tab_bar()
                .items(bar)
                .selected_index(move || {
                    items.with(|items| {
                        items
                            .iter()
                            .position(|item| tab_selected(nav, &item.target))
                            .and_then(|at| u32::try_from(at).ok())
                            .unwrap_or(0)
                    })
                })
                .on_change(move |index: &u32| {
                    let target = items.with_untracked(|items| {
                        usize::try_from(*index)
                            .ok()
                            .and_then(|at| items.get(at))
                            .map(|item| item.target.clone())
                    });
                    if let Some(target) = target {
                        tab_press(nav, &target);
                    }
                })
                .into_element()
        })
    })
}

/// The SF Symbol of a tab on iOS.
fn symbol(target: &NavigationTarget) -> &'static str {
    match target {
        NavigationTarget::Tab(PhoneTab::Now) => "bolt",
        NavigationTarget::Tab(PhoneTab::Agents)
        | NavigationTarget::Destination(Destination::Agents) => "terminal",
        NavigationTarget::Tab(PhoneTab::Plans) => "list.bullet.rectangle",
        NavigationTarget::Tab(PhoneTab::More) => "ellipsis",
        NavigationTarget::Destination(Destination::Usage) => "chart.bar",
        NavigationTarget::Destination(Destination::Settings) => "gearshape",
        NavigationTarget::Destination(_) => "circle",
        NavigationTarget::Ask => "bubble.left",
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
fn app_tab(nav: Nav, item: NavigationItem) -> AnyElement {
    let app = nav.app;
    let NavigationItem { id, label, target } = item;
    let probe = target.clone();
    let selected = computed(move || tab_selected(nav, &probe));
    let ink = move || {
        if selected.get() {
            color(Token::Ink)
        } else {
            color(Token::Ink3)
        }
    };
    let glyph = tab_icon(&target);
    let now_tab = target == NavigationTarget::Tab(PhoneTab::Now);
    let glyph = view()
        .relative()
        .size(px(TAB_ICON))
        .child(dynamic(move || icon(glyph, TAB_ICON, ink())))
        .when(now_tab, |v| v.child(needs_badge(app)));
    // The test id sits on the icon and label; a press there reaches the tab.
    let face = view()
        .test_id(&id)
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
    // The tab's own name, so it is announced even while a page's bar hides the face; Now
    // carries its needs-you count, which the badge otherwise announces alone.
    let name = move || {
        Some(SharedString::from(
            match (now_tab, app.snapshot().count_words(Destination::Now)) {
                (true, Some(words)) => format!("{label}, {words}"),
                _ => label.to_owned(),
            },
        ))
    };
    tab()
        .selected(selected)
        .accessibility_label(name)
        .on_press(move |_: &()| tab_press(nav, &target))
        .flex_1()
        .h(px(TAB_HEIGHT))
        .items_center()
        .cursor_pointer()
        .active(|s| s.opacity(0.7))
        .focus_visible(ring)
        .child(face)
        .into_element()
}

/// Now's needs-you count as a badge at the icon's top trailing corner (`.ptab .badge`).
fn needs_badge(app: AppState) -> AnyElement {
    view()
        .absolute()
        .top(px(-2.0))
        .left(px(TAB_ICON / 2.0 + 6.0))
        .child(atoms::count(app, Destination::Now))
        .into_element()
}
