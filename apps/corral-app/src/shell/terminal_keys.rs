//! The terminal's key line on the last row: a legend of the keys in focus (the page's, the
//! composer's while Ask's field has them, or the keys of the surface standing over the page), the
//! destinations, then Ask and Go to, and the sidebar's toggle while there is room. When the grid
//! is short of room the names give way one by one, then whole groups; a key's cap is never cut.

use nsh_core::prelude::*;

use crate::{
    model::{
        DataState, Destination, Live, Overlay,
        assistant::COMPOSER_KEYS,
        commands::{Surface, key_legend_live},
        nav::{ASK_KEY, ASK_LABEL, PALETTE_LABEL, SIDEBAR_LABEL},
    },
    shell::projects,
    state::AppState,
    ui::term::{self, cells},
};

/// The palette's key as the terminal spells a control chord.
pub(super) const PALETTE_CHORD: &str = "^K";
/// The sidebar's toggle (`mod+b`) as the terminal spells it.
const SIDEBAR_CHORD: &str = "^B";

/// One entry of the key line: its keys, what they go to, and how late its name gives way when
/// the line is short (0 first).
struct Entry {
    keys: String,
    name: &'static str,
    keep: u8,
}

impl Entry {
    fn new(keys: impl Into<String>, name: &'static str, keep: u8) -> Entry {
        Entry {
            keys: keys.into(),
            name,
            keep,
        }
    }

    /// The cap (the keys and a Cell each side), then the name a Cell after it.
    fn cells(&self, named: bool) -> usize {
        self.keys.chars().count()
            + 2
            + if named {
                1 + self.name.chars().count()
            } else {
                0
            }
    }
}

/// Go to's name gives way first, then Ask's: the sidebar lists both, named.
const KEEP_GO_TO: u8 = 1;
const KEEP_ASK: u8 = 2;
/// The destinations are named whenever they show: a destination's key alone (`,`, `.`) says
/// nothing, so they go whole rather than as bare keys.
const KEEP_DESTINATION: u8 = 3;
/// The keys in focus keep their names longest: they are the ones the page in view (or Ask's
/// field) answers to, and no other line of the screen lists them.
const KEEP_PAGE: u8 = 4;

/// The key line's groups in order, a line between each two: the keys in focus, the
/// destinations, then Ask and Go to, with the sidebar's toggle after them while there is room.
struct Line {
    own: Vec<Entry>,
    destinations: Vec<Entry>,
    actions: Vec<Entry>,
    sidebar: Option<Entry>,
}

/// The group the sidebar's toggle ends: Ask and Go to's.
const ACTIONS_GROUP: usize = 2;

/// One way of fitting the key line: names from `keep` on, the destinations or not, the
/// sidebar's toggle or not.
#[derive(Debug, Clone, Copy)]
struct Fit {
    keep: u8,
    destinations: bool,
    sidebar: bool,
}

/// Every fit, longest first. The sidebar's toggle goes first, whole: the sidebar it opens or
/// folds is in sight anyway. Then Go to's and Ask's names give way, which the sidebar lists too;
/// then the destinations go whole, all named or none shown, and the sidebar's toggle comes back
/// to say where they are named; last the names of the keys in focus. A cap never gives way to a
/// name.
fn fits() -> impl Iterator<Item = Fit> {
    let fit = |keep, destinations, sidebar| Fit {
        keep,
        destinations,
        sidebar,
    };
    [
        fit(0, true, true),
        fit(0, true, false),
        fit(KEEP_ASK, true, false),
        fit(KEEP_DESTINATION, true, false),
        fit(0, false, true),
    ]
    .into_iter()
    .chain((0..=KEEP_PAGE + 1).map(move |keep| fit(keep, false, false)))
}

