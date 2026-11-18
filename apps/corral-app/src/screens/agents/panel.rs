//! The selected session's panel (`.apanel`, the mock's `agentPanel`), from `RosterPanel`: the
//! head (state, role, id, project, close; the name; profile, model, harness, pane and since),
//! the Summary | Terminal tabs with Open session and the run's Session menu, how its latest pane
//! request ended, then the Summary (what waits on the owner, what the session does now and,
//! from a corrald that serves transcripts, what it said and did) or the pane's terminal, and the
//! reply box pinned under them. Every press goes through the roster's selection
//! (`run_roster`), never the full session route.
//!
//! The panel keeps what it shows loaded while it stays open (`RosterPanel::wants`), as the
//! session page does: the transcript read once and again when the run's row changes, the pane's
//! screen streamed (or revalidated once a tick where corrald does not stream) while the Terminal
//! tab shows.

use corral_protocol::detail::{DetailRef, STREAM_TERMINAL_V1};
use nsh_core::prelude::*;

use crate::model::agents::{Action, RosterPanel, RosterPanelTab};
use crate::model::daemon::Link;
use crate::model::hue::Hue;
use crate::model::{Form, Id};
use crate::screens::assistant;
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term::{self, cells};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::console::{self, BIG};
use super::panel_summary::summary;
use super::session;
use super::surface;
use super::{controls, reply, run_roster, segs};

/// The panel's width beside the roster (desktop points; the mock's 448 less its rule and
/// paddings' rounding) and over it (`min(460px, 100%)`).
const WIDTH: f32 = 420.0;
const OVER: f32 = 460.0;
/// The panel's width beside the roster in a terminal, in Cells.
pub(super) const TERM_PANEL: f32 = 46.0;
/// A section's sides on desktop (`.sec`, `.phd`).
const SIDE: f32 = 18.0;

/// Where the panel sits: beside the roster, over its right side, or over all of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Place {
    Beside,
    Over,
    Full,
}

/// The panel of the selected session in its place, or nothing while none is selected. It is
/// built anew for each session, so a draft, an open tool line or a stream belongs to one.
pub(super) fn panel(
    app: AppState,
    panel: Memo<Option<RosterPanel>>,
    place: Memo<Place>,
) -> AnyElement {
    let session = computed(move || panel.with(|p| p.as_ref().map(|p| p.session.clone())));
    dynamic(move || session.get().map(|_| untracked(|| body(app, panel, place))))
}

