//! The terminal shell: the top bar on the first row, the sidebar and the content as two framed
//! regions under it, the toast's row while one shows, and the key line on the last row.
//! Everything is sized in whole Cells (8 x 16 points by default) and drawn in the terminal's own
//! font and colours; colour means state, and the structure is lines and weight (`ui::term`).

use nsh_core::prelude::*;

use crate::{
    model::{
        Destination, Overlay, Page, SettingsSection, Workspace,
        commands::key_legend,
        nav::{ASK_KEY, ASK_LABEL, PALETTE_LABEL, THIS_DEVICE},
    },
    screens,
    shell::{banner, confirmation, toast},
    state::AppState,
    ui::{
        ask, atoms, composer, palette,
        term::{self, cells},
        tokens::{Token, color},
    },
};

/// The palette's key as the terminal spells a control chord.
const PALETTE_CHORD: &str = "^K";

/// The terminal shell for `Form::Terminal`.
pub fn shell(app: AppState) -> AnyElement {
    // Overlays opening and closing change the nav state too; the sidebar rows follow the
    // destination alone.
    let destination = computed(move || app.destination());
    let workspace = computed(move || app.snapshot().workspace);
    view()
        .flex_1()
        .flex_col()
        .child(term::top_bar(app))
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .flex_row()
                .child(sidebar(app, destination, workspace))
                .child(content(app)),
        )
        .child(toast(app))
        .child(key_line(app))
        .child(ask::hotkey(app))
        .into_element()
}

/// The connection banner in the content frame's first rows, its rule joined to the frame; under
/// it the page with Ask beside it. The palette and the confirmation are dialogs standing on the
/// content frame's lines. On a narrow grid Ask takes the whole frame and the page steps out
/// while it is open: covering it would leave the page's bands showing under Ask's rows (an
/// opaque fill clears what was drawn beneath it, not its colours).
fn content(app: AppState) -> AnyElement {
    term::framed(
        view()
            .flex_1()
            .flex_col()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(banner(app))
            .child(
                view()
                    .flex_1()
                    .flex_row()
                    .relative()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .child(
                        view()
                            .flex_1()
                            .min_w(px(0.0))
                            .min_h(px(0.0))
                            .overflow_hidden()
                            .style_when(
                                move || app.is_open(Overlay::Ask) && !term::wide(),
                                |s| s.hidden(),
                            )
                            .child(screens::content(app)),
                    )
                    .child(composer::ask_surface(app)),
            )
            .child(palette::palette(app))
            .child(confirmation(app)),
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
        .child(dynamic(move || workspace.get().map(autopilot)))
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
        .w(move || cells(term::sidebar_columns()))
        .flex_shrink_0()
        .child(dynamic(move || {
            workspace.get().map(|ws| {
                term::top_title(
                    text(ws.name)
                        .number_of_lines(1u32)
                        .font_weight(FontWeight::BOLD)
                        .text_color(color(Token::Ink)),
                )
            })
        }))
        .child(dynamic(move || workspace.get().map(device)))
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
        let name = text(ws.device)
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

/// The page's own keys keep their names longest: they are the ones the page in view answers
/// to, and no other line of the screen lists them.
const KEEP_PAGE: u8 = 11;

/// How long each destination's name stays when the key line is short: the daily ones and
/// Settings (whose key, `,`, says nothing by itself) last.
fn keep(destination: Destination) -> u8 {
    match destination {
        Destination::Usage => 0,
        Destination::Timeline => 1,
        Destination::Plans => 2,
        Destination::Review => 3,
        Destination::Changes => 4,
        Destination::Agents => 5,
        Destination::Settings => 6,
        Destination::Now => 7,
        Destination::Inbox => 8,
    }
}

/// The key line's groups in order, a line between each two: the keys of the page in view (when
/// the model gives it any), the destinations, then Ask and Go to.
fn groups(destination: Destination, page: Option<Page>) -> Vec<Vec<Entry>> {
    let own: Vec<Entry> = key_legend(destination, page)
        .into_iter()
        .map(|(keys, name)| Entry {
            keys: String::from(keys),
            name,
            keep: KEEP_PAGE,
        })
        .collect();
    let destinations = Destination::ALL
        .into_iter()
        .map(|destination| Entry {
            keys: String::from(destination.key()),
            name: destination.label(),
            keep: keep(destination),
        })
        .collect();
    let actions = vec![
        Entry {
            keys: String::from(ASK_KEY),
            name: ASK_LABEL,
            keep: 10,
        },
        Entry {
            keys: String::from(PALETTE_CHORD),
            name: PALETTE_LABEL,
            keep: 9,
        },
    ];
    let mut groups = Vec::with_capacity(3);
    if !own.is_empty() {
        groups.push(own);
    }
    groups.push(destinations);
    groups.push(actions);
    groups
}

/// The Cells the key line takes with names from `keep` on: its margins, each entry a Cell
/// apart, and a line with a Cell each side between two groups.
fn line_cells(groups: &[Vec<Entry>], keep: u8) -> usize {
    let entries = groups.iter().flatten();
    2 + entries
        .clone()
        .map(|e| e.cells(e.keep >= keep))
        .sum::<usize>()
        + entries.count().saturating_sub(1)
        + 2 * groups.len().saturating_sub(1)
}

/// The last row, a legend: each key as a cap and the name it goes to, the page's own keys first.
/// When the grid is short of room the names give way one by one, the least used first; the
/// page's keep theirs.
fn key_line(app: AppState) -> AnyElement {
    let route = computed(move || (app.destination(), app.page()));
    dynamic(move || {
        let room = term::columns();
        let (destination, page) = route.get();
        let groups = groups(destination, page);
        let keep = (0..=KEEP_PAGE)
            .find(|&keep| line_cells(&groups, keep) <= room)
            .unwrap_or(KEEP_PAGE);
        let mut row = view()
            .h(term::rows(1.0))
            .flex_shrink_0()
            .flex_row()
            .items_center()
            .px(cells(1.0))
            .overflow_hidden();
        for (at, group) in groups.into_iter().enumerate() {
            if at > 0 {
                row = row
                    .child(term::space(1.0))
                    .child(term::quiet("│"))
                    .child(term::space(1.0));
            }
            for (index, entry) in group.into_iter().enumerate() {
                if index > 0 {
                    row = row.child(term::space(1.0));
                }
                row = row.child(if entry.keep >= keep {
                    term::legend(entry.keys, entry.name)
                } else {
                    term::key_cap(entry.keys)
                });
            }
        }
        row.into_element()
    })
}
