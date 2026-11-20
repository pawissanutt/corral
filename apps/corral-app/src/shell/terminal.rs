//! The terminal shell: the top bar on the first row, the sidebar and the content as two framed
//! regions under it, the toast's row while one shows, and the key line on the last row
//! (`terminal_keys`). The sidebar folds to a rail of keys and counts by choice (`mod+b`), on a
//! narrow grid, and while Ask docks beside the page.
//! Everything is sized in whole Cells (8 x 16 points by default) and drawn in the terminal's own
//! font and colours; colour means state, and the structure is lines and weight (`ui::term`).

use nsh_core::prelude::*;

use crate::{
    model::{
        Area, Overlay, SettingsSection, Workspace,
        commands::area_key,
        nav::{PALETTE_LABEL, THIS_DEVICE},
    },
    screens,
    shell::{
        activity_pages, confirmation, current_area, places, refusal, section_short, terminal_bar,
        terminal_keys::{PALETTE_CHORD, key_line},
        toast,
    },
    state::AppState,
    ui::{
        ask, atoms, composer,
        kit::{
            notice,
            scope::{ScopeFace, scope_switcher},
        },
        palette,
        term::{self, cells},
        tokens::{Token, color},
    },
};

/// The terminal shell for `Form::Terminal`.
pub fn shell(app: AppState) -> AnyElement {
    // Overlays opening and closing change the nav state too; the sidebar rows follow the
    // place alone.
    let current = computed(move || current_area(app));
    let shown = computed(move || places(app));
    let workspace = computed(move || app.snapshot().workspace);
    let collapsed = computed(move || term::sidebar_collapsed(app));
    let focus = Focus::watch();
    let typing = computed(move || focus.typing(app));
    // The top bar comes after the page in the tree, drawn above it: when the focused control
    // goes with what it did (an answer sent, an entry removed, a session opened in the list's
    // place), the next Tab starts in the page rather than on the top bar's chips.
    view()
        .flex_1()
        .flex_col_reverse()
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .flex_col()
                .child(
                    // The page comes first in the tree, the sidebar laid out to its left: the
                    // terminal's keys with nothing focused (Page Down, the arrows, Tab) start at
                    // the first scroll or control in the tree. Were that the sidebar's, the
                    // focus would land unseen on its selected row, banded already, and Space or
                    // Enter there would leave the page.
                    view()
                        .flex_1()
                        .min_h(px(0.0))
                        .flex_row_reverse()
                        .child(content(app, focus))
                        .child(dynamic(move || {
                            if collapsed.get() {
                                untracked(|| rail(app, current, shown))
                            } else {
                                untracked(|| sidebar(app, current, shown, workspace))
                            }
                        })),
                )
                .child(toast(app))
                .child(key_line(app, typing, shown)),
        )
        .child(terminal_bar::top_bar(app))
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
            .child(screens::agents::rewind_sheet(app))
            .child(screens::agents::remove_sheet(app))
            .child(screens::agents::start_sheet(app))
            .child(confirmation(app))
            .child(refusal(app))
            .child(crate::ui::kit::more::overlays(app)),
    )
    .flex_1()
    .into_element()
}

