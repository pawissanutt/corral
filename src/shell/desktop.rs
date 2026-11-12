//! The desktop and web shell: a top bar across the window, the sidebar of
//! destinations, and the content container that holds the current page with
//! the Ask drawer and the palette over it.

use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::{Destination, SettingsSection, Workspace};
use crate::screens;
use crate::state::AppState;
use crate::ui::atoms::{HealthLayout, count, health_line, key_hint, nav_row, nav_row_icon};
use crate::ui::composer::ask_surface;
use crate::ui::icons::{Icon, icon};
use crate::ui::palette::palette;
use crate::ui::tokens::{FONT_MONO, Token, color, row_height, text_size};

const TOP_BAR_HEIGHT: f32 = 38.0;
const SIDEBAR_WIDTH: f32 = 208.0;
const RADIUS_CONTROL: f32 = 6.0;
const FOCUS_RING: f32 = 2.0;
/// Settings sections sit past the width of a destination's icon and the gap after it (16 + 10),
/// so their labels line up with the destinations' labels.
const SECTION_INDENT: f32 = 26.0;
/// The workspace path gives up its width long before the health line gives up any of its own:
/// flex shrinking is weighted by this factor against the health line's 1.
const PATH_SHRINK: f32 = 1000.0;
/// Narrower than this the path shows a few characters and an ellipsis, which says nothing.
const PATH_MIN_WIDTH: f32 = 56.0;

pub fn shell(app: AppState) -> AnyElement {
    // Memos so a change elsewhere in the snapshot or the nav state (an overlay
    // opening) does not rebuild the page or the workspace blocks.
    let destination = computed(move || app.destination());
    let workspace = computed(move || app.snapshot().workspace);
    view()
        .flex_1()
        .min_h(px(0.0))
        .bg(color(Token::Shell))
        .child(top_bar(app, workspace))
        .child(
            view()
                .flex_1()
                .flex_row()
                .min_h(px(0.0))
                .child(sidebar(app, destination, workspace))
                .child(content(app, destination)),
        )
        .into_element()
}

fn top_bar(app: AppState, workspace: Memo<Option<Workspace>>) -> AnyElement {
    view()
        .h(px(TOP_BAR_HEIGHT))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .gap(px(14.0))
        .pl(px(14.0))
        .pr(px(10.0))
        .bg(color(Token::Shell))
        .border_b(px(1.0))
        .border_b_color(color(Token::ShellInk).opacity(0.1))
        .child(
            view()
                .flex_row()
                .flex_shrink_0()
                .items_center()
                .gap(px(8.0))
                .child(icon(Icon::Brand, 18.0, color(Token::ShellInk)))
                .child(
                    text("Corral")
                        .text_size(px(text_size(app.form) - 1.0))
                        .font_weight(FontWeight::BOLD)
                        .letter_spacing(px(0.1))
                        .text_color(color(Token::ShellInk)),
                ),
        )
        .child(dynamic(move || {
            workspace.get().map(|ws| workspace_path(app, ws))
        }))
        // Sized from the health line's own width (not from zero) so that, when the bar runs
        // short, the path shrinks first and the health line keeps its room.
        .child(
            view()
                .flex_auto()
                .min_w(px(0.0))
                .overflow_hidden()
                .child(health_line(app, HealthLayout::Bar)),
        )
        .into_element()
}

/// Shrinks with an ellipsis as the bar runs short, then hides once too narrow to read; it keeps
/// its place, so hiding it never changes the room the health line gets.
fn workspace_path(app: AppState, ws: Workspace) -> AnyElement {
    let cramped = signal(false);
    text(ws.path)
        .flex_shrink(PATH_SHRINK)
        .min_w(px(0.0))
        .truncate()
        .font_family(FONT_MONO)
        .text_size(px(text_size(app.form) - 1.5))
        .text_color(color(Token::ShellInk2))
        .on_layout(move |frame: &Bounds<Points>| {
            cramped.set(frame.size.width.0 < PATH_MIN_WIDTH);
        })
        .style_when(move || cramped.get(), |s| s.invisible())
        .into_element()
}

fn sidebar(
    app: AppState,
    destination: Memo<Destination>,
    workspace: Memo<Option<Workspace>>,
) -> AnyElement {
    let rows = Destination::ALL.map(|d| destination_row(app, destination, d));
    view()
        .w(px(SIDEBAR_WIDTH))
        .flex_shrink_0()
        .min_h(px(0.0))
        .pt(px(10.0))
        .pb(px(10.0))
        .px(px(8.0))
        .gap(px(2.0))
        .bg(color(Token::Shell))
        .child(dynamic(move || workspace.get().map(|ws| project(app, ws))))
        .child(ask_button(app))
        .child(palette_opener(app))
        .child(
            scroll()
                .flex_1()
                .min_h(px(0.0))
                .child(view().gap(px(2.0)).children(rows).child(when(
                    move || destination.get() == Destination::Settings,
                    move || settings_sections(app),
                ))),
        )
        .child(dynamic(move || {
            workspace.get().map(|ws| device_footer(app, ws))
        }))
        .into_element()
}

