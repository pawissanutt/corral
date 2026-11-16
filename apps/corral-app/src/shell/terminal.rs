//! The terminal shell: the top bar on the first row, the sidebar and the content as two framed
//! regions under it, the toast's row while one shows, and the key line on the last row. The
//! sidebar folds to a rail of keys and counts by choice (`mod+b`), on a narrow grid, and while
//! Ask docks beside the page.
//! Everything is sized in whole Cells (8 x 16 points by default) and drawn in the terminal's own
//! font and colours; colour means state, and the structure is lines and weight (`ui::term`).

use nsh_core::prelude::*;

use crate::{
    model::{
        DataState, Destination, Live, Overlay, SettingsSection, Workspace,
        assistant::COMPOSER_KEYS,
        commands::{destination_key, key_legend_live},
        nav::{ASK_KEY, ASK_LABEL, PALETTE_LABEL, SIDEBAR_LABEL, THIS_DEVICE},
    },
    screens,
    shell::{confirmation, section_short, toast},
    state::AppState,
    ui::{
        ask, atoms, composer,
        kit::notice,
        palette,
        term::{self, cells},
        tokens::{Token, color},
    },
};

/// The palette's key as the terminal spells a control chord.
const PALETTE_CHORD: &str = "^K";
/// The sidebar's toggle (`mod+b`) as the terminal spells it.
const SIDEBAR_CHORD: &str = "^B";

/// The terminal shell for `Form::Terminal`.
pub fn shell(app: AppState) -> AnyElement {
    // Overlays opening and closing change the nav state too; the sidebar rows follow the
    // destination alone.
    let destination = computed(move || app.destination());
    let workspace = computed(move || app.snapshot().workspace);
    let collapsed = computed(move || term::sidebar_collapsed(app));
    let focus = Focus::watch();
    let typing = computed(move || focus.typing(app));
    view()
        .flex_1()
        .flex_col()
        .child(term::top_bar(app))
        .child(
            // The page comes first in the tree, the sidebar laid out to its left: the terminal's
            // keys with nothing focused (Page Down, the arrows, Tab) start at the first scroll or
            // control in the tree. Were that the sidebar's, the focus would land unseen on its
            // selected row, banded already, and Space or Enter there would leave the page.
            view()
                .flex_1()
                .min_h(px(0.0))
                .flex_row_reverse()
                .child(content(app, focus))
                .child(dynamic(move || {
                    if collapsed.get() {
                        untracked(|| rail(app, destination))
                    } else {
                        untracked(|| sidebar(app, destination, workspace))
                    }
                })),
        )
        .child(toast(app))
        .child(key_line(app, typing))
        .child(ask::hotkey(app))
        .into_element()
}

/// Where the keyboard focus is: anywhere in the content row (the page and Ask beside it), and in
/// the page.
#[derive(Clone, Copy)]
struct Focus {
    row: ElementRef,
    page: ElementRef,
    in_row: Signal<bool>,
    in_page: Signal<bool>,
}

impl Focus {
    fn watch() -> Focus {
        let focus = Focus {
            row: element_ref(),
            page: element_ref(),
            in_row: signal(false),
            in_page: signal(false),
        };
        focus.row.on_focus_in(move || focus.in_row.set(true));
        focus.row.on_focus_out(move || focus.in_row.set(false));
        focus.page.on_focus_in(move || focus.in_page.set(true));
        focus.page.on_focus_out(move || focus.in_page.set(false));
        focus
    }

    /// Tracked: whether Ask's composer has the keys, so letters typed go into its field and not
    /// to the page or the destinations: Ask is open and the focus is in the row, outside the page.
    fn typing(self, app: AppState) -> bool {
        app.is_open(Overlay::Ask) && self.in_row.get() && !self.in_page.get()
    }
}

