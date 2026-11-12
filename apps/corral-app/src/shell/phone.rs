//! The phone shell (iOS, Android, phone and tablet browsers): the health strip under the status
//! bar, the selected tab's page stack, the floating Ask button, and the tabs at the bottom, or the
//! top page's own bar in their place. iOS and Android show the system's tab bar; everywhere else
//! it takes no room, so the shell draws its own.
//!
//! Each tab is one stack: its root (a destination's screen, or the More list), then the pages
//! pushed over it. The model keeps the pages it knows (`NavState::pages`); the More list's
//! destinations and the Settings sections are this shell's own entries around them. The shell
//! keeps the two in step both ways: what the user does here goes to the model, and what the
//! model does (a screen opening a page, a reducer going back, the palette, the health strip)
//! lands on the right tab and stack, as the mock's `nav()` and `push` do.

use std::time::Duration;

use nsh_base::{nav_page, nav_stack, tab, tabs};
use nsh_core::{
    element::StyleRefinement,
    mobile::{TabBarItem, tab_bar},
    platform::Os,
    prelude::*,
};

use super::{banner, confirmation, lock, stale, toast};
use crate::{
    model::{
        Destination, NavState, Overlay, Page, PhoneTab, SettingsSection,
        nav::{ASK_BUTTON, ASK_BUTTON_LABEL},
        work::{self, MoreRow},
    },
    screens::{self, PhonePage, agents, assistant, inbox, now, plans, review, settings},
    state::AppState,
    ui::{
        ask,
        atoms::{self, HealthLayout},
        composer,
        icons::{Icon, icon},
        kit::{
            page::{head, pushed_head, search},
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
/// What the mock does to the board while corrald is down or this device offline (`.stalefx`).
const STALE_SATURATION: f32 = 0.35;
const STALE_OPACITY: f32 = 0.78;
/// A drawn tab (`.ptab button`): height, icon and label.
const TAB_HEIGHT: f32 = 50.0;
const TAB_ICON: f32 = 24.0;
const TAB_LABEL: f32 = 10.5;

/// One page of a tab's stack above its root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Entry {
    /// A page the model knows (`NavState::pages`).
    Page(Page),
    /// A destination opened from the More list.
    Destination(Destination),
    /// A Settings section opened from the Settings list.
    Section(SettingsSection),
}

/// What the shell follows in the model's navigation.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    destination: Destination,
    settings: SettingsSection,
    section_opens: u32,
    goes: u32,
    pages: Vec<Page>,
    palette: bool,
}

impl Seen {
    fn of(nav: &NavState) -> Seen {
        Seen {
            destination: nav.destination,
            settings: nav.settings,
            section_opens: nav.section_opens,
            goes: nav.goes,
            pages: nav.pages.clone(),
            palette: nav.is_open(Overlay::Palette),
        }
    }
}

/// The shell's navigation: the tab shown, its stack above the root, and the keys of the pages in
/// it that bring their own bar (the tab bar gives way while one of them is on top).
#[derive(Clone, Copy)]
struct Nav {
    app: AppState,
    tab: Signal<PhoneTab>,
    stack: Signal<Vec<Entry>>,
    bars: Signal<Vec<String>>,
}

pub fn shell(app: AppState) -> AnyElement {
    let (start, entries) = untracked(|| app.nav.with(start_of));
    let nav = Nav {
        app,
        tab: signal(start),
        stack: signal(entries),
        bars: signal(Vec::new()),
    };
    follow(nav);
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
        .child(when(
            move || app.lock.get(),
            move || {
                lock::lock_screen(app, move |page: Page| {
                    app.lock.set(false);
                    app.go(page.owner());
                    app.open(page);
                })
            },
        ))
        .into_element()
}

/// Where the shell starts: the tab of the model's first page and its pages, or the tab of the
/// model's destination (a More destination pushed over the More list).
fn start_of(nav: &NavState) -> (PhoneTab, Vec<Entry>) {
    match nav.pages.first() {
        Some(first) => (
            home(*first),
            nav.pages.iter().copied().map(Entry::Page).collect(),
        ),
        None => (nav.destination.phone_tab(), entries_of(nav.destination)),
    }
}

/// The tab a page belongs to.
fn home(page: Page) -> PhoneTab {
    page.owner().phone_tab()
}

/// The stack that shows a destination: nothing over a tab's root, the destination over More.
fn entries_of(destination: Destination) -> Vec<Entry> {
    if destination.phone_tab() == PhoneTab::More {
        vec![Entry::Destination(destination)]
    } else {
        Vec::new()
    }
}

fn pages_of(entries: &[Entry]) -> Vec<Page> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Page(page) => Some(*page),
            Entry::Destination(_) | Entry::Section(_) => None,
        })
        .collect()
}

