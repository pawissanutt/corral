//! The panel's Summary tab: what waits on the owner (`.callout`, its task's Blocked reason),
//! what the session does now and how long it has been quiet, and, from a corrald that serves
//! transcripts, what it said and did (`.log`), each a section (`.sec`) of the mock's
//! `agentPanel`. Every word and number is the `RosterPanel`'s; presses go through the roster's
//! selection.

use std::collections::BTreeSet;

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{Recent, RecentEntry, RecentState, RosterPanel, RosterWait};
use crate::model::hue::Hue;
use crate::model::text::span;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::notice::{loading, notice};
use crate::ui::term::{self, cells};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::run_roster;

/// A section's sides on desktop (`.sec`).
const SIDE: f32 = 18.0;

/// A section (`.sec`): its title in 12 pt semibold ink-3 with an optional note at the trailing
/// edge, then its body; 14 18 under a rule. Terminal: the title in bold, the note quiet, the
/// body, a blank row.
fn section(
    app: AppState,
    title: &'static str,
    note: Option<AnyElement>,
    body: AnyElement,
) -> AnyElement {
    let words = text(title)
        .role(AccessibilityRole::Header)
        .number_of_lines(1u32)
        .flex_shrink_0();
    if app.form == Form::Terminal {
        return view()
            .px(cells(1.0))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .child(
                        words
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::Ink)),
                    )
                    .child(view().flex_1().min_w(px(0.0)))
                    .children(note),
            )
            .child(body)
            .child(term::blank())
            .into_element();
    }
    view()
        .px(px(SIDE))
        .py(px(14.0))
        .border_b_1()
        .border_b_color(color(Token::Rule))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .mb(px(8.0))
                .child(
                    words
                        .font_family(FONT_TEXT)
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(Token::Ink3)),
                )
                .child(view().flex_1().min_w(px(0.0)))
                .children(note),
        )
        .child(body)
        .into_element()
}

/// A section's note or a quiet line: 11.5 pt ink-3 (desktop), quiet in a terminal.
fn quiet(app: AppState, words: impl IntoReactive<SharedString>, size: f32) -> AnyElement {
    let t = text(words).flex_shrink(1.0).min_w(px(0.0));
    if app.form == Form::Terminal {
        return t.text_color(color(Token::Ink3)).into_element();
    }
    t.font_family(FONT_TEXT)
        .text_size(px(size))
        .line_height(px((size * 1.45).round()))
        .text_color(color(Token::Ink3))
        .into_element()
}

/// The Summary, scrolling under the head: what waits on the owner, what the session does now
/// and what it said and did. Each part follows its own facts, so the times ticking redraw no
/// button.
pub(super) fn summary(app: AppState, current: Memo<RosterPanel>) -> AnyElement {
    let waits = computed(move || current.with(|p| p.waits.clone()));
    let recent = computed(move || current.with(|p| p.recent.clone()));
    let open = signal(BTreeSet::<String>::new());
    let content = view()
        .child(dynamic(move || {
            waits.with(|waits| {
                waits
                    .as_ref()
                    .map(|wait| untracked(|| waits_view(app, wait)))
            })
        }))
        .child(doing(app, current))
        .child(dynamic(move || {
            recent.with(|recent| {
                recent
                    .as_ref()
                    .map(|recent| untracked(|| recent_view(app, recent, open)))
            })
        }));
    scroll()
        .test_id("agent-panel-summary")
        .flex_1()
        .min_h(px(0.0))
        .child(content)
        .into_element()
}

/// What waits on the owner (`.callout`), in the needs-you fill with its button.
fn waits_view(app: AppState, wait: &RosterWait) -> AnyElement {
    let action = wait.action.clone();
    let press = button(
        app,
        wait.label,
        ButtonKind::Primary,
        "btn-agent-waits",
        move || run_roster(app, action.clone()),
    );
    let words = vec![
        span(wait.heading).bold().hue(Hue::You),
        span(" "),
        span(wait.text.clone()),
    ];
    let callout = notice(app, Hue::You, &words, Some(press));
    if app.form == Form::Terminal {
        return view()
            .px(cells(1.0))
            .child(callout)
            .child(term::blank())
            .into_element();
    }
    view()
        .px(px(SIDE))
        .py(px(14.0))
        .border_b_1()
        .border_b_color(color(Token::Rule))
        .child(callout)
        .into_element()
}

/// Doing now, with how long the session has been quiet at the trailing edge.
fn doing(app: AppState, current: Memo<RosterPanel>) -> AnyElement {
    let title = untracked(|| current.with(|p| p.doing_title));
    let words = computed(move || current.with(|p| p.doing.clone()));
    let quiet_for = computed(move || current.with(|p| p.quiet.clone()));
    let sentence = text(move || words.get()).text_color(color(Token::Ink));
    let sentence = if app.form == Form::Terminal {
        sentence
    } else {
        sentence
            .font_family(FONT_TEXT)
            .text_size(px(13.0))
            .line_height(px(19.0))
    };
    section(
        app,
        title,
        Some(quiet(app, move || quiet_for.get(), 11.5)),
        sentence.into_element(),
    )
}

