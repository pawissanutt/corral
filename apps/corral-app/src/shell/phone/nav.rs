//! The phone shell's navigation: the tab shown and its stack above the root, kept in step with
//! the model's pages both ways, the bars and titles its pages bring, and which pages are drawn.
//!
//! Every page a screen opens is pushed on the tab it was opened from, so Back returns to where
//! the user was; only a `go` (a tab, the health strip, a reducer) and the palette land on the
//! page's own tab, as the mock's `nav()` does.

use std::time::Duration;

use nsh_core::{platform::Os, prelude::*};

use crate::model::{
    Destination, NavState, Overlay, PhoneTab, Route, SettingsSection, work::MoreRow,
};
use crate::state::AppState;
use crate::ui::kit::page_phone::{PageScroll, page_scroll};

/// How long a pushed page slides in; the page under it stays drawn until then.
const SLIDE: Duration = Duration::from_millis(450);

/// One page of a tab's stack above its root.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum Entry {
    /// A page the model knows (`NavState::pages`): a live record.
    Page(Route),
    /// A destination opened from the More list.
    Destination(Destination),
    /// A Settings section opened from the Settings list.
    Section(SettingsSection),
    /// The project list opened from the More list.
    Projects,
}

/// What the shell follows in the model's navigation.
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    destination: Destination,
    settings: SettingsSection,
    section_opens: u32,
    goes: u32,
    pages: Vec<Route>,
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

/// The shell's navigation. Stack positions count the tab's root as 0 and the entry at depth
/// `d` as `d + 1`.
#[derive(Clone, Copy)]
pub(super) struct Nav {
    pub app: AppState,
    pub tab: Signal<PhoneTab>,
    pub stack: Signal<Vec<Entry>>,
    /// The bar each page on the stack brings, by page key, and whether it shows now.
    bars: Signal<Vec<(String, Memo<bool>)>>,
    /// The title of each page on the stack, by page key: the back label of the page above it.
    titles: Signal<Vec<(String, SharedString)>>,
    /// The lowest stack position drawn: the top page's, or the one under it while the top one
    /// slides in or out. Pages under it are hidden from sight, focus and screen readers.
    drawn_from: Signal<usize>,
    /// Counts stack changes, so a slide's end only hides what is still covered.
    turn: Stored<u64>,
}

impl Nav {
    /// Starts at the model's first page's tab with its pages, or at the model's destination,
    /// and follows the model from then on.
    pub fn start(app: AppState) -> Nav {
        let (tab, entries) = untracked(|| app.nav.with(start_of));
        let drawn = entries.len();
        let nav = Nav {
            app,
            tab: signal(tab),
            stack: signal(entries),
            bars: signal(Vec::new()),
            titles: signal(Vec::new()),
            drawn_from: signal(drawn),
            turn: stored(0),
        };
        nav.follow();
        nav.track_drawn();
        nav
    }