fn body(app: AppState, panel: Memo<Option<RosterPanel>>, place: Memo<Place>) -> AnyElement {
    let Some(first) = panel.get_untracked() else {
        return view().into_element();
    };
    let id = first.session.clone();
    let name = first.name.clone();
    // The panel closes with its session; until then the last panel stands.
    let current = computed(move || panel.get().unwrap_or_else(|| first.clone()));
    keep_loaded(app, current);
    chat(app, current);
    let delivered = computed(move || current.with(|p| p.reply.delivered.clone()));
    let draft = session::draft(app, delivered, id, run_roster);
    let tab = computed(move || current.with(|p| p.tab));
    let content = dynamic(move || match tab.get() {
        RosterPanelTab::Summary => untracked(|| summary(app, current)),
        RosterPanelTab::Terminal => untracked(|| terminal(app, current)),
    });
    let words = reply::words(
        app,
        computed(move || current.with(|p| p.reply.clone())),
        computed(move || current.with(|p| p.chat.clone())),
        draft,
        run_roster,
    );
    let inner = view()
        .test_id("agent-panel")
        .role(AccessibilityRole::Group)
        .label(name)
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(head(app, current, place))
        .child(pane_outcome(app, current))
        .child(content)
        .child(words);
    let at = move |wanted: Place| move || place.get() == wanted;
    match app.form {
        Form::Terminal => view()
            .flex_row()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .style_when(at(Place::Beside), |s| {
                s.w(cells(TERM_PANEL)).flex_shrink_0()
            })
            .style_when(move || place.get() != Place::Beside, |s| s.flex_1())
            .child(
                view()
                    .flex_row()
                    .self_stretch()
                    .style_when(move || place.get() != Place::Beside, |s| s.hidden())
                    .child(term::vrule()),
            )
            .child(inner)
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .flex_col()
            .min_h(px(0.0))
            .bg(color(Token::Bg))
            .style_when(at(Place::Beside), |s| {
                s.w(px(WIDTH))
                    .flex_shrink_0()
                    .border_l(px(1.0))
                    .border_l_color(color(Token::Rule))
            })
            .style_when(at(Place::Over), |s| {
                s.absolute()
                    .top(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .w(px(OVER))
                    .max_w(relative(1.0))
                    .border_l(px(1.0))
                    .border_l_color(color(Token::Rule2))
                    .shadow_lg()
            })
            .style_when(at(Place::Full), |s| {
                s.absolute()
                    .top(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .left(px(0.0))
            })
            .child(inner)
            .into_element(),
    }
}

/// While the panel is open, subscribes the assistant thread its composer chats in.
fn chat(app: AppState, current: Memo<RosterPanel>) {
    let scoped = untracked(|| app.scope());
    let scope = computed(move || current.with(|p| p.chat.clone()));
    effect(move || {
        if let Some(scope) = scope.get() {
            untracked(|| assistant::open_scope(scoped, scope, false));
        }
    });
}

/// Keeps the panel's details current, through the source it mounted on. A transcript or a
/// question is read when first wanted (a failed or stale one is read again then); the transcript
/// is revalidated when the session's row changes, unless the owner reads an earlier page on the
/// session page. The pane's screen, while the Terminal tab wants it, is held open as a stream;
/// where corrald does not stream it, it is revalidated once a tick of the feed's clock while
/// the link is up, a failed read not again until the row changes or the link comes back.
fn keep_loaded(app: AppState, current: Memo<RosterPanel>) {
    let scoped = untracked(|| app.scope());
    let id = untracked(|| current.with(|p| p.session.clone()));
    let wants = computed(move || current.with(|p| p.wants.clone()));
    effect(move || {
        let wants = wants.get();
        untracked(|| {
            for target in wants {
                if !matches!(target, DetailRef::Terminal { .. }) {
                    scoped.load(target);
                }
            }
        });
    });
    let row = computed({
        let id = id.clone();
        move || app.feed.with(|feed| feed.agents.get(id.as_str()).cloned())
    });
    let seen = stored(None::<Option<corral_protocol::rows::AgentRow>>);
    {
        let id = id.clone();
        effect(move || {
            let now = row.get();
            let earlier = app
                .agents
                .with_untracked(|state| state.earlier.contains(id.as_str()));
            let target = DetailRef::Session { id: id.to_string() };
            // Only what the panel wants: a run's transcript only from a corrald serving them.
            let wanted = current.with_untracked(|p| p.wants.contains(&target));
            if let Some(before) = seen.replace(Some(now.clone()))
                && before != now
                && !earlier
                && wanted
            {
                untracked(|| scoped.refresh(target));
            }
        });
    }
    let pane = computed(move || {
        current.with(|p| {
            p.wants.iter().find_map(|target| match target {
                DetailRef::Terminal { id } => Some(id.clone()),
                _ => None,
            })
        })
    });
    let open = stored(None::<String>);
    effect(move || {
        let now = pane.get();
        let before = open.replace(now.clone());
        if before != now {
            untracked(|| {
                if let Some(pane) = before {
                    scoped.terminal(pane, false);
                }
                if let Some(pane) = now {
                    scoped.terminal(pane, true);
                }
            });
        }
    });
    // The owner may already be letting go of its values when this runs.
    cleanup(move || {
        if let Some(pane) = open.try_with(std::mem::take).flatten() {
            scoped.terminal(pane, false);
        }
    });
    let streamed = computed(move || app.feed.with(|feed| feed.capable(STREAM_TERMINAL_V1)));
    // The client driver advances `now` once a second; read through this memo, the revalidation
    // runs once a tick rather than on every feed write, and not at all while the link is down.
    let tick = computed(move || {
        app.feed
            .with(|feed| (feed.link == Link::Up).then_some(feed.now))
    });
    let tried = stored(None::<Option<corral_protocol::rows::AgentRow>>);
    effect(move || {
        let Some(pane) = pane.get().filter(|_| !streamed.get()) else {
            tried.replace(None);
            return;
        };
        if tick.get().is_none() {
            tried.replace(None);
            return;
        }
        untracked(|| {
            let now = row.get();
            let changed = tried.replace(Some(now.clone())) != Some(now);
            let target = DetailRef::Terminal { id: pane };
            let failed = app.feed.with(|feed| {
                feed.detail(&target)
                    .is_some_and(|cache| cache.error.is_some())
            });
            if changed || !failed {
                scoped.refresh(target);
            }
        });
    });
}

/// The head's facts, redrawn when they change.
#[derive(Clone, PartialEq)]
struct Head {
    chip: String,
    hue: Hue,
    role: String,
    shown_id: String,
    project: String,
    name: String,
    facts: String,
    close_label: String,
    close_short: &'static str,
    close: Action,
}

/// The head's controls: the tabs, Open session and the Session menu. They are drawn apart from
/// the facts, so a run's state changing under an open Session menu (Pause turns its chip to
/// idle) leaves the menu and its trigger mounted, and the keyboard focus the closing menu puts
/// back on the trigger lands on a live view.
#[derive(Clone, PartialEq)]
struct HeadControls {
    tabs: Vec<(&'static str, &'static str, Action)>,
    open_label: &'static str,
    open: Action,
    menu: Option<Id>,
}

/// The head (`.phd`): the state chip, role, id and project with close at the trailing edge (a
/// back arrow while the panel covers the whole roster); the name; the facts line in mono; the
/// tabs and Open session. Desktop 14 18 12 under a rule; terminal Cells over a joined rule.
fn head(app: AppState, current: Memo<RosterPanel>, place: Memo<Place>) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let shown = computed(move || {
        current.with(|p| Head {
            chip: p.chip.clone(),
            hue: p.hue,
            role: p.role.clone(),
            shown_id: p.shown_id.clone(),
            project: p.project.clone(),
            name: p.name.clone(),
            facts: p.facts.clone(),
            close_label: p.close_label.clone(),
            close_short: p.close_short,
            close: p.close.clone(),
        })
    });
    let controls = computed(move || {
        current.with(|p| HeadControls {
            tabs: p
                .tabs
                .iter()
                .map(|tab| (tab.id, tab.label, tab.action.clone()))
                .collect(),
            open_label: p.open_label,
            open: p.open.clone(),
            menu: p.menu.clone(),
        })
    });
    let chosen = computed(move || {
        current.with(|p| p.tabs.iter().find(|tab| tab.selected).map(|tab| tab.id))
    });
    let block = view()
        .flex_none()
        .child(dynamic(move || {
            shown.with(|head| untracked(|| head_facts(app, head, place)))
        }))
        .child(dynamic(move || {
            controls.with(|controls| untracked(|| head_controls(app, controls, chosen)))
        }));
    if terminal {
        return view()
            .flex_none()
            .child(block.px(cells(1.0)))
            .child(term::hrule())
            .into_element();
    }
    block
        .px(px(SIDE))
        .pt(px(14.0))
        .pb(px(12.0))
        .border_b_1()
        .border_b_color(color(Token::Rule))
        .into_element()
}

fn head_facts(app: AppState, head: &Head, place: Memo<Place>) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let meta = |words: String, mono: bool| {
        let t = text(words)
            .number_of_lines(1u32)
            .truncate()
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .text_color(color(Token::Ink3));
        if terminal {
            t.into_element()
        } else {
            t.font_family(if mono { FONT_MONO } else { FONT_TEXT })
                .text_size(px(12.0))
                .into_element()
        }
    };
    let dismiss = close(app, head, place);
    let first = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .when(!terminal, |row| row.min_h(px(24.0)))
        .child(chip(app, head.chip.clone(), head.hue))
        .child(meta(head.role.clone(), false))
        .child(meta(head.shown_id.clone(), true))
        .child(meta(head.project.clone(), false))
        .child(view().flex_1().min_w(px(0.0)))
        .child(dismiss);
    let name = text(head.name.clone())
        .role(AccessibilityRole::Header)
        .number_of_lines(2u32)
        .text_color(color(Token::Ink))
        .font_weight(FontWeight::BOLD);
    let name = if terminal {
        name
    } else {
        name.mt(px(4.0))
            .mb(px(3.0))
            .font_family(FONT_TEXT)
            .text_size(px(16.0))
            .line_height(px(21.0))
    };
    let facts = (!head.facts.is_empty()).then(|| meta(head.facts.clone(), true));
    view()
        .child(first)
        .child(name)
        .children(facts)
        .into_element()
}