/// A stack page's key: its depth and route, so the same route pushed twice is two pages.
fn key_of(depth: usize, entry: &Entry) -> String {
    format!("{depth}:{entry:?}")
}

/// Follows the model's navigation. A `go` (the palette, a reducer, the health strip) lands on
/// its destination's tab. The palette's closing marks the push right after it as the palette's,
/// which lands on the page's own tab with a fresh stack (the mock's `nav()`); a screen's push
/// stays on the tab it came from (the mock's `push`).
fn follow(nav: Nav) {
    let app = nav.app;
    let seen = computed(move || app.nav.with(Seen::of));
    let last = stored(seen.get_untracked());
    let from_palette = stored(false);
    effect(move || {
        let now = seen.get();
        let was = last.replace(now.clone());
        if now == was {
            return;
        }
        let closed = was.palette && !now.palette;
        let global = from_palette.replace(false) || closed;
        let only_closed = closed
            && now.pages == was.pages
            && now.goes == was.goes
            && now.section_opens == was.section_opens;
        untracked(|| nav.reconcile(&was, &now, global));
        if only_closed {
            from_palette.replace(true);
            // A palette closed without a command leaves nothing for the next change to claim.
            let _ = set_timeout(Duration::ZERO, move || {
                from_palette.replace(false);
            });
        }
    });
}

impl Nav {
    /// Brings the shell to what the model changed from `was` to `now`.
    fn reconcile(self, was: &Seen, now: &Seen, global: bool) {
        if now.section_opens != was.section_opens {
            self.land(
                PhoneTab::More,
                vec![
                    Entry::Destination(Destination::Settings),
                    Entry::Section(now.settings),
                ],
            );
            return;
        }
        if now.goes != was.goes {
            self.land(now.destination.phone_tab(), entries_of(now.destination));
            return;
        }
        if now.pages == was.pages {
            return;
        }
        if now.pages == self.stack.with_untracked(|s| pages_of(s)) {
            return;
        }
        if let Some(page) = pushed(&was.pages, &now.pages) {
            self.push(page, global);
        } else if pushed(&now.pages, &was.pages).is_some() {
            self.stack.update(|s| {
                if let Some(at) = s.iter().rposition(|e| matches!(e, Entry::Page(_))) {
                    s.truncate(at);
                }
            });
        } else if let Some(first) = now.pages.first() {
            self.land(
                home(*first),
                now.pages.iter().copied().map(Entry::Page).collect(),
            );
        } else {
            self.land(now.destination.phone_tab(), entries_of(now.destination));
        }
    }

    /// A page the model pushed. The question, a session and a thread live under their own tab;
    /// a task or a plan stays on the stack it was opened from, unless it was opened from a tab's
    /// root of another tab or from the palette.
    fn push(self, page: Page, global: bool) {
        let tab = self.tab.get_untracked();
        let target = home(page);
        let at_root = self.stack.with_untracked(Vec::is_empty);
        let detail = matches!(page, Page::Task(_) | Page::Plan(_));
        if global || (target != tab && (at_root || !detail)) {
            self.land(target, vec![Entry::Page(page)]);
        } else {
            self.stack.update(|s| s.push(Entry::Page(page)));
        }
    }