fn project(app: AppState, ws: Workspace) -> AnyElement {
    let base = text_size(app.form);
    view()
        .pt(px(6.0))
        .pb(px(12.0))
        .px(px(8.0))
        .gap(px(2.0))
        .child(
            text(ws.name)
                .truncate()
                .text_size(px(base))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::ShellInk)),
        )
        .child(
            text(ws.autopilot)
                .number_of_lines(2u32)
                .text_size(px(base - 2.0))
                .text_color(color(Token::ShellInk2)),
        )
        .into_element()
}

fn ask_button(app: AppState) -> AnyElement {
    let ink = color(Token::ShellInk);
    view()
        .h(px(row_height(app.form) + 4.0))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(8.0))
        .mb(px(6.0))
        .rounded(px(RADIUS_CONTROL))
        .bg(ink.opacity(0.1))
        .cursor_pointer()
        .transition_colors()
        .duration(120.0_f32)
        .hover(move |s| s.bg(ink.opacity(0.16)))
        .active(move |s| s.bg(ink.opacity(0.2)))
        .focus_visible(focus_ring)
        .role(AccessibilityRole::Button)
        .label("Ask")
        .test_id("ask-open")
        .on_press(move || app.open_ask())
        .child(icon(Icon::Ask, 16.0, ink))
        .child(
            text("Ask…")
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(text_size(app.form)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ink),
        )
        .child(key_hint("/"))
        .into_element()
}

fn palette_opener(app: AppState) -> AnyElement {
    let ink = color(Token::ShellInk);
    let quiet = color(Token::ShellInk2);
    view()
        .h(px(row_height(app.form)))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(8.0))
        .mb(px(10.0))
        .rounded(px(RADIUS_CONTROL))
        .border(px(1.0))
        .border_color(ink.opacity(0.16))
        .cursor_pointer()
        .transition_colors()
        .duration(120.0_f32)
        .hover(move |s| s.bg(ink.opacity(0.06)).border_color(ink.opacity(0.28)))
        .active(move |s| s.bg(ink.opacity(0.1)))
        .focus_visible(focus_ring)
        .role(AccessibilityRole::Button)
        .label("Go to, answer, open")
        .test_id("palette-open")
        .on_press(move || app.open_palette())
        .child(icon(Icon::Search, 16.0, quiet))
        .child(
            text("Go to, answer, …")
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(text_size(app.form)))
                .text_color(quiet),
        )
        .child(key_hint("Ctrl K"))
        .into_element()
}

fn destination_row(app: AppState, destination: Memo<Destination>, d: Destination) -> AnyElement {
    let trailing = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(count(app, d))
        .child(
            text(d.key().to_string())
                .w(px(10.0))
                .font_family(FONT_MONO)
                .text_size(px(text_size(app.form) - 3.0))
                .text_color(color(Token::ShellInk2))
                .accessibility_elements_hidden(true),
        )
        .into_element();
    nav_row_icon(
        app,
        Icon::destination(d),
        d.label(),
        format!("nav-{}", d.slug()),
        move || destination.get() == d,
        Some(trailing),
        move || app.go(d),
    )
}

fn settings_sections(app: AppState) -> AnyElement {
    view()
        .pl(px(SECTION_INDENT))
        .gap(px(2.0))
        .children(SettingsSection::ALL.map(|section| {
            nav_row(
                app,
                section.label(),
                format!("settings-{}", section.slug()),
                move || {
                    app.nav.with(|nav| {
                        nav.destination == Destination::Settings && nav.settings == section
                    })
                },
                None,
                move || app.go_settings(section),
            )
        }))
        .into_element()
}

fn device_footer(app: AppState, ws: Workspace) -> AnyElement {
    let small = text_size(app.form) - 1.5;
    view()
        .flex_shrink_0()
        .mt(px(8.0))
        .pt(px(10.0))
        .pb(px(2.0))
        .px(px(8.0))
        .gap(px(6.0))
        .border_t(px(1.0))
        .border_t_color(color(Token::ShellInk).opacity(0.12))
        .child(
            text("This device ")
                .text_size(px(small))
                .text_color(color(Token::ShellInk2))
                .child(
                    text(ws.device)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(Token::ShellInk)),
                ),
        )
        .child(
            text("Actions sign as ")
                .text_size(px(small))
                .text_color(color(Token::ShellInk2))
                .child(text(ws.via).font_family(FONT_MONO)),
        )
        .into_element()
}

/// The page, then the Ask drawer and the palette, which the contract wants
/// as the container's last two children so they cover the page and nothing else.
fn content(app: AppState, destination: Memo<Destination>) -> AnyElement {
    view()
        .flex_1()
        .relative()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_hidden()
        .bg(color(Token::Bg))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .child(dynamic(move || {
                    let d = destination.get();
                    // The page's own reads must not rebuild it from here.
                    untracked(move || screens::destination(app, d))
                })),
        )
        .child(ask_surface(app))
        .child(palette(app))
        .into_element()
}

fn focus_ring(s: StyleRefinement) -> StyleRefinement {
    s.outline(px(FOCUS_RING))
        .outline_color(color(Token::Focus))
        .outline_offset(px(1.0))
}