/// The key line for what is in view. While Ask's field has the keys, letters typed go into it:
/// the line gives the composer's own keys and Go to, the one chord that still reaches past it.
/// While a surface stands over the page (`surface`) the page's keys do nothing: the line gives
/// the surface's own. Otherwise the keys of the page in view (when the model gives it any and
/// the destination is served), the `shown` destinations, then Ask and Go to. A destination whose
/// key the page takes for itself (`.`, Changes, opens a list's menu) is left out rather than
/// listed twice with two meanings. `off_board`: Now's selection is off the board.
/// `session`: the legend of the session page on top for its focused surface (its composer's
/// keys while it has them), which replaces the page's and the composer's.
#[allow(clippy::too_many_arguments)]
fn line(
    destination: Destination,
    live: Option<Live>,
    served: bool,
    typing: bool,
    surface: Option<Surface>,
    off_board: bool,
    shown: &[Destination],
    session: Option<Vec<(&'static str, &'static str)>>,
) -> Line {
    let go_to = Entry::new(PALETTE_CHORD, PALETTE_LABEL, KEEP_GO_TO);
    if typing {
        let own = match &session {
            Some(legend) => legend
                .iter()
                .map(|&(keys, name)| Entry::new(chord(keys), name, KEEP_PAGE))
                .collect(),
            None => COMPOSER_KEYS
                .iter()
                .map(|hint| Entry::new(hint.keys, hint.action, KEEP_PAGE))
                .collect(),
        };
        return Line {
            own,
            destinations: Vec::new(),
            actions: vec![go_to],
            sidebar: None,
        };
    }
    if let Some(surface) = surface {
        return Line {
            own: surface
                .keys()
                .into_iter()
                .map(|(keys, name)| Entry::new(keys, name, KEEP_PAGE))
                .collect(),
            destinations: Vec::new(),
            actions: if surface.reaches_past() {
                vec![go_to]
            } else {
                Vec::new()
            },
            sidebar: None,
        };
    }
    let legend = match session {
        Some(legend) => legend,
        None if served => key_legend_live(destination, live.as_ref(), off_board),
        None => Vec::new(),
    };
    let taken = |key: char| {
        legend
            .iter()
            .any(|(keys, _)| keys.split(' ').any(|own| own.chars().eq([key])))
    };
    let destinations = shown
        .iter()
        .copied()
        .filter(|item| !taken(item.key()))
        .map(|item| Entry::new(item.key(), item.label(), KEEP_DESTINATION))
        .collect();
    Line {
        own: legend
            .into_iter()
            .map(|(keys, name)| Entry::new(chord(keys), name, KEEP_PAGE))
            .collect(),
        destinations,
        actions: vec![Entry::new(ASK_KEY, ASK_LABEL, KEEP_ASK), go_to],
        sidebar: Some(Entry::new(SIDEBAR_CHORD, SIDEBAR_LABEL, 0)),
    }
}

/// A key as the line spells it: a control chord as the terminal writes one (`^K`, as Go to's).
fn chord(keys: &str) -> String {
    keys.replace("Ctrl+", "^")
}

impl Line {
    /// The entries `fit` shows, each with the index of its group.
    fn entries(&self, fit: Fit) -> impl Iterator<Item = (usize, &Entry)> {
        let destinations: &[Entry] = if fit.destinations {
            &self.destinations
        } else {
            &[]
        };
        let sidebar = self.sidebar.as_ref().filter(|_| fit.sidebar);
        [self.own.as_slice(), destinations, self.actions.as_slice()]
            .into_iter()
            .enumerate()
            .flat_map(|(group, entries)| entries.iter().map(move |entry| (group, entry)))
            .chain(sidebar.map(|entry| (ACTIONS_GROUP, entry)))
    }

    /// The Cells the first `count` entries take with `fit`: the line's margins, a Cell between
    /// two entries, and a line with a Cell each side between two groups.
    fn cells(&self, fit: Fit, count: usize) -> usize {
        let mut cells = 2;
        let mut last = None;
        for (group, entry) in self.entries(fit).take(count) {
            cells += match last {
                None => 0,
                Some(previous) if previous == group => 1,
                Some(_) => 3,
            };
            cells += entry.cells(entry.keep >= fit.keep);
            last = Some(group);
        }
        cells
    }