    /// Shows `tab` with `entries` over its root and makes the model's pages the same.
    fn land(self, tab: PhoneTab, entries: Vec<Entry>) {
        let pages = pages_of(&entries);
        batch(|| {
            self.tab.set(tab);
            self.stack.set(entries);
        });
        if self.app.nav.with_untracked(|n| n.pages != pages) {
            self.app.nav.update(|n| n.pages = pages);
        }
    }

    /// The destination on screen: the More destination in the stack, or the tab's own.
    fn shown(self) -> Option<Destination> {
        self.stack
            .with_untracked(|s| {
                s.iter().rev().find_map(|e| match e {
                    Entry::Destination(d) => Some(*d),
                    Entry::Page(_) | Entry::Section(_) => None,
                })
            })
            .or_else(|| self.tab.get_untracked().destination())
    }

    /// The user chose `tab`: its root, with nothing over it, as the mock's tab bar does.
    fn select(self, tab: PhoneTab) {
        batch(|| {
            self.tab.set(tab);
            self.stack.set(Vec::new());
        });
        match tab.destination() {
            Some(destination) => self.app.go(destination),
            None => {
                if self.app.nav.with_untracked(|n| !n.pages.is_empty()) {
                    self.app.nav.update(|n| n.pages.clear());
                }
            }
        }
    }

    /// A More row: its destination over the list, through the model's own action.
    fn open_more(self, row: MoreRow) {
        self.stack.set(vec![Entry::Destination(row.destination)]);
        let action = row.action;
        self.app
            .act(self.app.work, move |state, env| state.act(action, env));
    }

    /// Leaves the page at `depth` and those above it (the header's back, the swipe, Android's
    /// back, the browser's): the model's pages among them go too, in one change. Going back
    /// twice to the same depth does nothing the second time.
    fn back_to(self, depth: usize) {
        let Some(pages) = self
            .stack
            .with_untracked(|s| (s.len() > depth).then(|| pages_of(&s[depth..]).len()))
        else {
            return;
        };
        self.stack.update(|s| s.truncate(depth));
        if pages > 0 {
            self.app.nav.update(|n| {
                for _ in 0..pages {
                    n.back();
                }
            });
        }
        // A page's owner took over the destination when it opened; give it back to what shows.
        // A `go` lands on its destination's stack, so a section open goes back through its own.
        let Some(destination) = self.shown() else {
            return;
        };
        if self
            .app
            .nav
            .with_untracked(|n| !n.pages.is_empty() || n.destination == destination)
        {
            return;
        }
        let section = self.stack.with_untracked(|s| match s.last() {
            Some(Entry::Section(section)) => Some(*section),
            _ => None,
        });
        match section {
            Some(section) => self.app.go_settings(section),
            None => self.app.go(destination),
        }
    }

    /// Whether the page on top brings its own bar, which takes the tab bar's place.
    fn covered(self) -> bool {
        let top = self
            .stack
            .with(|s| s.last().map(|e| key_of(s.len() - 1, e)));
        top.is_some_and(|key| self.bars.with(|b| b.contains(&key)))
    }
}

/// `now` is `was` with one more page on top.
fn pushed(was: &[Page], now: &[Page]) -> Option<Page> {
    match now.split_last() {
        Some((page, below)) if below == was => Some(*page),
        _ => None,
    }
}

fn health_strip(app: AppState) -> AnyElement {
    view()
        .bg(color(Token::Shell))
        .pt_safe_area_plus(px(2.0))
        .pb(px(6.0))
        .pl_safe_area_plus(px(16.0))
        .pr_safe_area_plus(px(16.0))
        .child(atoms::health_line(app, HealthLayout::Strip))
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
                    .with(|s| s.iter().copied().enumerate().collect::<Vec<_>>())
            },
            |item: &(usize, Entry)| key_of(item.0, &item.1),
            move |item, _| {
                let (depth, entry) = item.get_untracked();
                pushed_page(nav, depth, entry)
            },
        ))
        .into_element()
}

