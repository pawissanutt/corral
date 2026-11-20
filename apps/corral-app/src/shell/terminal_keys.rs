//! The terminal's key line on the last row: a legend of the keys in focus (the page's, the
//! composer's while Ask's field has them, or the keys of the surface standing over the page), the
//! destinations, then Ask and Go to, and the sidebar's toggle while there is room. Every entry is
//! a whole pair, its key and what it does: a key without its word says nothing, so when the grid
//! is short of room whole entries give way, never a word. The keys in focus come first, in their
//! order, as many as fit; then Ask, the destinations (all or none) and the sidebar's toggle as
//! room is left. Go to (`^K`) always ends the line, as `^K more` when keys in focus were left
//! out: the palette lists every action of what is in view.

use nsh_core::prelude::*;

use crate::{
    model::{
        Area, DataState, Destination, Live, Overlay, WorkspaceMode,
        assistant::COMPOSER_KEYS,
        commands::{Surface, area_key, key_legend_live},
        nav::{ASK_KEY, ASK_LABEL, PALETTE_LABEL, SIDEBAR_LABEL},
    },
    state::AppState,
    ui::term::{self, cells},
};

/// The palette's key as the terminal spells a control chord.
pub(super) const PALETTE_CHORD: &str = "^K";
/// The sidebar's toggle (`mod+b`) as the terminal spells it.
const SIDEBAR_CHORD: &str = "^B";

/// One entry of the key line: its keys and what they go to.
#[derive(Debug, Clone)]
struct Entry {
    keys: String,
    name: &'static str,
}

impl Entry {
    fn new(keys: impl Into<String>, name: &'static str) -> Entry {
        Entry {
            keys: keys.into(),
            name,
        }
    }

    /// The cap (the keys and a Cell each side), then the name a Cell after it.
    fn cells(&self) -> usize {
        self.keys.chars().count() + 2 + 1 + self.name.chars().count()
    }
}

/// What Go to says when keys in focus did not fit: the palette has them.
const MORE_LABEL: &str = "more";

/// The key line's groups in order, a line between each two: the keys in focus, the
/// destinations, then Ask and Go to, with the sidebar's toggle after them while there is room.
struct Line {
    own: Vec<Entry>,
    destinations: Vec<Entry>,
    /// Ask, while the page's keys are the line's.
    ask: Option<Entry>,
    /// Go to (`^K`), which ends the line wherever the palette reaches.
    go_to: Option<Entry>,
    sidebar: Option<Entry>,
}

/// The group Ask, Go to and the sidebar's toggle share.
const ACTIONS_GROUP: usize = 2;

/// What a layout of the line shows: how many keys in focus, and which of the rest.
#[derive(Debug, Clone, Copy, Default)]
struct Fit {
    own: usize,
    ask: bool,
    destinations: bool,
    sidebar: bool,
}