    /// The longest fit within `room` and how many entries it shows: all of them, unless even
    /// bare caps overflow, when the last caps go whole rather than any being cut.
    fn layout(&self, room: usize) -> (Fit, usize) {
        for fit in fits() {
            let count = self.entries(fit).count();
            if self.cells(fit, count) <= room {
                return (fit, count);
            }
        }
        let bare = Fit {
            keep: KEEP_PAGE + 1,
            destinations: false,
            sidebar: false,
        };
        let count = (0..self.entries(bare).count())
            .rev()
            .find(|&count| self.cells(bare, count) <= room)
            .unwrap_or(0);
        (bare, count)
    }
}

/// Tracked: the surface standing over the page and taking its keys, the newest first: the
/// palette, the confirmation, the refusal sheet, the Remove sheet, the rewind sheet, an
/// operation's form, the keyboard's menu or one a press opened; on Artifacts the full-size view,
/// or the filter while it has the keys.
fn open_surface(app: AppState) -> Option<Surface> {
    if app.is_open(Overlay::Palette) {
        Some(Surface::Palette)
    } else if app.confirm.with(Option::is_some) {
        Some(Surface::Confirm)
    } else if app.refusal.with(Option::is_some) || app.agents.with(|state| state.removing.is_some())
    {
        Some(Surface::Sheet)
    } else if app.agents.with(|state| state.rewind.is_some()) {
        Some(Surface::Menu)
    } else if app.work.with(|state| state.composer.is_some()) {
        Some(Surface::Form)
    } else if app.work.with(|state| state.menu.is_some()) || app.menus.get() > 0 {
        Some(Surface::Menu)
    } else if app.destination() == Destination::Artifacts && !app.is_open(Overlay::Ask) {
        app.artifacts.with(|state| {
            if state.full.is_some() {
                Some(Surface::FullSize)
            } else {
                state.typing.then_some(Surface::Filter)
            }
        })
    } else {
        None
    }
}

/// A session legend without its Ctrl+K: the line ends with its own Go to (`^K`).
fn without_go_to(legend: Vec<(&'static str, &'static str)>) -> Vec<(&'static str, &'static str)> {
    legend
        .into_iter()
        .filter(|&(keys, _)| chord(keys) != PALETTE_CHORD)
        .collect()
}