/// A tab's root carries its own large title on every platform, so no header shows over it.
fn root(nav: Nav, tab: PhoneTab) -> Screen {
    let app = nav.app;
    let body = match tab.destination() {
        Some(destination) => frame(
            app,
            None,
            screens::destination(app, destination),
            None,
            true,
            false,
        ),
        None => more_list(nav),
    };
    nav_page()
        .key(tab.slug())
        .title(tab.label())
        .header_visible(false)
        .child(body)
}

/// The page an entry shows.
fn page_of(app: AppState, entry: Entry) -> PhonePage {
    match entry {
        Entry::Page(Page::Question) => inbox::question_phone(app),
        Entry::Page(Page::Task(_)) => now::task_phone(app),
        Entry::Page(Page::Plan(_)) => plans::plan_phone(app),
        Entry::Page(Page::Session(_)) => agents::session_phone(app),
        Entry::Page(Page::Thread(_)) => assistant::thread_phone(app),
        Entry::Destination(Destination::Review) if untracked(|| app.snapshot().connected()) => {
            review::page_phone(app)
        }
        Entry::Destination(destination) => PhonePage {
            title: destination.label().to_owned(),
            back: PhoneTab::More.label(),
            body: screens::destination(app, destination),
            trailing: None,
            bar: None,
            ask: true,
            scroll: false,
        },
        Entry::Section(section) => settings::section_phone(app, section),
    }
}

/// A pushed page: the head with its back control (the system's on iOS and Android), then the
/// page in its frame.
fn pushed_page(nav: Nav, depth: usize, entry: Entry) -> Screen {
    let app = nav.app;
    let PhonePage {
        title,
        back,
        body,
        trailing,
        bar,
        ask,
        scroll,
    } = page_of(app, entry);
    if bar.is_some() {
        let key = key_of(depth, &entry);
        let gone = key.clone();
        nav.bars.update(|b| b.push(key));
        cleanup(move || nav.bars.update(|b| b.retain(|k| *k != gone)));
    }
    let page = nav_page()
        .title(SharedString::from(title.clone()))
        .header_style_with(|s| {
            s.bg(color(Token::Bg))
                .text_color(color(Token::Ink))
                .tint_color(color(Token::Ink2))
        })
        .on_dismiss(move || nav.back_to(depth));
    let page = if matches!(os(), Os::Ios | Os::Android) {
        match trailing {
            Some(trailing) => page.header_right(trailing),
            None => page,
        }
    } else {
        page.header(pushed_head(
            app,
            title,
            back,
            move || nav.back_to(depth),
            trailing,
        ))
    };
    page.child(frame(app, None, body, bar, ask, scroll))
}

/// What every page sits in: an optional head, the connection banner, the body (faded while the
/// board is stale, scrolled here when the page asks for it), the floating Ask button over its
/// bottom edge, and the page's own bar under it.
fn frame(
    app: AppState,
    head: Option<AnyElement>,
    body: AnyElement,
    bar: Option<AnyElement>,
    ask: bool,
    scroll: bool,
) -> AnyElement {
    let content = if scroll {
        nsh_core::prelude::scroll()
            .flex_1()
            .min_h(px(0.0))
            .child(view().when(ask, |v| v.pb(px(ASK_CLEARANCE))).child(body))
            .into_element()
    } else {
        body
    };
    view()
        .flex_1()
        .min_h(px(0.0))
        .bg(color(Token::Bg))
        .children(head)
        .child(banner(app))
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .relative()
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
                        move || !app.is_open(Overlay::Ask) && !app.share.get(),
                        move || ask_button(app),
                    ))
                }),
        )
        .children(bar)
        .into_element()
}