    /// Follows the model's navigation. The palette's closing marks the push right after it as
    /// the palette's, which lands on the page's own tab with a fresh stack.
    fn follow(self) {
        let app = self.app;
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
            untracked(|| self.reconcile(&was, &now, global));
            if only_closed {
                from_palette.replace(true);
                // A palette closed without a command leaves nothing for the next change to claim.
                set_timeout(Duration::ZERO, move || {
                    from_palette.replace(false);
                });
            }
        });
    }

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
        if now.pages == was.pages || now.pages == self.stack.with_untracked(|s| pages_of(s)) {
            return;
        }
        match pushed(&was.pages, &now.pages) {
            Some(page) if global => self.land(home(&page), vec![Entry::Page(page)]),
            _ => self.stack.update(|stack| follow_pages(stack, &now.pages)),
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
            .with_untracked(|s| stacked(s))
            .or_else(|| self.tab.get_untracked().destination())
    }

    /// Tracked [`Self::shown`]: what an assistant workspace's tab, a destination, marks.
    pub fn showing(self) -> Option<Destination> {
        self.stack
            .with(|s| stacked(s))
            .or_else(|| self.tab.get().destination())
    }

    /// The user chose `tab`: its root, with nothing over it, as the mock's tab bar does.
    pub fn select(self, tab: PhoneTab) {
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
    pub fn open_more(self, row: MoreRow) {
        self.stack.set(vec![Entry::Destination(row.destination)]);
        let action = row.action;
        self.app
            .act(self.app.work, move |state, _| state.act(action));
    }

    /// More's Projects row: the project list over More.
    pub fn open_projects(self) {
        self.stack.set(vec![Entry::Projects]);
    }

    /// Leaves the page at `depth` and those above it (the header's back, the swipe, Android's
    /// back, the browser's): the model's pages among them go too, in one change. Going back
    /// twice to the same depth does nothing the second time.
    pub fn back_to(self, depth: usize) {
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

    /// Tracked: whether the page on top shows its own bar, which takes the tab bar's place.
    pub fn covered(self) -> bool {
        let top = self
            .stack
            .with(|s| s.last().map(|e| key_of(s.len() - 1, e)));
        let shown = self.bars.with(|bars| {
            bars.iter()
                .map(|(key, shown)| (key.clone(), shown.get()))
                .collect::<Vec<_>>()
        });
        covers(top.as_deref(), &shown)
    }

    /// Records what the page at `depth` brings while it is mounted: its title, for the page
    /// above it, and its bar's presence.
    pub fn register(self, depth: usize, entry: &Entry, title: &str, bar: Option<Memo<bool>>) {
        let key = key_of(depth, entry);
        self.titles
            .update(|t| t.push((key.clone(), SharedString::from(title.to_owned()))));
        if let Some(shown) = bar {
            self.bars.update(|b| b.push((key.clone(), shown)));
        }
        cleanup(move || {
            self.titles.update(|t| t.retain(|(k, _)| *k != key));
            self.bars.update(|b| b.retain(|(k, _)| *k != key));
        });
    }

    /// Tracked: the back label of the page at `depth`, naming the page under it: the tab's
    /// root, or the page below in this stack.
    pub fn back_label(self, depth: usize) -> SharedString {
        let root = SharedString::from(self.tab.get().label());
        let Some(below) = depth.checked_sub(1) else {
            return root;
        };
        let Some(key) = self.stack.with(|s| s.get(below).map(|e| key_of(below, e))) else {
            return root;
        };
        self.titles.with(|titles| {
            titles
                .iter()
                .find(|(k, _)| *k == key)
                .map_or(root, |(_, title)| title.clone())
        })
    }

    /// Tracked: whether the page at stack `position` is drawn. iOS and Android stack natively
    /// and hide covered pages themselves.
    pub fn drawn(self, position: usize) -> bool {
        matches!(os(), Os::Ios | Os::Android) || position >= self.drawn_from.get()
    }

    /// The user asked to leave the top page: the one under it shows again before the top one
    /// slides out (the browser's back, Escape).
    pub fn uncover(self, depth: usize) {
        self.turn.with(|turn| *turn += 1);
        self.drawn_from.set(depth);
    }

    /// Keeps the page under a pushed one drawn while it slides in, then hides it; a page that
    /// goes leaves the one under it drawn at once. Every change of tab or stack also starts the
    /// new page's scroll afresh, so a tucked Ask button comes back.
    fn track_drawn(self) {
        let last = stored(self.stack.with_untracked(Vec::len));
        effect(move || {
            let _tab = self.tab.get();
            let len = self.stack.with(Vec::len);
            let grew = last.replace(len) < len;
            let turn = self.turn.with(|turn| {
                *turn += 1;
                *turn
            });
            untracked(|| {
                page_scroll().set(PageScroll::default());
                if !grew {
                    self.drawn_from.set(len);
                    return;
                }
                self.drawn_from.set(len - 1);
                set_timeout(SLIDE, move || {
                    if self.turn.get() == turn {
                        self.drawn_from.set(len);
                    }
                });
            });
        });
    }
}

/// Where the shell starts: the tab of the model's first page and its pages, or the tab of the
/// model's destination (a More destination pushed over the More list).
fn start_of(nav: &NavState) -> (PhoneTab, Vec<Entry>) {
    match nav.pages.first() {
        Some(first) => (
            home(first),
            nav.pages.iter().cloned().map(Entry::Page).collect(),
        ),
        None => (nav.destination.phone_tab(), entries_of(nav.destination)),
    }
}

/// The tab a page belongs to.
fn home(page: &Route) -> PhoneTab {
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

fn pages_of(entries: &[Entry]) -> Vec<Route> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Page(page) => Some(page.clone()),
            Entry::Destination(_) | Entry::Section(_) | Entry::Projects => None,
        })
        .collect()
}

