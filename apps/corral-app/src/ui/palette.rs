//! The command palette: a dialog over the window on desktop, a sheet on the phone, the content
//! pane turned into it in a terminal. It renders the model's rows in their groups and delegates
//! selection to AppState; filtering, wording and actions live in `model::commands`.

use nsh_base::{CommandEntry, CommandGroup, CommandItem, SheetEvent, command, dialog, sheet};
use nsh_core::prelude::*;

use crate::model::commands::{
    self as palette_model, Action, PALETTE_FOOTER, PALETTE_HINTS, PALETTE_PLACEHOLDER,
    palette_empty,
};
use crate::model::{Destination, Form, Overlay};
use crate::state::AppState;
use crate::ui::atoms::key_hint;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::text::{rich, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

/// The rows of the terminal grid the palette's list leaves to the rest: the top bar, the key line,
/// the frame's two lines, the palette's head, its search field and the line under it.
const OUTSIDE_LIST_ROWS: usize = 7;

/// The model's command palette in the existing Dialog (Desktop, Terminal) or Sheet (Phone).
/// Running a row closes the palette before applying its model action.
pub fn palette(app: AppState) -> AnyElement {
    match app.form {
        Form::Phone => sheet()
            .open(move || Some(app.is_open(Overlay::Palette)))
            .close_on_swipe(true)
            .on_open_change(move |_: &SheetEvent| app.close(Overlay::Palette))
            .backdrop_style(|s| s.bg(color(Token::Shell).opacity(0.5)))
            // Made on each opening: the query starts empty and nothing lives while it is closed.
            // Built untracked, so a signal the build reads cannot reset an open palette.
            .child(dynamic(move || untracked(|| phone_panel(app))))
            .into_element(),
        Form::Desktop | Form::Terminal => when(
            move || app.is_open(Overlay::Palette),
            move || untracked(|| dialog_panel(app)),
        )
        .into_element(),
    }
}

/// A dialog over the window: a backdrop that closes it on a press, the palette near the top.
fn dialog_panel(app: AppState) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let backdrop = view()
        .absolute()
        .inset_0()
        .focusable(false)
        .accessibility_elements_hidden(true)
        .on_press(move || app.close(Overlay::Palette));
    // A terminal has no translucency: a tinted backdrop would blank the screen behind.
    let backdrop = if terminal {
        backdrop
    } else {
        backdrop.bg(color(Token::Shell).opacity(0.35))
    };
    let panel = view()
        .self_center()
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule2))
        .child(command_list(app));
    let panel = if terminal {
        terminal_place(panel)
    } else {
        panel
            .w(px(600.0))
            .mt(px(110.0))
            .rounded(px(12.0))
            .overflow_hidden()
            .shadow_2xl()
            .shadow_color(color(Token::Shell))
            .into_element()
    };
    dialog()
        .label("Commands")
        .animation(ModalAnimation::Fade)
        .on_cancel(move || app.close(Overlay::Palette))
        .flex_1()
        .child(backdrop)
        .child(panel)
        .into_element()
}

/// Where the terminal palette stands: over the content, its frame on the content frame's own
/// lines, so it reads as the content pane turned into the palette while the sidebar's Go to row
/// stays in sight as the row that opened it. NSH cuts no words behind a dialog (it clears what
/// it knows of the page's text before drawing one), so a box anywhere else would leave the
/// halves of the page's words beside it; over the whole pane there are none.
fn terminal_place(panel: View) -> AnyElement {
    panel
        .absolute()
        .top(term::rows(2.0))
        .bottom(term::rows(2.0))
        .left(term::cells(term::sidebar_columns() + 1.0))
        .right(term::cells(1.0))
        .into_element()
}

/// The sheet's panel: a grab handle and the palette, riding on the software keyboard.
fn phone_panel(app: AppState) -> AnyElement {
    view()
        .pb_keyboard()
        .bg(color(Token::Surface))
        .rounded_t(px(14.0))
        .border_t(px(1.0))
        .border_color(color(Token::Rule2))
        .shadow_2xl()
        .shadow_color(color(Token::Shell))
        .child(
            view()
                .pb(move || {
                    px(8.0)
                        + if keyboard_height() > px(0.0) {
                            px(0.0)
                        } else {
                            safe_area().bottom
                        }
                })
                .child(
                    view()
                        .self_center()
                        .w(px(36.0))
                        .h(px(5.0))
                        .mt(px(8.0))
                        .mb(px(4.0))
                        .rounded(px(3.0))
                        .bg(color(Token::Rule2))
                        .accessibility_elements_hidden(true),
                )
                .child(command_list(app)),
        )
        .into_element()
}