/// The More tab's root (`SCREENS.more`): the large title with search, then a row per
/// destination with its icon, label and the model's subtitle, each pushing its destination.
fn more_list(nav: Nav) -> AnyElement {
    let app = nav.app;
    let more = app
        .work
        .with_untracked(|state| work::more(state, app.env()));
    let rows: Vec<AnyElement> = more.rows.iter().map(|&row| more_row(nav, row)).collect();
    let list = view()
        .mt(px(6.0))
        .border_t_1()
        .border_t_color(color(Token::Rule))
        .children(rows)
        .into_element();
    frame(
        app,
        Some(head(app, more.title, None, vec![search(app)])),
        nsh_core::prelude::scroll()
            .test_id("more-list")
            .flex_1()
            .min_h(px(0.0))
            .child(view().pb(px(ASK_CLEARANCE)).child(list))
            .into_element(),
        None,
        true,
        false,
    )
}

/// One More row (`.prow` at 28 / 1fr / 20 columns, at least 56 pt high).
fn more_row(nav: Nav, row: MoreRow) -> AnyElement {
    let app = nav.app;
    list_row(
        app,
        format!("{}, {}", row.label, row.subtitle),
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
                .child(dim(app, row.subtitle).truncate())
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

/// Ask floats at the trailing edge just above the tab bar or the page's bar (`.askfab`).
fn ask_button(app: AppState) -> AnyElement {
    let ink = color(Token::BtnInk);
    view()
        .absolute()
        .right(px(ASK_RIGHT))
        .bottom(px(ASK_BOTTOM))
        .mr_safe_area()
        .h(px(ASK_HEIGHT))
        .pl(px(14.0))
        .pr(px(18.0))
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
                .text_color(ink),
        )
        .into_element()
}

/// iOS and Android: the system's bar, in flow so it takes its own height, the home indicator's
/// or navigation bar's inset included.
fn system_tabs(nav: Nav) -> AnyElement {
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
        .selected_index(move || nav.tab.get().index())
        .on_change(move |index: &u32| {
            if let Some(tab) = PhoneTab::from_index(*index) {
                nav.select(tab);
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

/// Every other platform: the tabs in the mock's look (`.ptab`), above the bottom inset, on the
/// page's ground behind a hairline.
fn app_tabs(nav: Nav) -> AnyElement {
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
        .children(PhoneTab::ALL.iter().map(|&which| app_tab(nav, which)))
        .into_element()
}

/// One drawn tab: the selected one has the strong ink and a bold label, so colour is never the
/// only cue. Choosing a tab shows its root, the selected one included.
fn app_tab(nav: Nav, which: PhoneTab) -> AnyElement {
    let app = nav.app;
    let selected = move || nav.tab.get() == which;
    let ink = move || {
        if selected() {
            color(Token::Ink)
        } else {
            color(Token::Ink3)
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
        .child(glyph)
        .child(
            text(which.label())
                .font_family(FONT_TEXT)
                .text_size(px(TAB_LABEL))
                .style_when(selected, |s| s.font_weight(FontWeight::BOLD))
                .text_color(ink),
        );
    tab()
        .selected(selected)
        .on_press(move |_: &()| nav.select(which))
        .flex_1()
        .h(px(TAB_HEIGHT))
        .items_center()
        .cursor_pointer()
        .active(|s| s.opacity(0.7))
        .focus_visible(ring)
        .child(face)
        .into_element()
}

/// The Inbox count as a badge at the icon's top trailing corner (`.ptab .badge`).
fn inbox_badge(app: AppState) -> AnyElement {
    view()
        .absolute()
        .top(px(-2.0))
        .left(px(TAB_ICON / 2.0 + 6.0))
        .child(atoms::count(app, Destination::Inbox))
        .into_element()
}
