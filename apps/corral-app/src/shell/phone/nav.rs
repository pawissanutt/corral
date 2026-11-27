//! The phone shell's navigation: the tab shown and its stack above the root, kept in step with
//! the model's pages both ways, the bars and titles its pages bring, and which pages are drawn.
//!
//! Every page a screen opens is pushed on the tab it was opened from, so Back returns to where
//! the user was; only a `go` (a tab, the health strip, a reducer) and the palette land on the
//! page's own tab, as the mock's `nav()` does.

use std::time::Duration;

use nsh_core::{platform::Os, prelude::*};

use crate::model::{Destination, NavState, Overlay, PhoneTab, Route, SettingsSection};
use crate::state::AppState;
use crate::ui::kit::page_phone::{PageScroll, page_scroll};

/// How long a pushed page slides in; the page under it stays drawn until then.
const SLIDE: Duration = Duration::from_millis(450);

/// One page of a tab's stack above its root.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum Entry {
    /// A page the model knows (`NavState::pages`): a task's or a session's.
    Page(Route),
    /// A destination over a tab's root: one of Activity's pages over its list, or Settings
    /// over whichever tab the gear was pressed on.
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
    /// On the web, the depth of a page the browser's back took away that still slides out: the
    /// browser's history is already under it, so what shows and the address are the stack up
    /// to it, while the page stays mounted until its dismissal.
    leaving: Signal<Option<usize>>,
    /// Each depth's generation, part of its page's key: a page pushed again where one still
    /// slides out (Forward right after Back) is a new screen, not the one going.
    gens: Signal<Vec<u64>>,
    last_gen: Stored<u64>,
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
            leaving: signal(None),
            gens: signal(Vec::new()),
            last_gen: stored(0),
            bars: signal(Vec::new()),
            titles: signal(Vec::new()),
            drawn_from: signal(drawn),
            turn: stored(0),
        };
        nav.follow();
        nav.track_drawn();
        nav.tell_section();
        nav.tell_list();
        nav.open_lists();
        nav.leave_unoffered();
        nav
    }

    /// Leaves a tab the workspace stops offering (a cold `/insight` that the feed then shows to
    /// be assistant-only): Home, as every place it does not have goes.
    fn leave_unoffered(self) {
        let offered = computed(move || self.app.feed.with(crate::model::nav::phone_tabs));
        effect(move || {
            let tab = self.tab.get();
            if !offered.get().contains(&tab) {
                untracked(|| self.app.go(Destination::Home));
            }
        });
    }

    /// Tells the app which place's list of pages shows at the tab's root with nothing over it,
    /// with the navigation's `goes` then, for the address to name (`/insight`, `/activity`).
    fn tell_list(self) {
        effect(move || {
            let tab = self.tab.get();
            let bare = self.visible_len() == 0;
            let list = (bare && tab.destination().is_none()).then(|| tab.area());
            let goes = self.app.nav.with_untracked(|nav| nav.goes);
            self.app.phone_list.set(list.map(|area| (area, goes)));
        });
    }

    /// Opens the tab root an address asks for: a list of pages, with nothing over it. A
    /// workspace without that tab (assistant-only has no Insight) goes where the place's key
    /// would instead.
    fn open_lists(self) {
        effect(move || {
            let Some(area) = self.app.phone_list_open.get() else {
                return;
            };
            untracked(|| {
                self.app.phone_list_open.set(None);
                let offered = self.app.feed.with(crate::model::nav::phone_tabs);
                match offered.into_iter().find(|tab| tab.area() == area) {
                    Some(tab) => self.select(tab),
                    None => self.app.go_area(area),
                }
            });
        });
    }

    /// Tells the app which Settings section the stack has open, for the address to name.
    fn tell_section(self) {
        effect(move || {
            let shown = self.visible_len();
            let section = self.stack.with(|s| {
                s[..shown].iter().rev().find_map(|entry| match entry {
                    Entry::Section(section) => Some(*section),
                    Entry::Page(_) | Entry::Destination(_) => None,
                })
            });
            self.app.phone_section.set(section);
        });
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
        // Settings has no tab of its own: it opens over the tab in view.
        let here = self.tab.get_untracked();
        if now.section_opens != was.section_opens {
            self.land(
                here,
                vec![
                    Entry::Destination(Destination::Settings),
                    Entry::Section(now.settings),
                ],
            );
            return;
        }
        if now.goes != was.goes {
            self.land(
                now.destination.phone_tab().unwrap_or(here),
                entries_of(now.destination),
            );
            return;
        }
        if now.pages == was.pages || now.pages == pages_of(&self.visible()) {
            return;
        }
        match pushed(&was.pages, &now.pages) {
            Some(page) if global => self.land(home(&page), vec![Entry::Page(page)]),
            _ => {
                // A page still sliding out goes first: the model's pages are those under it.
                self.release();
                self.stack.update(|stack| follow_pages(stack, &now.pages));
            }
        }
    }

    /// Tracked: how many entries show, without one that slides out after the browser's back.
    fn visible_len(self) -> usize {
        let len = self.stack.with(Vec::len);
        self.leaving.get().map_or(len, |at| at.min(len))
    }

    /// The browser's back took the page at `depth` away on the web: what shows (and the address)
    /// is the stack under it from now on, while it slides out until its dismissal.
    pub fn leave(self, depth: usize) {
        if os() != Os::Web {
            return;
        }
        let visible = self.visible();
        if visible.len() <= depth {
            return;
        }
        let pages = pages_of(&visible[depth..]).len();
        self.leaving.set(Some(depth));
        self.went_back(pages);
    }

    /// The entries that show: the stack without a page sliding out after the browser's back.
    fn visible(self) -> Vec<Entry> {
        let at = self.leaving.get_untracked();
        self.stack.with_untracked(|s| {
            let shown = at.map_or(s.len(), |at| at.min(s.len()));
            s[..shown].to_vec()
        })
    }

    /// Lets a page sliding out go at once (its screen leaves now), and makes whatever is pushed
    /// from its depth up a new screen rather than the one the browser already took away.
    fn release(self) {
        let Some(at) = self.leaving.get_untracked() else {
            return;
        };
        self.leaving.set(None);
        self.stack.update(|s| s.truncate(at));
        let round = self.last_gen.with(|last| {
            *last += 1;
            *last
        });
        self.gens.update(|gens| {
            // Depths past the vector inherit its last round (`key`): extending keeps theirs.
            let inherited = gens.last().copied().unwrap_or(0);
            gens.resize(gens.len().max(at + 1), inherited);
            for slot in &mut gens[at..] {
                *slot = round;
            }
        });
    }

    /// Before the stack changes to `entries` on `tab`: a page sliding out stays until its
    /// dismissal while what shows under it is already that; any other change lets it go first,
    /// on its own, so a page pushed again where it was (Forward) mounts anew rather than
    /// keeping the screen the browser already took away.
    fn let_go(self, tab: PhoneTab, entries: &[Entry]) -> bool {
        let Some(at) = self.leaving.get_untracked() else {
            return false;
        };
        let kept = self.tab.get_untracked() == tab
            && self
                .stack
                .with_untracked(|s| s.get(..at).is_some_and(|under| under == entries));
        if !kept {
            self.release();
        }
        kept
    }

    /// Tracked: the key of the page `entry` at `depth`: its depth, route and generation.
    pub fn key(self, depth: usize, entry: &Entry) -> String {
        // A depth past those released takes the last round, so deeper pages mount anew too.
        let round = self.gens.with(|gens| {
            gens.get(depth)
                .or(gens.last().filter(|_| depth >= gens.len()))
                .copied()
                .unwrap_or(0)
        });
        let key = key_of(depth, entry);
        if round == 0 {
            key
        } else {
            format!("{key}#{round}")
        }
    }

    /// Shows `tab` with `entries` over its root and makes the model's pages the same.
    fn land(self, tab: PhoneTab, entries: Vec<Entry>) {
        let pages = pages_of(&entries);
        if !self.let_go(tab, &entries) {
            batch(|| {
                self.tab.set(tab);
                self.stack.set(entries);
            });
        }
        if self.app.nav.with_untracked(|n| n.pages != pages) {
            self.app.nav.update(|n| n.pages = pages);
        }
    }

    /// The destination on screen: the one in the stack, or the tab's own.
    fn shown(self) -> Option<Destination> {
        stacked(&self.visible()).or_else(|| self.tab.get_untracked().destination())
    }

    /// The user chose `tab`: its root, with nothing over it, as the mock's tab bar does.
    pub fn select(self, tab: PhoneTab) {
        self.leaving.set(None);
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

    /// A row of Activity's list: its page over the list.
    pub fn open_page(self, page: Destination) {
        self.let_go(self.tab.get_untracked(), &[Entry::Destination(page)]);
        self.stack.set(vec![Entry::Destination(page)]);
        self.app.go(page);
    }

    /// Leaves the page at `depth` and those above it (the header's back, the swipe, Android's
    /// back, the browser's): the model's pages among them go too, in one change. Going back
    /// twice to the same depth does nothing the second time. Over a profile shown alone,
    /// Configuration stays and its profile list shows again; leaving Configuration with unsaved
    /// edits asks first, as leaving it any other way does.
    pub fn back_to(self, depth: usize) {
        // The dismissal of a page the browser's back already left: the model went back then.
        if let Some(at) = self.leaving.get_untracked()
            && at <= depth
        {
            self.stack.update(|s| s.truncate(depth));
            if self.stack.with_untracked(Vec::len) <= at {
                self.leaving.set(None);
            }
            return;
        }
        // A page above still sliding out left the model already: only those shown go now.
        let visible = self.visible();
        let Some((pages, config)) = (self.stack.with_untracked(Vec::len) > depth).then(|| {
            let going = visible.get(depth..).unwrap_or_default();
            let config = going.contains(&Entry::Section(SettingsSection::Config));
            (pages_of(going).len(), config)
        }) else {
            return;
        };
        if config && (self.app.back_to_profiles() || self.app.asks_before_leaving_config()) {
            return;
        }
        self.stack.update(|s| s.truncate(depth));
        self.leaving.set(None);
        self.went_back(pages);
    }

    /// The model follows a back past `pages` of its pages: they go, and the destination is
    /// what shows again.
    fn went_back(self, pages: usize) {
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
        let section = match self.visible().last() {
            Some(Entry::Section(section)) => Some(*section),
            _ => None,
        };
        match section {
            Some(section) => self.app.go_settings(section),
            None => self.app.go(destination),
        }
    }

    /// Tracked: whether the page on top shows its own bar, which takes the tab bar's place.
    pub fn covered(self) -> bool {
        let top = self
            .stack
            .with(|s| s.last().map(|e| (s.len() - 1, e.clone())))
            .map(|(depth, e)| self.key(depth, &e));
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
        let key = untracked(|| self.key(depth, entry));
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
        let Some(key) = self
            .stack
            .with(|s| s.get(below).cloned())
            .map(|e| self.key(below, &e))
        else {
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
        let top = move || {
            let top = self.stack.with(|s| s.last().cloned());
            let len = self.stack.with(Vec::len);
            (len, top.map(|entry| self.key(len - 1, &entry)))
        };
        let last = stored(untracked(top));
        effect(move || {
            let _tab = self.tab.get();
            let (len, key) = top();
            // A page pushed again at the same depth (Forward right after Back) slides in too.
            let (was, was_key) = last.replace((len, key.clone()));
            let grew = was < len || (was == len && len > 0 && was_key != key);
            let turn = self.turn.with(|turn| {
                *turn += 1;
                *turn
            });
            untracked(|| {
                page_scroll().set(PageScroll::default());
                if !grew {
                    // What a page still sliding out uncovered stays drawn.
                    let shown = self.leaving.get_untracked().map_or(len, |at| at.min(len));
                    self.drawn_from.set(shown);
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
/// model's destination (one of Activity's pages pushed over its list; Settings over Home).
fn start_of(nav: &NavState) -> (PhoneTab, Vec<Entry>) {
    match nav.pages.first() {
        Some(first) => (
            home(first),
            nav.pages.iter().cloned().map(Entry::Page).collect(),
        ),
        None => (
            nav.destination.phone_tab().unwrap_or(PhoneTab::Home),
            entries_of(nav.destination),
        ),
    }
}

/// The tab a page belongs to.
fn home(page: &Route) -> PhoneTab {
    page.owner().phone_tab().unwrap_or(PhoneTab::Home)
}

/// The stack that shows a destination: nothing over the root of the tab that is the
/// destination, else the destination over its tab's root (Activity's list, or for Settings
/// the tab in view).
fn entries_of(destination: Destination) -> Vec<Entry> {
    let root = destination.phone_tab().and_then(PhoneTab::destination);
    if root == Some(destination) {
        Vec::new()
    } else {
        vec![Entry::Destination(destination)]
    }
}

fn pages_of(entries: &[Entry]) -> Vec<Route> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Page(page) => Some(page.clone()),
            Entry::Destination(_) | Entry::Section(_) => None,
        })
        .collect()
}

/// The topmost destination a stack shows over its root.
fn stacked(entries: &[Entry]) -> Option<Destination> {
    entries.iter().rev().find_map(|e| match e {
        Entry::Destination(d) => Some(*d),
        Entry::Page(_) | Entry::Section(_) => None,
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
