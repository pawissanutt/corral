//! Reading an opened job's output: what the view keeps while it shows (the search, the match it
//! stands on, whether it follows the newest lines), its keys, keeping to the end while it
//! follows, reading earlier pages until a match, and the find bar. test ids `job-search`,
//! `field-job-search`, `btn-search-next`, `btn-search-previous`, `btn-search-close`,
//! `btn-search-earlier`.

use nsh_core::element::input;
use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::session::JobOutputView;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::super::super::super::run;
use super::super::agents_detail::words;
use super::super::cell;

/// How close to the end still counts as at the end, in points.
pub(super) const END_SLACK: f32 = 24.0;
/// A search with no match stands on the newest one once there is.
pub(super) const NEWEST: usize = usize::MAX;

/// Whether a key on the page may act: no sheet, overlay or menu stands over it.
fn page_has_keys(app: AppState) -> bool {
    app.nav.with_untracked(|nav| nav.newest().is_none())
}

/// A bare key of the view: taken only while no field is edited and nothing stands over the page.
pub(super) fn view_key(app: AppState, key: &str, act: impl Fn() + 'static) {
    let options = KeyOptions {
        when: When::Auto,
        ..KeyOptions::default()
    };
    on_key_with(key, options, move |_| {
        if page_has_keys(app) {
            act();
        }
    });
}

/// What the opened output's view keeps while it shows: the search and whether it follows.
#[derive(Clone, Copy)]
pub(super) struct Reading {
    pub(super) view: Memo<Option<JobOutputView>>,
    /// The find bar is open.
    pub(super) finding: Signal<bool>,
    pub(super) query: Signal<String>,
    /// The match the search stands on; `NEWEST` until the owner walks.
    pub(super) at: Signal<usize>,
    /// Search earlier output reads page after page until a match or the start.
    pub(super) hunting: Signal<bool>,
    /// Paused since this byte of the output; none while following.
    pub(super) paused: Signal<Option<u64>>,
    /// Bumped when the current match should be brought into view.
    pub(super) reveal: Signal<u32>,
    pub(super) scroller: ElementRef,
    pub(super) hit: ElementRef,
    pub(super) field: ElementRef,
}

impl Reading {
    /// Pause following where the output stands, unless already paused.
    pub(super) fn hold(self) {
        if self.paused.get_untracked().is_none() {
            let end = self.view.with_untracked(|v| v.as_ref().map(|v| v.end));
            self.paused.set(end);
        }
    }

    fn resume(self) {
        self.paused.set(None);
        self.scroller.scroll_to_end(true);
    }

    pub(super) fn toggle(self) {
        if self.paused.get_untracked().is_some() {
            self.resume();
        } else {
            self.hold();
        }
    }

    /// Step `delta` matches from the current one, wrapping.
    fn step(self, delta: isize) {
        let total = self.view.with_untracked(|v| {
            v.as_ref()
                .and_then(|v| self.query.with_untracked(|q| v.search(q, 0)))
                .map_or(0, |found| found.total)
        });
        if total == 0 {
            return;
        }
        let now = self.at.get_untracked().min(total - 1);
        let next = (now as isize + delta).rem_euclid(total as isize) as usize;
        self.at.set(next);
        self.hold();
        self.reveal.update(|n| *n = n.wrapping_add(1));
    }

    pub(super) fn find(self) {
        self.finding.set(true);
        after_answer(move || self.field.focus());
    }

    fn leave(self) {
        self.finding.set(false);
        self.query.set(String::new());
        self.hunting.set(false);
        self.at.set(NEWEST);
    }
}

/// The output view's keys (desktop and terminal): `/` find, `f` follow or pause, and in a
/// terminal `n` and `N` walk the matches; while the find bar is open Shift+Enter walks back and
/// Esc leaves it.
pub(super) fn keys(app: AppState, reading: Reading) {
    if app.form == Form::Phone {
        return;
    }
    view_key(app, "/", move || reading.find());
    view_key(app, "f", move || reading.toggle());
    if app.form == Form::Terminal {
        view_key(app, "n", move || reading.step(1));
        view_key(app, "shift+n", move || reading.step(-1));
    }
    effect(move || {
        if !reading.finding.get() {
            return;
        }
        let always = KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        };
        on_key_with("shift+enter", always, move |_| {
            if page_has_keys(app) {
                reading.step(-1);
            }
        });
        on_key_with("escape", always, move |_| {
            if page_has_keys(app) {
                reading.leave();
            }
        });
    });
}

/// While following, each change of the output keeps the view at its newest line; a requested
/// match is brought into view.
pub(super) fn keep_to_end(reading: Reading) {
    let end = computed(move || {
        reading
            .view
            .with(|v| v.as_ref().map(|v| (v.end, v.lines.len())))
    });
    effect(move || {
        let _ = end.get();
        if reading.paused.get_untracked().is_none() {
            after_answer(move || reading.scroller.scroll_to_end(false));
        }
    });
    effect(move || {
        let _ = reading.reveal.get();
        after_answer(move || {
            reading
                .hit
                .reveal(RevealOptions::default().vertical(ScrollAlignment::Center))
        });
    });
}