/// The workspace in the top line, its autopilot state under it; the scope switcher; the
/// places this workspace has with their counts and keys in columns, Activity's pages under
/// Activity while one shows; Go to and Settings (its sections under it while it is
/// selected) under a rule; this device in the bottom line. The rows scroll, so every one stays
/// reachable on a short grid.
fn sidebar(
    app: AppState,
    current: Memo<Area>,
    shown: Memo<Vec<Area>>,
    workspace: Memo<Option<Workspace>>,
) -> AnyElement {
    let rows = dynamic(move || {
        let shown = shown.get();
        untracked(move || {
            view()
                .children(
                    shown
                        .into_iter()
                        .map(move |item| place_row(app, current, item)),
                )
                .into_element()
        })
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
        .child(scope_switcher(app, ScopeFace::Sidebar))
        .child(term::hrule())
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .child(rows)
                    .child(term::hrule())
                    .child(atoms::nav_row(
                        app,
                        PALETTE_LABEL,
                        String::from("palette-open"),
                        move || app.is_open(Overlay::Palette),
                        Some(trailing(None, String::from(PALETTE_CHORD))),
                        move || app.open_palette(),
                    ))
                    .child(place_row(app, current, Area::Settings)),
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

/// The folded sidebar: the scope's name, each place as its key with its count beside it, then
/// Go to and Settings as their keys, the Settings sections' short names under Settings
/// while it is selected. Activity's pages need their names, so the folded sidebar leaves them
/// to the unfolded one (`^B`) and to Go to.
fn rail(app: AppState, current: Memo<Area>, shown: Memo<Vec<Area>>) -> AnyElement {
    let key_row = move |area: Area| {
        atoms::nav_row(
            app,
            area_key(area),
            format!("nav-{}", area.slug()),
            move || current.get() == area,
            Some(
                view()
                    .w(cells(3.0))
                    .flex_row()
                    .justify_end()
                    .children(area.counted().map(|counted| atoms::count(app, counted)))
                    .into_element(),
            ),
            move || app.go_area(area),
        )
    };
    let rows = dynamic(move || {
        let shown = shown.get();
        untracked(move || {
            view()
                .children(shown.into_iter().map(key_row))
                .into_element()
        })
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
            .child(rows)
            .child(term::hrule())
            .child(atoms::nav_row(
                app,
                PALETTE_CHORD,
                String::from("palette-open"),
                move || app.is_open(Overlay::Palette),
                None,
                move || app.open_palette(),
            ))
            .child(key_row(Area::Settings))
            .child(when(move || current.get() == Area::Settings, sections)),
    );
    let body = view()
        .flex_1()
        .min_h(px(0.0))
        .child(scope_switcher(app, ScopeFace::Rail))
        .child(term::hrule())
        .child(body);
    term::framed(body)
        .w(move || cells(term::sidebar_columns(app)))
        .flex_shrink_0()
        .into_element()
}

/// A place's row: its name, count and key. Activity brings its pages under it while one of
/// them shows, Settings its sections while it is the one shown.
fn place_row(app: AppState, current: Memo<Area>, item: Area) -> AnyElement {
    let row = atoms::nav_row(
        app,
        item.label(),
        format!("nav-{}", item.slug()),
        move || current.get() == item,
        Some(trailing(
            item.counted().map(|counted| atoms::count(app, counted)),
            String::from(area_key(item)),
        )),
        move || app.go_area(item),
    );
    match item {
        Area::Activity => view()
            .child(row)
            .child(when(
                move || current.get() == Area::Activity,
                move || activity_rows(app),
            ))
            .into_element(),
        Area::Settings => view()
            .child(row)
            .child(when(
                move || current.get() == Area::Settings,
                move || settings_sections(app),
            ))
            .into_element(),
        Area::Home | Area::Tasks | Area::Sessions => row,
    }
}

/// Activity's pages, two Cells in under Activity, as the Settings sections sit under Settings.
fn activity_rows(app: AppState) -> AnyElement {
    let pages = computed(move || activity_pages(app));
    dynamic(move || {
        let pages = pages.get();
        untracked(move || {
            view()
                .flex_row()
                .child(view().w(cells(2.0)).flex_shrink_0())
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .children(pages.into_iter().map(move |page| {
                            atoms::nav_row(
                                app,
                                page.label(),
                                format!("nav-{}", page.slug()),
                                move || app.destination() == page,
                                None,
                                move || app.go(page),
                            )
                        })),
                )
                .into_element()
        })
    })
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
        let Some(name) = ws.device.clone() else {
            return term::quiet(THIS_DEVICE.to_owned()).into_element();
        };
        let name = text(name.into_owned())
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