/// The topmost destination a stack shows over its root.
fn stacked(entries: &[Entry]) -> Option<Destination> {
    entries.iter().rev().find_map(|e| match e {
        Entry::Destination(d) => Some(*d),
        Entry::Page(_) | Entry::Section(_) | Entry::Projects => None,
    })
}

/// A stack page's key: its depth and route, so the same route pushed twice is two pages.
pub(super) fn key_of(depth: usize, entry: &Entry) -> String {
    format!("{depth}:{entry:?}")
}

/// `now` is `was` with one more page on top.
fn pushed(was: &[Route], now: &[Route]) -> Option<Route> {
    match now.split_last() {
        Some((page, below)) if below == was => Some(page.clone()),
        _ => None,
    }
}

/// Brings `stack` to the model's `pages` on the tab it is on: the entries up to the first page
/// that differs stay, the model's pages from there go on top. A push lands over the page it was
/// opened from, a back leaves the pages under it, and a page the model replaced (a pager's
/// Next) takes its place, whichever domain each page belongs to.
fn follow_pages(stack: &mut Vec<Entry>, pages: &[Route]) {
    let mut matched = 0;
    let mut keep = stack.len();
    for (at, entry) in stack.iter().enumerate() {
        if let Entry::Page(route) = entry {
            if pages.get(matched) == Some(route) {
                matched += 1;
            } else {
                keep = at;
                break;
            }
        }
    }
    stack.truncate(keep);
    stack.extend(pages.iter().skip(matched).cloned().map(Entry::Page));
}

/// Whether the page keyed `top` covers the tab bar: it brought a bar, and the bar shows now.
fn covers(top: Option<&str>, bars: &[(String, bool)]) -> bool {
    top.is_some_and(|top| bars.iter().any(|(key, shown)| key == top && *shown))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Id, Live};

    #[test]
    fn a_page_bar_covers_the_tabs_only_while_it_shows() {
        let plan = key_of(0, &Entry::Page(Route::Live(Live::Plan(Id::from("T-885")))));
        // Opened while its detail was loading: the plan has no bar yet, so the tabs stay.
        assert!(!covers(Some(plan.as_str()), &[(plan.clone(), false)]));
        // The detail arrived awaiting acceptance: Accept and Changes take the tabs' place.
        assert!(covers(Some(plan.as_str()), &[(plan.clone(), true)]));
        assert!(!covers(None, &[(plan, true)]));
    }

    #[test]
    fn a_session_opened_from_a_task_pushes_over_it_on_the_same_tab() {
        let task = Route::Live(Live::Task(Id::from("T-871")));
        let session = Route::Live(Live::Session(Id::from("T-871/R1")));
        let mut stack = vec![Entry::Page(task.clone())];
        follow_pages(&mut stack, &[task.clone(), session.clone()]);
        assert_eq!(stack, [Entry::Page(task.clone()), Entry::Page(session)]);
        follow_pages(&mut stack, std::slice::from_ref(&task));
        assert_eq!(stack, [Entry::Page(task)]);
    }
}
