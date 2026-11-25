//! The side pane's chrome and its session tabs. One strip of tabs leads the pane: Ask's, then
//! each session kept there (a chat, a Code or Build session, a run of any role), each closed by
//! its ×, and the pane's own close at the end. Under it the chosen tab: Ask, or the session
//! under a head like Ask's (name, state, Open session ↗) with its own compact composer, never
//! Ask's draft or key.

use nsh_core::prelude::*;

use crate::model::status::SessionState;
use crate::model::{Id, Live, Overlay, dock};
use crate::screens::agents::{Compact, ask_session, ask_view};
use crate::state::AppState;
use crate::ui::atoms::control_height;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

const CLOSE_SIZE: f32 = 20.0;
const TAB_GAP: f32 = 2.0;

// Leave text room after the padding, mark and close, even when scrolling.
fn session_tab_min() -> f32 {
    44.0 + control_height(CLOSE_SIZE)
}

/// The strip over the pane: Ask's tab, the session tabs, then the pane's close.
pub(super) fn strip(app: AppState) -> AnyElement {
    let tabs = computed(move || app.dock_tabs());
    let ask = computed(move || app.is_open(Overlay::Ask));
    let shown = computed(move || app.shown_dock());
    let tab_min = session_tab_min();
    let room = signal(0.0_f32);
    let ask_width = signal(0.0_f32);
    let row = dynamic(move || {
        let tabs = tabs.get();
        untracked(|| {
            let session_count = tabs.len() as f32;
            let ask_tab = tab(
                TabFace {
                    test_id: "dock-tab-ask".into(),
                    label: dock::ASK_TAB.into(),
                    spoken: dock::ASK_TAB.into(),
                    lead: icon(Icon::Ask, 13.0, color(Token::Ink2)),
                    dim: false,
                },
                move || ask.get(),
                move || app.open_ask(),
                None,
                room,
            );
            view()
                .flex_row()
                .items_center()
                // Constrain flex shrink to the viewport until only the minimum tabs fit.
                .w(move || {
                    px(room
                        .get()
                        .max(ask_width.get() + session_count * (tab_min + TAB_GAP)))
                })
                .gap(px(TAB_GAP))
                .child(
                    view()
                        .flex_none()
                        .on_layout(move |bounds: &Bounds<Points>| {
                            ask_width.set(bounds.size.width.0);
                        })
                        .child(ask_tab),
                )
                .children(tabs.into_iter().map(|t| {
                    let (chosen, select, close) = (t.id.clone(), t.id.clone(), t.id.clone());
                    let on = computed(move || shown.with(|shown| shown.as_ref() == Some(&chosen)));
                    tab(
                        TabFace {
                            test_id: format!("dock-tab-{}", t.id),
                            label: t.label,
                            spoken: t.spoken,
                            lead: crate::ui::kit::status::session_mark(app, t.state),
                            dim: t.state == SessionState::Gone,
                        },
                        move || on.get(),
                        move || app.select_dock(&select),
                        Some(Box::new(move || app.undock(&close))),
                        room,
                    )
                }))
                .into_element()
        })
    });
    view()
        .flex_row()
        .items_center()
        .flex_none()
        .gap(px(4.0))
        .h(px(control_height(38.0)))
        .pl(px(6.0))
        .pr(px(6.0))
        .bg(color(Token::Surface))
        .border_b(px(1.0))
        .border_b_color(color(Token::Rule))
        .child(
            scroll()
                .test_id("dock-tab-strip")
                .horizontal()
                .scrollbar(false)
                .flex_1()
                .min_w(px(0.0))
                .on_layout(move |bounds: &Bounds<Points>| room.set(bounds.size.width.0))
                .child(row),
        )
        .child(
            view()
                .flex_none()
                .child(crate::screens::agents::pane_new_menu(app)),
        )
        .child(close_pane(app))
        .into_element()
}

struct TabFace {
    test_id: String,
    label: String,
    spoken: String,
    lead: AnyElement,
    dim: bool,
}