/// Search earlier output: read the page before the loaded one while nothing matches and there
/// is one, then stand on the newest match found. A page asked for once is not asked for again:
/// a read that could not leave stops the search rather than repeating it.
pub(super) fn hunt(app: AppState, reading: Reading) {
    let asked = stored(None::<crate::model::session::Action>);
    effect(move || {
        if !reading.hunting.get() {
            asked.replace(None);
            return;
        }
        let query = reading.query.get();
        let state = reading.view.with(|v| {
            v.as_ref().map(|v| {
                let total = v.search(&query, 0).map_or(0, |found| found.total);
                (total, v.older.clone(), v.loading)
            })
        });
        untracked(|| match state {
            Some((total, _, _)) if total > 0 => {
                reading.hunting.set(false);
                reading.at.set(NEWEST);
                reading.reveal.update(|n| *n = n.wrapping_add(1));
            }
            Some((_, Some(earlier), false)) if asked.get() != Some(earlier.clone()) => {
                asked.replace(Some(earlier.clone()));
                run(app, earlier);
            }
            Some((_, _, true)) => {}
            _ => reading.hunting.set(false),
        });
    });
}

/// The find bar: the field, how many matches and which one shows, the walk between them, what
/// was not searched with the way to search earlier output, and the way out.
pub(super) fn find_bar(app: AppState, reading: Reading) -> AnyElement {
    let form = app.form;
    after_answer(move || reading.field.focus());
    let field = input()
        .element_ref(&reading.field)
        .test_id("field-job-search")
        .label("Find in the output")
        .placeholder("Find in the output")
        .value(move || reading.query.get())
        .on_change(move |typed: &str| {
            reading.query.set(typed.to_owned());
            reading.at.set(NEWEST);
            reading.hunting.set(false);
            if !typed.is_empty() {
                reading.hold();
                reading.reveal.update(|n| *n = n.wrapping_add(1));
            }
        })
        .on_submit(move |_: &str| reading.step(1))
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink));
    let field = match form {
        Form::Terminal => field.bg(AdaptiveColor::TRANSPARENT).into_element(),
        Form::Desktop | Form::Phone => field
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 16.0 } else { 13.0 }))
            .min_h(px(if form == Form::Phone { 44.0 } else { 30.0 }))
            .px(px(8.0))
            .rounded(px(6.0))
            .bg(color(Token::Bg))
            .border_1()
            .border_color(color(Token::Rule2))
            .focus(|s| s.border_color(color(Token::Focus)))
            .into_element(),
    };
    let found = computed(move || {
        let query = reading.query.get();
        let at = reading.at.get();
        reading.view.with(|v| {
            v.as_ref()
                .and_then(|v| v.search(&query, at))
                .map(|found| (found.count, found.note, found.earlier.is_some()))
        })
    });
    let count = cell(
        form,
        move || {
            found.with(|found| {
                found
                    .as_ref()
                    .map(|(count, ..)| count.clone())
                    .unwrap_or_default()
            })
        },
        Token::Ink2,
        true,
    )
    .flex_shrink_0();
    // What was not searched and the way to search it, built again only when those change, not
    // as the count moves.
    let gap = computed(move || {
        found.with(|found| {
            found
                .as_ref()
                .map(|(_, note, earlier)| (note.clone(), *earlier))
        })
    });
    let previous = button_with(
        app,
        "Previous",
        ButtonKind::Quiet,
        ButtonOptions {
            key_hint: Some(if form == Form::Terminal {
                "N"
            } else {
                "Shift+⏎"
            }),
            ..ButtonOptions::default()
        },
        "btn-search-previous",
        move || reading.step(-1),
    );
    let next = button_with(
        app,
        "Next",
        ButtonKind::Quiet,
        ButtonOptions {
            key_hint: Some(if form == Form::Terminal { "n" } else { "⏎" }),
            ..ButtonOptions::default()
        },
        "btn-search-next",
        move || reading.step(1),
    );
    let close = button_with(
        app,
        "Done",
        ButtonKind::Quiet,
        ButtonOptions {
            key_hint: Some("esc"),
            ..ButtonOptions::default()
        },
        "btn-search-close",
        move || reading.leave(),
    );
    let row = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 8.0 }))
        .child(field)
        .child(count)
        .child(previous)
        .child(next)
        .child(close);
    let unsearched = dynamic(move || {
        let (note, earlier) = gap.get()?;
        let hunting = reading.hunting.get();
        Some(untracked(|| {
            let said = note.map(|said| {
                words(form, said, color(Token::Ink3))
                    .flex_1()
                    .min_w(px(0.0))
            });
            let earlier = earlier.then(|| {
                button_with(
                    app,
                    if hunting {
                        "Searching earlier output…"
                    } else {
                        "Search earlier output"
                    },
                    ButtonKind::Ghost,
                    ButtonOptions {
                        disabled: hunting,
                        ..ButtonOptions::default()
                    },
                    "btn-search-earlier",
                    move || {
                        if !reading.hunting.get_untracked() {
                            reading.hunting.set(true);
                        }
                    },
                )
            });
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(if form == Form::Terminal { 16.0 } else { 8.0 }))
                .children(said)
                .children(earlier)
                .into_element()
        }))
    });
    let column = view()
        .test_id("job-search")
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(row)
        .child(unsearched);
    match form {
        Form::Terminal => view()
            .flex_none()
            .child(column.px(term::cells(1.0)))
            .child(term::hrule())
            .into_element(),
        _ => column
            .flex_none()
            .px(px(if form == Form::Phone { 12.0 } else { 18.0 }))
            .py(px(8.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .into_element(),
    }
}