fn head_controls(
    app: AppState,
    head: &HeadControls,
    chosen: Memo<Option<&'static str>>,
) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let options = head
        .tabs
        .iter()
        .map(|(id, label, action)| {
            let mine = *id;
            let on = computed(move || chosen.get() == Some(mine));
            let action = action.clone();
            segs::option(app, id, (*label).to_owned(), None, on, move || {
                run_roster(app, action.clone())
            })
        })
        .collect();
    let open = head.open.clone();
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .when(!terminal, |row| row.mt(px(10.0)))
        .child(segs::track(app, options))
        .child(button(
            app,
            head.open_label,
            ButtonKind::Quiet,
            "btn-open-session",
            move || run_roster(app, open.clone()),
        ))
        .children(head.menu.clone().map(|id| controls::session_menu(app, id)))
        .into_element()
}

/// Close, named for the session: a cross, or a back arrow while the panel covers the roster.
/// The terminal draws no icons, so its Close is words, short ones: the row names the session.
fn close(app: AppState, head: &Head, place: Memo<Place>) -> AnyElement {
    let label = if app.form == Form::Terminal {
        head.close_short.to_owned()
    } else {
        head.close_label.clone()
    };
    let action = head.close.clone();
    dynamic(move || {
        let glyph = if place.get() == Place::Full {
            Icon::Back
        } else {
            Icon::Close
        };
        let action = action.clone();
        untracked(|| {
            button_with(
                app,
                label.clone(),
                ButtonKind::Quiet,
                ButtonOptions {
                    icon: Some(glyph),
                    icon_only: true,
                    ..ButtonOptions::default()
                },
                "btn-close-panel",
                move || run_roster(app, action.clone()),
            )
        })
    })
}