/// The `Command` itself, with the app's look on every part: the search field after a search
/// icon, the rows under their group captions, the model's message when nothing matches, the
/// keys under the list on desktop and in the head in a terminal. The query is held here so the
/// item list can offer the free-text Ask row for text that names no command.
fn command_list(app: AppState) -> AnyElement {
    let form = app.form;
    let query = signal(String::new());
    let highlighted = signal(None::<SharedString>);
    let (bar_height, search_size, list_height) = match form {
        Form::Desktop => (48.0, 15.0, 380.0),
        Form::Phone => (52.0, 16.0, 320.0),
        Form::Terminal => (
            16.0,
            text_size(form),
            term::rows(term::grid_rows().saturating_sub(OUTSIDE_LIST_ROWS) as f32).0,
        ),
    };
    let list = command()
        .items(move || commands(app, &query.get()))
        .query(move || Some(SharedString::from(query.get())))
        .on_query_change(move |q: &SharedString| query.set(q.to_string()))
        .on_highlight_change(move |id: &Option<SharedString>| highlighted.set(id.clone()))
        .on_select(move |id: &SharedString| run(app, id, &query.get_untracked()))
        .on_cancel(move || app.close(Overlay::Palette))
        .auto_focus(true)
        .placeholder(PALETTE_PLACEHOLDER)
        .accessibility_label("Commands")
        .search_accessibility_label("Search commands")
        .separator(false)
        .search_bar_style(move |s| {
            let s = s.flex_row().items_center().h(px(bar_height));
            if form == Form::Terminal {
                s.px(px(8.0))
            } else {
                s.px(px(14.0))
                    .gap(px(10.0))
                    .border_b(px(1.0))
                    .border_color(color(Token::Rule))
            }
        })
        .search_style(move |s| {
            s.flex_1()
                .h(px(bar_height))
                .font_family(FONT_TEXT)
                .text_size(px(search_size))
                .text_color(color(Token::Ink))
        })
        .list_style(move |s| {
            let s = s.max_h(px(list_height));
            if form == Form::Terminal {
                s
            } else {
                s.p(px(6.0))
            }
        })
        .render_item(move |entry: &CommandEntry| row(app, entry, highlighted))
        .render_group(move |group: &CommandGroup| group_header(form, group))
        .empty(dynamic(move || empty(app, &query.get())))
        .when(form != Form::Terminal, |c| {
            c.search_prefix(icon(Icon::Search, 16.0, color(Token::Ink3)))
        })
        .when(form == Form::Desktop, |c| c.footer(footer()))
        .when(form == Form::Terminal, |c| {
            c.header(terminal_header()).search_prefix(
                view()
                    .w(term::cells(2.0))
                    .flex_shrink_0()
                    .child(text("›").text_color(color(Token::Ink3))),
            )
        });
    // `Command` takes no test id of its own; the search field is the only input inside this view,
    // which is where tests type into the palette.
    view().test_id("palette-input").child(list).into_element()
}