/// What it said and did (`.log`), newest first: its words, or the loading, unavailable or empty
/// state in the model's words, under the stale note while the page shown is an older one.
fn recent_view(app: AppState, recent: &Recent, open: Signal<BTreeSet<String>>) -> AnyElement {
    let body = match &recent.state {
        RecentState::Loading(caption) => {
            loading(app, *caption, &[(0.9, 10.0), (0.6, 10.0), (0.75, 10.0)])
        }
        RecentState::Unavailable(words) => quiet(app, words.clone(), 12.5),
        RecentState::Ready if recent.entries.is_empty() => quiet(app, recent.empty, 12.5),
        RecentState::Ready => view()
            .children(
                recent
                    .entries
                    .iter()
                    .enumerate()
                    .map(|(at, entry)| entry_view(app, at, entry, open))
                    .collect::<Vec<_>>(),
            )
            .into_element(),
    };
    let stale = recent.stale.clone().map(|words| {
        let t = text(words).text_color(color(Token::Held));
        if app.form == Form::Terminal {
            t.into_element()
        } else {
            t.mb(px(6.0))
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .into_element()
        }
    });
    section(
        app,
        recent.title,
        Some(quiet(app, recent.order, 11.5)),
        view().children(stale).child(body).into_element(),
    )
}

/// One entry: its time, then the words, or a tool line folded to its summary that opens onto
/// the whole call. Desktop: 40 pt of time in mono, 8 pt above and below, a rule between entries.
/// Terminal: one row each, a folded line marked `›` (`⌄` open).
fn entry_view(
    app: AppState,
    at: usize,
    entry: &RecentEntry,
    open: Signal<BTreeSet<String>>,
) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let time = text(entry.at.clone())
        .number_of_lines(1u32)
        .flex_shrink_0()
        .text_color(color(Token::Ink3));
    let time = if terminal {
        time.w(cells(7.0))
    } else {
        time.w(px(40.0))
            .pt(px(1.0))
            .font_family(FONT_MONO)
            .text_size(px(12.0))
    };
    let said = if entry.tool {
        tool_line(app, at, entry, open)
    } else {
        let words = text(entry.text.clone()).text_color(color(Token::Ink));
        if terminal {
            words.number_of_lines(2u32).into_element()
        } else {
            words
                .font_family(FONT_TEXT)
                .text_size(px(13.0))
                .line_height(px(19.0))
                .into_element()
        }
    };
    view()
        .flex_row()
        .items_start()
        .when(!terminal, |row| {
            row.gap(px(10.0)).py(px(8.0)).when(at > 0, |row| {
                row.border_t_1().border_t_color(color(Token::Rule))
            })
        })
        .child(time)
        .child(view().flex_1().min_w(px(0.0)).child(said))
        .into_element()
}

/// A tool entry's folded line: a chevron and the summary in quiet 12 pt; pressing it opens the
/// whole call (the entry's hint) on the sunken ground in mono, and folds it again.
fn tool_line(
    app: AppState,
    at: usize,
    entry: &RecentEntry,
    open: Signal<BTreeSet<String>>,
) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let key = format!("{at}\u{1f}{}\u{1f}{}", entry.at, entry.text);
    let shown = {
        let key = key.clone();
        computed(move || open.with(|keys| keys.contains(&key)))
    };
    let summary = text(entry.text.clone())
        .number_of_lines(1u32)
        .truncate()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(color(Token::Ink3));
    let fold = if terminal {
        text(move || if shown.get() { "⌄" } else { "›" })
            .flex_shrink_0()
            .text_color(color(Token::Ink3))
            .into_element()
    } else {
        view()
            .flex_shrink_0()
            .rotate(move || if shown.get() { deg(90.0) } else { deg(0.0) })
            .child(icon(Icon::Chevron, 12.0, color(Token::Ink3)))
            .into_element()
    };
    let line = view()
        .flex_row()
        .items_center()
        .self_start()
        .gap(px(6.0))
        .max_w(relative(1.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(entry.text.clone())
        .accessibility_state(move || AccessibilityState {
            expanded: Some(shown.get()),
            ..AccessibilityState::default()
        })
        .test_id(&format!("btn-tool-{at}"))
        .on_press(move || {
            let key = key.clone();
            open.update(move |keys| {
                if !keys.remove(&key) {
                    keys.insert(key);
                }
            });
        })
        .child(fold)
        .child(if terminal {
            summary.into_element()
        } else {
            summary
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .into_element()
        })
        .when(!terminal, |line| {
            line.rounded(px(4.0))
                .hover(|s| s.bg(color(Token::Hover)))
                .focus_visible(|s| {
                    s.outline(px(2.0))
                        .outline_color(color(Token::Focus))
                        .outline_offset(px(1.0))
                })
        });
    let whole = entry.hint.clone();
    let call = dynamic(move || {
        shown.get().then(|| {
            let words = text(whole.clone()).text_color(color(Token::Ink2));
            if terminal {
                view().pl(cells(2.0)).child(words).into_element()
            } else {
                view()
                    .mt(px(6.0))
                    .px(px(12.0))
                    .py(px(10.0))
                    .rounded(px(6.0))
                    .bg(color(Token::BgSunk))
                    .border_1()
                    .border_color(color(Token::Rule))
                    .child(
                        words
                            .font_family(FONT_MONO)
                            .text_size(px(12.0))
                            .line_height(px(19.0)),
                    )
                    .into_element()
            }
        })
    });
    view().child(line).child(call).into_element()
}