/// The connection banner in the content frame's first rows, its rule joined to the frame; under
/// it the page with Ask beside it. The palette and the confirmation are dialogs standing on the
/// content frame's lines. Where Ask cannot dock (`term::ask_docked`) it takes the whole frame and
/// the page steps out while it is open: covering it would leave the page's bands showing under
/// Ask's rows (an opaque fill clears what was drawn beneath it, not its colours).
fn content(app: AppState, focus: Focus) -> AnyElement {
    term::framed(
        view()
            .flex_1()
            .flex_col()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(notice::connection(app))
            .child(
                view()
                    .element_ref(&focus.row)
                    .flex_1()
                    .flex_row()
                    .relative()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .child(
                        view()
                            .element_ref(&focus.page)
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .overflow_hidden()
                            .style_when(
                                move || app.is_open(Overlay::Ask) && !term::ask_docked(app),
                                |s| s.hidden(),
                            )
                            .child(screens::content(app)),
                    )
                    .child(composer::ask_surface(app)),
            )
            .child(palette::palette(app))
            .child(confirmation(app))
            .child(screens::now::verb_composer(app)),
    )
    .flex_1()
    .into_element()
}

/// The workspace in the top line, its autopilot state under it; the nine destinations with
/// their counts and keys in columns, the Settings sections under Settings while it is
/// selected; Ask and Go to under a rule; this device in the bottom line. The rows scroll, so
/// every one stays reachable on a short grid.
fn sidebar(
    app: AppState,
    destination: Memo<Destination>,
    workspace: Memo<Option<Workspace>>,
) -> AnyElement {
    let rows = Destination::ALL.into_iter().map(move |item| {
        let row = atoms::nav_row(
            app,
            item.label(),
            format!("nav-{}", item.slug()),
            move || destination.get() == item,
            Some(trailing(
                Some(atoms::count(app, item)),
                String::from(item.key()),
            )),
            move || app.go(item),
        );
        if item == Destination::Settings {
            view()
                .child(row)
                .child(when(
                    move || destination.get() == Destination::Settings,
                    move || settings_sections(app),
                ))
                .into_element()
        } else {
            row
        }
    });
    let body = view()
        .flex_1()
        .min_h(px(0.0))
        .child(dynamic(move || {
            workspace
                .get()
                .filter(|ws| !ws.autopilot.is_empty())
                .map(autopilot)
        }))
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .children(rows)
                    .child(term::hrule())
                    .child(atoms::nav_row(
                        app,
                        ASK_LABEL,
                        String::from("ask-open"),
                        move || app.is_open(Overlay::Ask),
                        Some(trailing(None, String::from(ASK_KEY))),
                        move || app.open_ask(),
                    ))
                    .child(atoms::nav_row(
                        app,
                        PALETTE_LABEL,
                        String::from("palette-open"),
                        move || app.is_open(Overlay::Palette),
                        Some(trailing(None, String::from(PALETTE_CHORD))),
                        move || app.open_palette(),
                    )),
            ),
        );
    term::framed(body)
        .w(move || cells(term::sidebar_columns(app)))
        .flex_shrink_0()
        .child(dynamic(move || {
            workspace.get().map(|ws| {
                term::top_title(
                    text(ws.name.into_owned())
                        .number_of_lines(1u32)
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink)),
                )
            })
        }))
        .child(dynamic(move || workspace.get().map(device)))
        .into_element()
}