/// One tab: its mark and name, chosen in ink over a 2 pt underline as the page's tab strips
/// draw it; a session tab's × closes it, leaving the session running.
fn tab(
    face: TabFace,
    chosen: impl Fn() -> bool + Copy + 'static,
    pick: impl Fn() + 'static,
    close: Option<Box<dyn Fn()>>,
    room: Signal<f32>,
) -> AnyElement {
    let is_session = close.is_some();
    let tab_ref = element_ref();
    effect(move || {
        let _ = room.get();
        if chosen() {
            tab_ref.reveal(
                RevealOptions::default()
                    .vertical(ScrollAlignment::Nearest)
                    .animated(false),
            );
        }
    });
    let dim = face.dim;
    let words = text(face.label)
        .test_id(&format!("{}-text", face.test_id))
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0))
        .flex_shrink(1.0)
        .font_family(FONT_TEXT)
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(move || {
            if chosen() {
                color(Token::Ink)
            } else if dim {
                color(Token::Ink3)
            } else {
                color(Token::Ink2)
            }
        })
        .style_when(chosen, |s| s.font_weight(FontWeight::SEMIBOLD));
    let label = view()
        .test_id(&face.test_id)
        .role(AccessibilityRole::Button)
        .label(face.spoken.clone())
        .accessibility_state(move || AccessibilityState {
            selected: chosen(),
            ..AccessibilityState::default()
        })
        .on_press(pick)
        .cursor_pointer()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .min_w(px(0.0))
        .flex_shrink(1.0)
        .pl(px(8.0))
        .pr(px(if close.is_some() { 2.0 } else { 8.0 }))
        .h_full()
        .overflow_hidden()
        .child(view().flex_none().child(face.lead))
        .child(words);
    let close = close.map(|close| {
        view()
            .test_id(&format!("{}-close", face.test_id))
            .role(AccessibilityRole::Button)
            .label(format!("{} · {}", dock::CLOSE_TAB, face.spoken))
            .on_press(close)
            .cursor_pointer()
            .size(px(control_height(CLOSE_SIZE)))
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(4.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .child(icon(Icon::Close, 10.0, color(Token::Ink3)))
    });
    let frame = view().flex_row().items_center().element_ref(&tab_ref);
    let frame = if is_session {
        frame
            .min_w(px(session_tab_min()))
            .max_w(px(200.0))
            .flex_shrink(1.0)
    } else {
        frame.flex_none()
    };
    frame
        .h(px(control_height(30.0)))
        .pr(px(if close.is_some() { 4.0 } else { 0.0 }))
        .rounded_t(px(6.0))
        .border_b(px(2.0))
        .border_b_color(move || {
            if chosen() {
                color(Token::Ink)
            } else {
                color(Token::Surface)
            }
        })
        .hover(|s| s.bg(color(Token::Hover)))
        .tooltip(tip(face.spoken.into()))
        .child(label)
        .children(close)
        .into_element()
}

/// The pane's close: the close icon, named with its keys for screen readers and the hint.
fn close_pane(app: AppState) -> AnyElement {
    view()
        .test_id("pane-close")
        .role(AccessibilityRole::Button)
        .label(format!("{} (Esc, {})", dock::HIDE, dock::KEY))
        .tooltip(tip(format!("{} · {}", dock::HIDE, dock::KEY).into()))
        .on_press(move || app.close_pane())
        .size(px(control_height(28.0)))
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .active(|s| s.bg(color(Token::Sel)))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(2.0))
        })
        .child(icon(Icon::Close, 14.0, color(Token::Ink2)))
        .into_element()
}

/// The session tabs' bodies. A tab mounts the first time it is chosen and stays mounted, hidden,
/// while it is in the strip: going back to it resumes where it was (its transcript, scroll and
/// draft) instead of reading it again from the start. A source change starts them all afresh.
pub(super) fn pane(app: AppState) -> AnyElement {
    let selected = computed(move || app.shown_dock());
    let source = computed(move || app.dock_source());
    let strip = computed(move || {
        app.dock_tabs()
            .into_iter()
            .map(|tab| tab.id)
            .collect::<Vec<Id>>()
    });
    view()
        .test_id("dock-pane")
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .style_when(move || selected.get().is_none(), |s| s.hidden())
        .child(dynamic(move || {
            let _ = source.get();
            untracked(|| {
                let visited = signal(Vec::<Id>::new());
                effect(move || {
                    if let Some(id) = selected.get()
                        && !visited.with_untracked(|seen| seen.contains(&id))
                    {
                        visited.update(|seen| seen.push(id));
                    }
                });
                each(
                    move || {
                        let seen = visited.get();
                        strip.with(|ids| {
                            ids.iter()
                                .filter(|id| seen.contains(id))
                                .cloned()
                                .collect::<Vec<Id>>()
                        })
                    },
                    |id: &Id| id.clone(),
                    move |id, _| {
                        let id = id.get_untracked();
                        let chosen = id.clone();
                        let on = computed(move || selected.with(|s| s.as_ref() == Some(&chosen)));
                        view()
                            .flex_1()
                            .min_h(px(0.0))
                            .flex_col()
                            .style_when(move || !on.get(), |s| s.hidden())
                            .child(session(app, id))
                    },
                )
            })
        }))
        .into_element()
}