/// The key line for what is in view. While Ask's field has the keys, letters typed go into it:
/// the line gives the composer's own keys and Go to, the one chord that still reaches past it.
/// While a surface stands over the page (`surface`) the page's keys do nothing: the line gives
/// the surface's own. Otherwise the keys of the page in view (when the model gives it any and
/// the destination is served), the `shown` places and Settings, then Ask and Go to. A place
/// whose key the page takes for itself is left out rather than listed twice with two meanings.
/// `session`: the legend of the session page on top for its focused surface (its composer's
/// keys while it has them), which replaces the page's and the composer's.
fn line(
    destination: Destination,
    live: Option<Live>,
    served: bool,
    typing: bool,
    surface: Option<Surface>,

    shown: &[Area],
    session: Option<Vec<(&'static str, &'static str)>>,
) -> Line {
    let go_to = Entry::new(PALETTE_CHORD, PALETTE_LABEL);
    if typing {
        let own = match &session {
            Some(legend) => legend
                .iter()
                .map(|&(keys, name)| Entry::new(chord(keys), name))
                .collect(),
            None => COMPOSER_KEYS
                .iter()
                .map(|hint| Entry::new(hint.keys, hint.action))
                .collect(),
        };
        return Line {
            own,
            destinations: Vec::new(),
            ask: None,
            go_to: Some(go_to),
            sidebar: None,
        };
    }
    if let Some(surface) = surface {
        return Line {
            own: surface
                .keys()
                .into_iter()
                .map(|(keys, name)| Entry::new(keys, name))
                .collect(),
            destinations: Vec::new(),
            ask: None,
            go_to: surface.reaches_past().then_some(go_to),
            sidebar: None,
        };
    }
    let legend = match session {
        Some(legend) => legend,
        None if served => key_legend_live(destination, live.as_ref()),
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
        .chain([Area::Settings])
        .filter(|item| !taken(item.key()))
        .map(|item| Entry::new(area_key(item), item.label()))
        .collect();
    Line {
        own: legend
            .into_iter()
            .map(|(keys, name)| Entry::new(chord(keys), name))
            .collect(),
        destinations,
        ask: Some(Entry::new(ASK_KEY, ASK_LABEL)),
        go_to: Some(go_to),
        sidebar: Some(Entry::new(SIDEBAR_CHORD, SIDEBAR_LABEL)),
    }
}

/// A key as the line spells it: a control chord as the terminal writes one (`^K`, as Go to's).
fn chord(keys: &str) -> String {
    keys.replace("Ctrl+", "^")
}

impl Line {
    /// The entries `fit` shows in the line's order, each with the index of its group; Go to
    /// says `more` when keys in focus were left out.
    fn entries(&self, fit: Fit) -> Vec<(usize, Entry)> {
        let mut shown: Vec<(usize, Entry)> = self.own[..fit.own.min(self.own.len())]
            .iter()
            .map(|entry| (0, entry.clone()))
            .collect();
        if fit.destinations {
            shown.extend(self.destinations.iter().map(|entry| (1, entry.clone())));
        }
        if fit.ask {
            shown.extend(self.ask.iter().map(|entry| (ACTIONS_GROUP, entry.clone())));
        }
        if let Some(go_to) = &self.go_to {
            let mut go_to = go_to.clone();
            if fit.own < self.own.len() {
                go_to.name = MORE_LABEL;
            }
            shown.push((ACTIONS_GROUP, go_to));
        }
        if fit.sidebar {
            shown.extend(
                self.sidebar
                    .iter()
                    .map(|entry| (ACTIONS_GROUP, entry.clone())),
            );
        }
        shown
    }

    /// The Cells `fit` takes: the line's margins, a Cell between two entries, and a line with a
    /// Cell each side between two groups.
    fn cells(&self, fit: Fit) -> usize {
        let mut cells = 2;
        let mut last = None;
        for (group, entry) in self.entries(fit) {
            cells += match last {
                None => 0,
                Some(previous) if previous == group => 1,
                Some(_) => 3,
            };
            cells += entry.cells();
            last = Some(group);
        }
        cells
    }

    /// What fits in `room`, whole pairs only: the keys in focus in their order while they fit
    /// (Go to kept at the end), then, once all of them show, Ask, the destinations and the
    /// sidebar's toggle, each while it fits.
    fn layout(&self, room: usize) -> Fit {
        let fits = |fit: Fit| self.cells(fit) <= room;
        let mut fit = Fit::default();
        while fit.own < self.own.len()
            && fits(Fit {
                own: fit.own + 1,
                ..fit
            })
        {
            fit.own += 1;
        }
        if fit.own == self.own.len() {
            let grows: [fn(&mut Fit); 3] = [
                |fit| fit.ask = true,
                |fit| fit.destinations = true,
                |fit| fit.sidebar = true,
            ];
            for grow in grows {
                let mut more = fit;
                grow(&mut more);
                if fits(more) {
                    fit = more;
                }
            }
        }
        fit
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
pub(super) fn key_line(app: AppState, typing: Memo<bool>, shown: Memo<Vec<Area>>) -> AnyElement {
    let route = computed(move || {
        let destination = app.destination();
        // The assistant workspace's landing in Home's place is not Home's list.
        let landing = destination == Destination::Home
            && app.workspace_mode() == WorkspaceMode::AssistantOnly;
        (destination, app.live_page(), app.data.get(), landing)
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
        let (destination, live, data, landing) = route.get();
        let shown = shown.get();
        // An unserved destination answers no keys of its own; disconnected, none is served.
        let served = data != DataState::Disconnected
            && !landing
            && app.available(destination) == crate::model::Availability::Ready;
        let line = line(
            destination,
            live,
            served,
            typing.get(),
            surface.get(),
            &shown,
            session.get(),
        );
        let fit = line.layout(room);
        let mut row = view()
            .h(term::rows(1.0))
            .flex_shrink_0()
            .flex_row()
            .items_center()
            .px(cells(1.0))
            .overflow_hidden();
        // A grid too narrow for even Go to shows no entry rather than a cut one.
        let entries = if line.cells(fit) <= room {
            line.entries(fit)
        } else {
            Vec::new()
        };
        let mut last = None;
        for (group, entry) in entries {
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
            row = row.child(term::legend(entry.keys, entry.name));
        }
        row.into_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The line of `legend` in focus on a session page.
    fn session_line(typing: bool, legend: Vec<(&'static str, &'static str)>) -> Line {
        line(
            Destination::Sessions,
            Some(Live::Session(crate::model::Id::new("B-k3x9"))),
            true,
            typing,
            None,
            &Area::PLACES,
            Some(legend),
        )
    }

    #[test]
    fn key_line_never_cuts_a_key_or_shows_a_destination_key_unnamed() {
        let views = [
            (Destination::Home, None),
            (Destination::Tasks, None),
            (Destination::Sessions, None),
            (Destination::Settings, None),
            (
                Destination::Tasks,
                Some(Live::Task(crate::model::Id::new("T-14"))),
            ),
            (
                Destination::Sessions,
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
                    &Area::PLACES,
                    live.as_ref()
                        .filter(|live| matches!(live, Live::Session(_)))
                        .map(|_| {
                            use crate::model::session::{Focus, Tab, legend};
                            without_go_to(legend(Focus::Composer, Tab::Chat, true))
                        }),
                );
                for room in 20..=240 {
                    let fit = line.layout(room);
                    assert!(
                        line.cells(fit) <= room,
                        "{destination:?} at {room}: {fit:?} overflows"
                    );
                    let destinations = line
                        .entries(fit)
                        .iter()
                        .filter(|(group, _)| *group == 1)
                        .count();
                    assert!(
                        destinations == 0 || destinations == line.destinations.len(),
                        "{destination:?} at {room}: the destinations show in part"
                    );
                }
                // The classic 80-column grid keeps every key in focus.
                assert_eq!(
                    line.layout(80).own,
                    line.own.len(),
                    "{destination:?} at 80 drops own keys"
                );
            }
        }
    }

    /// On a session view with many keys (the Board's) the line dropped every word, the app's
    /// own `/ Ask` and `^K Go to` too, and showed bare caps (`p   n   q  │  /   ^K`). A crowded
    /// line keeps whole pairs: the keys in focus first and in order, as many as fit, then
    /// `^K more` saying the palette has the rest.
    #[test]
    fn a_crowded_key_line_keeps_whole_pairs_and_never_a_bare_key() {
        let board = vec![
            ("esc", "chat"),
            ("j", "next task"),
            ("k", "previous task"),
            ("⏎", "open task"),
            ("r", "refresh board"),
            ("p", "review plan"),
            ("n", "plan board tasks"),
            ("q", "close the record"),
            ("Alt+H", "thinking"),
            ("Ctrl+O", "expand"),
            ("Alt+P", "next model"),
            ("Alt+E", "effort"),
        ];
        for typing in [false, true] {
            let line = session_line(typing, board.clone());
            for room in 20..=240 {
                let fit = line.layout(room);
                let shown = line.entries(fit);
                assert!(line.cells(fit) <= room, "at {room}: {fit:?} overflows");
                assert!(
                    shown.iter().all(|(_, entry)| !entry.name.is_empty()),
                    "at {room}: a key shows without its word"
                );
                let own: Vec<&str> = shown
                    .iter()
                    .filter(|(group, _)| *group == 0)
                    .map(|(_, entry)| entry.keys.as_str())
                    .collect();
                let first: Vec<&str> = line.own[..own.len()]
                    .iter()
                    .map(|entry| entry.keys.as_str())
                    .collect();
                assert_eq!(own, first, "at {room}: the keys in focus lost their order");
                let last = shown
                    .last()
                    .map(|(_, entry)| (entry.keys.as_str(), entry.name));
                if own.len() < line.own.len() {
                    assert_eq!(
                        last,
                        Some((PALETTE_CHORD, MORE_LABEL)),
                        "at {room}: keys were left out without `^K more`"
                    );
                }
            }
            let at_140 = line.layout(140);
            assert!(at_140.own >= 6, "140 Cells show the Board's first keys");
        }
    }

    /// With a menu, a sheet or a form open the key line listed the page's keys, which do
    /// nothing then.
    #[test]
    fn an_open_surface_s_keys_replace_the_page_s() {
        let line = line(
            Destination::Tasks,
            None,
            true,
            false,
            Some(Surface::Menu),
            &Area::PLACES,
            None,
        );
        let keys: Vec<&str> = line.own.iter().map(|entry| entry.keys.as_str()).collect();
        let menu: Vec<&str> = Surface::Menu.keys().iter().map(|(keys, _)| *keys).collect();
        assert_eq!(keys, menu);
        assert!(line.destinations.is_empty() && line.ask.is_none() && line.go_to.is_none());
    }
}