/// The folded sidebar: each destination as its key, its count beside it, the Settings
/// sections' short names under Settings while it is selected, then Ask and Go to as their keys.
fn rail(app: AppState, destination: Memo<Destination>) -> AnyElement {
    let rows = Destination::ALL.into_iter().map(move |item| {
        atoms::nav_row(
            app,
            destination_key(item).unwrap_or_default(),
            format!("nav-{}", item.slug()),
            move || destination.get() == item,
            Some(
                view()
                    .w(cells(3.0))
                    .flex_row()
                    .justify_end()
                    .child(atoms::count(app, item))
                    .into_element(),
            ),
            move || app.go(item),
        )
    });
    let sections = move || {
        view()
            .children(SettingsSection::ALL.into_iter().map(move |section| {
                atoms::nav_row(
                    app,
                    section_short(section),
                    format!("settings-{}", section.slug()),
                    move || app.nav.with(|nav| nav.settings == section),
                    None,
                    move || app.go_settings(section),
                )
            }))
            .into_element()
    };
    let body = scroll().flex_1().min_h(px(0.0)).child(
        view()
            .children(rows)
            .child(when(
                move || destination.get() == Destination::Settings,
                sections,
            ))
            .child(term::hrule())
            .child(atoms::nav_row(
                app,
                ASK_KEY,
                String::from("ask-open"),
                move || app.is_open(Overlay::Ask),
                None,
                move || app.open_ask(),
            ))
            .child(atoms::nav_row(
                app,
                PALETTE_CHORD,
                String::from("palette-open"),
                move || app.is_open(Overlay::Palette),
                None,
                move || app.open_palette(),
            )),
    );
    term::framed(view().flex_1().min_h(px(0.0)).child(body))
        .w(move || cells(term::sidebar_columns(app)))
        .flex_shrink_0()
        .into_element()
}

/// The autopilot state under the workspace's name: its first part as it reads in the line
/// (`Autopilot`), what it means for the work after it, quieter; a rule under both.
fn autopilot(ws: Workspace) -> AnyElement {
    let (state, detail) = ws.autopilot.split_once(" · ").unwrap_or((ws.autopilot, ""));
    view()
        .child(
            view()
                .px(cells(1.0))
                .child(text(state).text_color(color(Token::Ink)))
                .when(!detail.is_empty(), |v| {
                    v.child(
                        text(detail)
                            .number_of_lines(2u32)
                            .text_color(color(Token::Ink3)),
                    )
                }),
        )
        .child(term::hrule())
        .into_element()
}

/// This device, in the sidebar's bottom line: with its caption where the line has room.
fn device(ws: Workspace) -> AnyElement {
    term::bottom_title(dynamic(move || {
        let name = text(ws.device.clone().into_owned())
            .font_weight(FontWeight::BOLD)
            .text_color(color(Token::Ink));
        if term::wide() {
            term::quiet(format!("{THIS_DEVICE} "))
                .child(name)
                .into_element()
        } else {
            name.number_of_lines(1u32).into_element()
        }
    }))
}

/// A row's count and key in two right-aligned columns, so every count and every key lines up.
fn trailing(count: Option<AnyElement>, key: String) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .child(
            view()
                .w(cells(3.0))
                .flex_row()
                .justify_end()
                .children(count),
        )
        .child(term::space(2.0))
        .child(
            view()
                .w(cells(2.0))
                .flex_row()
                .justify_end()
                .child(term::quiet(key)),
        )
        .child(term::space(1.0))
        .into_element()
}

/// The seven Settings sections, two Cells in under Settings. The indent is a column of its own:
/// padding on a terminal box of rows can come out as one Cell.
fn settings_sections(app: AppState) -> AnyElement {
    view()
        .flex_row()
        .child(view().w(cells(2.0)).flex_shrink_0())
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .children(SettingsSection::ALL.into_iter().map(move |section| {
                    atoms::nav_row(
                        app,
                        section.label(),
                        format!("settings-{}", section.slug()),
                        move || app.nav.with(|nav| nav.settings == section),
                        None,
                        move || app.go_settings(section),
                    )
                })),
        )
        .into_element()
}

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

/// Go to's and Ask's names give way after every destination's.
const KEEP_GO_TO: u8 = 10;
const KEEP_ASK: u8 = 11;
/// The keys in focus keep their names longest: they are the ones the page in view (or Ask's
/// field) answers to, and no other line of the screen lists them.
const KEEP_PAGE: u8 = 12;