/// The last row, a legend: each key as a cap and the name it goes to, the keys in focus first.
/// When the grid is short of room the names give way one by one, then whole groups, as
/// [`fits`] orders them; a key's cap is never cut.
pub(super) fn key_line(
    app: AppState,
    typing: Memo<bool>,
    shown: Memo<Vec<Destination>>,
) -> AnyElement {
    let route = computed(move || {
        let selection = app.selection();
        let off_board = app
            .feed
            .with(|feed| crate::model::work::off_board(&selection, feed).is_some());
        (
            app.destination(),
            app.live_page(),
            app.data.get(),
            off_board,
            projects::open_project(app).is_some(),
        )
    });
    let surface = computed(move || open_surface(app));
    // The session page on top gives the legend of what has its keys (a picker, the `/` menu, the
    // transcript, a view, a card, the composer), unless Ask's field stands over it. Its Ctrl+K
    // is the line's own Go to, so it is not listed twice.
    let session = computed(move || {
        let Some(Live::Session(id)) = app.live_page() else {
            return None;
        };
        if app.is_open(Overlay::Ask) {
            return None;
        }
        let env = app.env();
        let legend = app.feed.with(|feed| {
            app.session.with(|state| {
                use crate::model::session::{self, Cx, Facts, Tab};
                let facts = Facts::of(feed, &id);
                let ui = state.ui(id.as_str());
                let cx = Cx::new(feed, ui, &id, env, &facts);
                let tab = if session::shown_tabs(&cx).contains(&ui.tab) {
                    ui.tab
                } else {
                    Tab::Chat
                };
                let slash = ui.draft.starts_with('/')
                    && session::slash(&cx, &session::actions(&cx)).is_some();
                session::legend_of(&cx, tab, slash)
            })
        });
        Some(without_go_to(legend))
    });
    dynamic(move || {
        let room = term::columns();
        let (destination, live, data, off_board, project_page) = route.get();
        let shown = shown.get();
        // An unserved destination answers no keys of its own; disconnected, none is served. A
        // Project page and the assistant workspace's landing are not their owner's list.
        let served = data != DataState::Disconnected
            && !project_page
            && shown.contains(&destination)
            && app.available(destination) == crate::model::Availability::Ready;
        let line = line(
            destination,
            live,
            served,
            typing.get(),
            surface.get(),
            off_board,
            &shown,
            session.get(),
        );
        let (fit, count) = line.layout(room);
        let mut row = view()
            .h(term::rows(1.0))
            .flex_shrink_0()
            .flex_row()
            .items_center()
            .px(cells(1.0))
            .overflow_hidden();
        let mut last = None;
        for (group, entry) in line.entries(fit).take(count) {
            match last {
                None => {}
                Some(previous) if previous == group => row = row.child(term::space(1.0)),
                Some(_) => {
                    row = row
                        .child(term::space(1.0))
                        .child(term::quiet("│"))
                        .child(term::space(1.0));
                }
            }
            last = Some(group);
            let keys = entry.keys.clone();
            row = row.child(if entry.keep >= fit.keep {
                term::legend(keys, entry.name)
            } else {
                term::key_cap(keys)
            });
        }
        row.into_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_line_never_cuts_a_key_or_shows_a_destination_key_unnamed() {
        let views = [
            (Destination::Now, None),
            (Destination::Plans, None),
            (Destination::Review, None),
            (Destination::Settings, None),
            (
                Destination::Agents,
                Some(Live::Session(crate::model::Id::new("T-14_R1"))),
            ),
        ];
        for (destination, live) in views {
            for typing in [false, true] {
                let line = line(
                    destination,
                    live.clone(),
                    true,
                    typing,
                    None,
                    false,
                    &Destination::ALL,
                    live.as_ref().map(|_| {
                        use crate::model::session::{Focus, Tab, legend};
                        without_go_to(legend(Focus::Composer, Tab::Chat, true))
                    }),
                );
                for room in 20..=240 {
                    let (fit, count) = line.layout(room);
                    assert!(
                        line.cells(fit, count) <= room,
                        "{destination:?} at {room}: {fit:?} overflows"
                    );
                    let unnamed = line
                        .entries(fit)
                        .take(count)
                        .any(|(group, entry)| group == 1 && entry.keep < fit.keep);
                    assert!(
                        !unnamed,
                        "{destination:?} at {room}: a destination key shows without its name"
                    );
                }
                // The classic 80-column grid keeps every key in focus, named.
                let (fit, count) = line.layout(80);
                assert!(
                    count >= line.own.len(),
                    "{destination:?} at 80 drops own keys"
                );
                assert!(
                    fit.keep <= KEEP_PAGE,
                    "{destination:?} at 80 drops own names"
                );
            }
        }
    }

    /// With a menu, a sheet or a form open the key line listed the page's keys, which do
    /// nothing then.
    #[test]
    fn an_open_surface_s_keys_replace_the_page_s() {
        let line = line(
            Destination::Now,
            None,
            true,
            false,
            Some(Surface::Menu),
            false,
            &Destination::ALL,
            None,
        );
        let keys: Vec<&str> = line.own.iter().map(|entry| entry.keys.as_str()).collect();
        let menu: Vec<&str> = Surface::Menu.keys().iter().map(|(keys, _)| *keys).collect();
        assert_eq!(keys, menu);
        assert!(line.destinations.is_empty() && line.actions.is_empty());
    }
}