/// One command's row: its label, its key at the trailing edge where it has one. The one in effect
/// is `checked` for screen readers only, as the mock marks none.
fn row(
    app: AppState,
    entry: &CommandEntry,
    highlighted: Signal<Option<SharedString>>,
) -> AnyElement {
    let form = app.form;
    let size = text_size(form);
    let height = match form {
        Form::Desktop => 32.0,
        Form::Phone => 48.0,
        Form::Terminal => 16.0,
    };
    let content = view()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .h(px(height))
        .px(px(if form == Form::Terminal { 8.0 } else { 10.0 }))
        .child(
            text(entry.label.clone())
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(size))
                .text_color(color(Token::Ink)),
        )
        .when_some(key_of(&entry.id), |v, keys| {
            v.child(match form {
                // Plain and bold: a cap on every row would stack into a column of fills that
                // the highlighted row's band runs into.
                Form::Terminal => text(keys)
                    .flex_shrink_0()
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink))
                    .into_element(),
                Form::Desktop | Form::Phone => key_hint(keys),
            })
        });
    // A terminal draws the highlighted row's band itself.
    if form == Form::Terminal {
        return content.into_element();
    }
    let id = entry.id.clone();
    content
        .rounded(px(if form == Form::Phone { 10.0 } else { 6.0 }))
        .bg(move || {
            if highlighted.with(|h| h.as_ref() == Some(&id)) {
                color(Token::Sel)
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .into_element()
}

fn group_header(form: Form, group: &CommandGroup) -> AnyElement {
    if form == Form::Terminal {
        return view()
            .px(px(8.0))
            .child(
                text(group.heading.clone())
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink3)),
            )
            .into_element();
    }
    // `.pg`: spaced capitals in the faintest ink, so the captions sit behind the rows.
    view()
        .px(px(10.0))
        .pt(px(8.0))
        .pb(px(4.0))
        .child(
            text(group.heading.clone())
                .uppercase()
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(10.5))
                .letter_spacing(px(10.5 * 0.08))
                .text_color(color(Token::Ink4)),
        )
        .into_element()
}

/// The model's message while no row matches the query.
fn empty(app: AppState, query: &str) -> AnyElement {
    let message = palette_empty(query);
    match app.form {
        Form::Terminal => view().px(px(8.0)).child(rich(app, &message)).into_element(),
        Form::Desktop | Form::Phone => view()
            .px(px(14.0))
            .py(px(20.0))
            .max_w(px(420.0))
            .child(rich_small(app, &message))
            .into_element(),
    }
}

/// The terminal palette's first row: what it is, then its keys as the key line shows keys. The
/// title is its own text, so NSH sets no default title into the frame above it.
fn terminal_header() -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .h(term::rows(1.0))
        .px(px(8.0))
        .child(
            text("Commands")
                .flex_1()
                .number_of_lines(1u32)
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .children(PALETTE_HINTS.into_iter().map(|(keys, words)| {
            view()
                .flex_row()
                .child(term::space(2.0))
                .child(term::legend(keys, words))
        }))
        .into_element()
}

/// The palette's own keys under the list on desktop, the model's attribution at the trailing
/// edge.
fn footer() -> AnyElement {
    let hint = |keys: &'static str, words: &'static str| {
        view()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(key_hint(keys))
            .child(
                text(words)
                    .font_family(FONT_TEXT)
                    .text_size(px(11.0))
                    .text_color(color(Token::Ink3)),
            )
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .px(px(14.0))
        .py(px(8.0))
        .border_t(px(1.0))
        .border_color(color(Token::Rule))
        .children(PALETTE_HINTS.map(|(keys, words)| hint(keys, words)))
        .child(view().flex_1().min_w(px(0.0)))
        .child(
            text(PALETTE_FOOTER)
                .flex_shrink_0()
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(11.0))
                .text_color(color(Token::Ink3)),
        )
        .into_element()
}

/// Converts already-filtered model rows to the native command widget's input vocabulary.
fn commands(app: AppState, query: &str) -> Vec<CommandItem> {
    let here = app.destination();
    let section = app.nav.with(|nav| nav.settings);
    let theme = app.theme.get();
    let data = app.data.get();
    let conn = app.conn.get();
    let mode = app.mode.get();
    palette_model::filtered(query)
        .into_iter()
        .map(|command| {
            let checked = match command.action {
                Action::Go(destination) => destination == here,
                Action::Settings(item) => here == Destination::Settings && item == section,
                Action::Theme(item) => item == theme,
                Action::Data(item) => item == data,
                Action::Connection(item) => item == conn,
                Action::Mode(item) => item == mode,
                _ => false,
            };
            // Keep group matches and always-Ask rows through Command's second, label-only filter.
            CommandItem::new(command.id, command.label)
                .group(command.group)
                .checked(checked)
                .keywords([query.to_owned()])
        })
        .collect()
}

/// Resolves selection through the same pure command table that produced the displayed row.
fn run(app: AppState, id: &str, query: &str) {
    if let Some(action) = palette_model::run(id, query) {
        app.run_command(action);
    }
}

/// The key cap of the model row, without duplicating shortcut decisions in the renderer.
fn key_of(id: &str) -> Option<&'static str> {
    palette_model::shortcut(id)
}