/// How the run's latest pane request (keys, Focus pane) ended, in its tone, under the head;
/// nothing before the first (`pane-status`).
fn pane_outcome(app: AppState, current: Memo<RosterPanel>) -> AnyElement {
    let status = computed(move || current.with(|p| p.pane.clone()));
    dynamic(move || {
        status.with(|status| {
            status.as_ref().map(|(hue, words)| {
                let ink = match hue {
                    Hue::Neutral => color(Token::Ink3),
                    hue => hue_ink(*hue),
                };
                let line = text(words.clone()).test_id("pane-status").text_color(ink);
                if app.form == Form::Terminal {
                    return view().flex_none().px(cells(1.0)).child(line).into_element();
                }
                view()
                    .flex_none()
                    .px(px(SIDE))
                    .py(px(8.0))
                    .border_b_1()
                    .border_b_color(color(Token::Rule))
                    .child(line.font_family(FONT_TEXT).text_size(px(12.0)))
                    .into_element()
            })
        })
    })
}

/// The Terminal tab: the pane's toolbar over its screen, as the session page draws them.
fn terminal(app: AppState, current: Memo<RosterPanel>) -> AnyElement {
    let rows = computed(move || current.with(|p| p.terminal.as_ref().map(surface::Rows::of)));
    let bar = computed(move || current.with(|p| p.terminal.as_ref().map(surface::Termbar::of)));
    let top = dynamic(move || {
        bar.with(|bar| {
            bar.as_ref()
                .map(|bar| untracked(|| surface::toolbar(app, bar, run_roster)))
        })
    });
    if app.form == Form::Terminal {
        return view()
            .flex_1()
            .min_h(px(0.0))
            .child(top)
            .child(term::hrule())
            .child(surface::surface_scroll(rows).child(dynamic(move || {
                rows.with(|rows| surface::rows_view(app, rows.as_ref(), BIG, (0.0, 0.0)))
            })))
            .into_element();
    }
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(top)
        .child(
            surface::surface_scroll(rows)
                .bg(console::ground())
                .child(dynamic(move || {
                    rows.with(|rows| surface::rows_view(app, rows.as_ref(), BIG, (14.0, 12.0)))
                })),
        )
        .into_element()
}