/// How long each destination's name stays when the key line is short: the daily ones and
/// Settings (whose key, `,`, says nothing by itself) last.
fn keep(destination: Destination) -> u8 {
    match destination {
        Destination::Builds => 0,
        Destination::Usage => 1,
        Destination::Timeline => 2,
        Destination::Plans => 3,
        Destination::Review => 4,
        Destination::Changes => 5,
        Destination::Agents => 6,
        Destination::Settings => 7,
        Destination::Now => 8,
        Destination::Inbox => 9,
    }
}

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
/// folds is in sight anyway. Then each destination's name gives way, the least used
/// first; then the destinations go whole (the sidebar, folded or not, lists their keys); then Go
/// to's and Ask's names; last the names of the keys in focus. A cap never gives way to a name.
fn fits() -> impl Iterator<Item = Fit> {
    std::iter::once(Fit {
        keep: 0,
        destinations: true,
        sidebar: true,
    })
    .chain((0..=KEEP_GO_TO).map(|keep| Fit {
        keep,
        destinations: true,
        sidebar: false,
    }))
    .chain((KEEP_GO_TO..=KEEP_PAGE + 1).map(|keep| Fit {
        keep,
        destinations: false,
        sidebar: false,
    }))
}

/// The key line for what is in view. While Ask's field has the keys, letters typed go into it:
/// the line gives the composer's own keys and Go to, the one chord that still reaches past it.
/// Otherwise the keys of the page in view (when the model gives it any and the destination is
/// served), the destinations, then Ask and Go to. `off_board`: Now's selection is off the board.
fn line(
    destination: Destination,
    live: Option<Live>,
    served: bool,
    typing: bool,
    off_board: bool,
) -> Line {
    let go_to = Entry::new(PALETTE_CHORD, PALETTE_LABEL, KEEP_GO_TO);
    if typing {
        return Line {
            own: COMPOSER_KEYS
                .iter()
                .map(|hint| Entry::new(hint.keys, hint.action, KEEP_PAGE))
                .collect(),
            destinations: Vec::new(),
            actions: vec![go_to],
            sidebar: None,
        };
    }
    let legend = if served {
        key_legend_live(destination, live.as_ref(), off_board)
    } else {
        Vec::new()
    };
    Line {
        own: legend
            .into_iter()
            .map(|(keys, name)| Entry::new(keys, name, KEEP_PAGE))
            .collect(),
        destinations: Destination::ALL
            .into_iter()
            .map(|item| Entry::new(item.key(), item.label(), keep(item)))
            .collect(),
        actions: vec![Entry::new(ASK_KEY, ASK_LABEL, KEEP_ASK), go_to],
        sidebar: Some(Entry::new(SIDEBAR_CHORD, SIDEBAR_LABEL, 0)),
    }
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

/// The last row, a legend: each key as a cap and the name it goes to, the keys in focus first.
/// When the grid is short of room the names give way one by one, then whole groups, as
/// [`fits`] orders them; a key's cap is never cut.
fn key_line(app: AppState, typing: Memo<bool>) -> AnyElement {
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
        )
    });
    dynamic(move || {
        let room = term::columns();
        let (destination, live, data, off_board) = route.get();
        // An unserved destination answers no keys of its own; disconnected, none is served.
        let served = data != DataState::Disconnected
            && app.available(destination) == crate::model::Availability::Ready;
        let line = line(destination, live, served, typing.get(), off_board);
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
    fn key_line_drops_names_and_groups_before_any_key_is_cut() {
        let views = [
            (Destination::Inbox, None),
            (Destination::Now, None),
            (Destination::Plans, None),
            (Destination::Review, None),
            (Destination::Settings, None),
            (
                Destination::Inbox,
                Some(Live::Question(crate::model::Id::new("T-868/R1/Q1"))),
            ),
        ];
        for (destination, live) in views {
            for typing in [false, true] {
                let line = line(destination, live.clone(), true, typing, false);
                for room in 20..=240 {
                    let (fit, count) = line.layout(room);
                    assert!(
                        line.cells(fit, count) <= room,
                        "{destination:?} at {room}: {fit:?} overflows"
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
}