fn session(app: AppState, id: Id) -> AnyElement {
    let app = app.for_dock();
    keep_child_live(app, &id);
    let page = ask_view(app, id.clone());
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(head(app, id.clone(), page))
        .child(ask_session(app, id, page, Compact::Dock))
        .into_element()
}

/// A child agent's conversation is told apart only through its session's live stream (its fleet
/// and the `children` events that re-read the child's transcript), and its transcript is not
/// pinned by that stream: while its tab is mounted the pane holds both, whatever page shows.
fn keep_child_live(app: AppState, id: &Id) {
    let Some((parent, _)) = id.as_str().split_once("/child/") else {
        return;
    };
    let parent = Id::from(parent);
    let live = untracked(|| {
        app.feed.with(|feed| {
            crate::model::agents::session_line(feed, parent.as_str())
                .is_some_and(|line| line.state != SessionState::Gone)
        })
    });
    if live {
        crate::screens::agents::watch_transcript(app, parent);
    }
    let source = untracked(|| app.scope());
    let target = corral_protocol::detail::DetailRef::Session { id: id.to_string() };
    source.watch_detail(target.clone(), true);
    cleanup(move || source.watch_detail(target, false));
}

/// As Ask's head: the session's id and name, its role and state, then Open session ↗, which
/// draws the session's full views in the page while the conversation stays here.
fn head(app: AppState, id: Id, page: Memo<crate::model::session::SessionPage>) -> AnyElement {
    let header = computed(move || page.with(|p| p.header.clone()));
    let open = id.clone();
    view()
        .flex_row()
        .items_center()
        .flex_none()
        .gap(px(8.0))
        .h(px(40.0))
        .pl(px(14.0))
        .pr(px(10.0))
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .child(dynamic(move || {
            let h = header.get();
            untracked(|| {
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .flex_1()
                    .min_w(px(0.0))
                    .child(
                        text(h.shown_id.clone())
                            .number_of_lines(1u32)
                            .flex_shrink_0()
                            .font_family(FONT_MONO)
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color(Token::Ink2)),
                    )
                    .child(
                        text(h.name.clone())
                            .number_of_lines(1u32)
                            .truncate()
                            .min_w(px(0.0))
                            .flex_shrink(1.0)
                            .font_family(FONT_TEXT)
                            .text_size(px(13.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::Ink)),
                    )
                    .child(
                        text(h.role.clone())
                            .number_of_lines(1u32)
                            .flex_shrink_0()
                            .font_family(FONT_TEXT)
                            .text_size(px(12.5))
                            .text_color(color(Token::Ink3)),
                    )
                    .child(super::drawer::state_word(app, &h.state))
                    .into_element()
            })
        }))
        .child(button(
            app,
            dock::OPEN_PAGE,
            ButtonKind::Quiet,
            "btn-open-page",
            move || app.open_dock_page(Live::Session(open.clone())),
        ))
        .into_element()
}

fn tip(words: SharedString) -> AnyElement {
    view()
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(6.0))
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule2))
        .child(
            text(words)
                .font_family(FONT_TEXT)
                .text_size(px(11.5))
                .text_color(color(Token::Ink)),
        )
        .into_element()
}

/// Open in side pane beside a session's Open page (the Sessions inspector, Home's open card):
/// the Split icon alone, its words for screen readers. None for a task or off the desktop.
pub(crate) fn open_beside(app: AppState, live: &Live) -> Option<AnyElement> {
    let Live::Session(id) = live else {
        return None;
    };
    if !dock::supported(app.form) {
        return None;
    }
    let id = id.clone();
    Some(crate::ui::kit::button::button_with(
        app,
        dock::OPEN_BESIDE,
        ButtonKind::Quiet,
        crate::ui::kit::button::ButtonOptions {
            icon: Some(Icon::Split),
            icon_only: true,
            ..Default::default()
        },
        format!("btn-side-pane-{id}"),
        move || app.dock_session(&id),
    ))
}

/// In a page's or view's place while the side pane shows the same conversation: says so, with
/// the way to take it back out of the pane.
pub(crate) fn beside_note(app: AppState, id: Id) -> AnyElement {
    view()
        .test_id("conversation-beside")
        .flex_row()
        .items_center()
        .gap(px(12.0))
        .p(px(16.0))
        .child(text(dock::CONVERSATION_BESIDE).text_color(color(Token::Ink2)))
        .child(button(
            app,
            dock::MOVE_BACK,
            ButtonKind::Ghost,
            "btn-move-back",
            move || app.move_back(&id),
        ))
        .into_element()
}

#[cfg(test)]
#[path = "docked_tests.rs"]
mod tests;
