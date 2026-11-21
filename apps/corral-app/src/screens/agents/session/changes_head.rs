//! A one-row Changes toolbar, with detailed comparison and filter controls under More.

use nsh_core::EllipsizeMode;
use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{
    Action, AuthorChip, ChangesAction, ChangesTab, ComparisonChip, ComparisonLine, FILTER_FIELD,
    ReadState, ScopeChip,
};
use crate::model::text::span;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::hint::hint_on;
use crate::ui::kit::notice::{error as kit_error, notice};
use crate::ui::kit::text::{dim, mono};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

use super::super::super::run;
use super::super::hue_words;

const TARGET: f32 = 44.0;
const RING: f32 = 2.0;

/// The head where it folds (a terminal, a phone), so the file list keeps its rows on a small
/// screen: one line with the summary, the comparison, what narrows the list and when it was
/// read, and `⋯ More`, which unfolds the rest as the list's first rows; then what keeps the
/// list from showing (the host cannot serve it, the read failed, the repository cannot be
/// read), always.
pub(super) fn folded(app: AppState, shown: Memo<Option<ChangesTab>>) -> AnyElement {
    let form = app.form;
    let line = computed(move || {
        shown.with(|tab| {
            tab.as_ref().map(|tab| {
                let mut words: Vec<_> = tab
                    .scopes
                    .iter()
                    .filter(|chip| chip.selected)
                    .map(|chip| chip.label.clone())
                    .collect();
                words.push(tab.part.summary.clone());
                words.extend(
                    tab.comparisons
                        .iter()
                        .find(|chip| chip.selected)
                        .map(|chip| chip.label.clone()),
                );
                words.extend(tab.narrowed.clone());
                if !matches!(tab.part.read, ReadState::Read { .. }) && tab.part.read.has_rows() {
                    words.push(tab.part.read.words());
                }
                (words.join(" · "), tab.part.hue)
            })
        })
    });
    let words = text(move || {
        line.with(|line| line.as_ref().map(|(words, _)| words.clone()))
            .unwrap_or_default()
    })
    .test_id("part-changes-summary")
    .number_of_lines(1u32)
    .truncate()
    .min_w(px(0.0))
    .flex_1()
    .text_color(move || {
        hue_words(
            form,
            line.with(|line| line.as_ref().map_or(Hue::Neutral, |(_, hue)| *hue)),
        )
    });
    let words = match form {
        Form::Terminal => words.font_weight(FontWeight::BOLD),
        _ => words
            .font_family(FONT_TEXT)
            .text_size(px(14.0))
            .font_weight(FontWeight::SEMIBOLD),
    };
    let more = computed(move || shown.with(|tab| tab.as_ref().is_some_and(|tab| tab.more)));
    let toggle = dynamic(move || {
        let open = more.get();
        untracked(|| {
            button(
                app,
                if open { "⋯ Less" } else { "⋯ More" },
                ButtonKind::Quiet,
                "btn-changes-more",
                move || {
                    let session =
                        shown.with_untracked(|tab| tab.as_ref().map(|tab| tab.session.clone()));
                    if let Some(session) = session {
                        run(app, Action::Changes(ChangesAction::More { session }));
                    }
                },
            )
        })
    });
    let blocked = computed(move || {
        shown.with(|tab| {
            tab.as_ref().map(|tab| {
                (
                    tab.part.unavailable.clone(),
                    tab.part.read.clone(),
                    tab.part.reread.clone(),
                    tab.error.clone(),
                )
            })
        })
    });
    let blocked = dynamic(move || {
        let (unavailable, read, reread, error) = blocked.get()?;
        Some(untracked(|| {
            let read = match &read {
                ReadState::Failed(_) => Some(kit_error(
                    app,
                    &vec![span(read.words())],
                    reread.map(|action| {
                        button(
                            app,
                            "Retry",
                            ButtonKind::Quiet,
                            "btn-reread-changes",
                            move || run(app, action.clone()),
                        )
                    }),
                )),
                ReadState::DoesNotApply(why) => Some(dim(app, why.clone()).into_element()),
                ReadState::NotRead | ReadState::Reading if unavailable.is_none() => {
                    Some(dim(app, read.words()).into_element())
                }
                _ => None,
            };
            view()
                .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                .children(unavailable.map(|why| {
                    notice(
                        app,
                        Hue::Held,
                        &vec![span(format!("unavailable: {why}"))],
                        None,
                    )
                }))
                .children(read)
                .children(error.map(|error| notice(app, Hue::Held, &vec![span(error)], None)))
                .into_element()
        }))
    });
    view()
        .test_id("part-changes")
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .min_w(px(0.0))
                .child(words)
                .child(toggle),
        )
        .child(blocked)
        .into_element()
}

/// Comparison, repository, read and filter controls shown on demand.
pub(super) fn extras(app: AppState, shown: Memo<Option<ChangesTab>>) -> AnyElement {
    let form = app.form;
    let switch = computed(move || {
        shown.with(|tab| {
            tab.as_ref()
                .map(|tab| tab.comparisons.clone())
                .unwrap_or_default()
        })
    });
    let switch = dynamic(move || {
        let chips = switch.get();
        untracked(|| comparisons(app, &chips))
    });
    let line =
        computed(move || shown.with(|tab| tab.as_ref().and_then(|tab| tab.comparison.clone())));
    let line = dynamic(move || line.get().map(|line| untracked(|| identity(app, &line))));
    let scopes = computed(move || {
        shown.with(|tab| {
            tab.as_ref()
                .map(|tab| tab.scopes.clone())
                .unwrap_or_default()
        })
    });
    let scopes = dynamic(move || {
        let chips = scopes.get();
        (!chips.is_empty()).then(|| untracked(|| repositories(app, &chips)))
    });
    let reread = computed(move || {
        shown.with(|tab| {
            tab.as_ref()
                .filter(|tab| tab.part.read.has_rows())
                .and_then(|tab| tab.part.reread.clone())
        })
    });
    let reread = dynamic(move || {
        reread.get().map(|action| {
            untracked(|| {
                button(
                    app,
                    "Reread",
                    ButtonKind::Quiet,
                    "btn-reread-changes",
                    move || run(app, action.clone()),
                )
            })
        })
    });
    let authors = computed(move || {
        shown.with(|tab| {
            tab.as_ref()
                .map(|tab| tab.authors.clone())
                .unwrap_or_default()
        })
    });
    let authors = dynamic(move || {
        let chips = authors.get();
        (!chips.is_empty()).then(|| untracked(|| author_chips(app, &chips)))
    });
    let tools = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 10.0 }))
        .child(filter(app, shown))
        .child(authors)
        .child(reread);
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .child(switch)
        .child(line)
        .child(scopes)
        .child(tools)
        .into_element()
}

/// A pressed-or-not chip with words that can change: a comparison, a repository, an author.
/// Without an action it is muted and does nothing; `hint` says why.
fn chip(
    app: AppState,
    test_id: &str,
    words: String,
    count: Option<usize>,
    selected: bool,
    hint: Option<String>,
    action: Option<Action>,
) -> AnyElement {
    let form = app.form;
    let live = action.is_some();
    let ink = match (live, selected) {
        (false, _) => Token::Ink4,
        (true, true) => Token::Ink,
        (true, false) => Token::Ink2,
    };
    let label = text(words.clone())
        .number_of_lines(1u32)
        .text_color(color(ink))
        .font_weight(if selected {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        });
    let label = match form {
        Form::Terminal => label,
        _ => label
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 13.5 } else { 12.5 })),
    };
    let number = count.map(|n| {
        let n = text(n.to_string()).text_color(color(Token::Ink3));
        match form {
            Form::Terminal => n,
            _ => n.font_family(FONT_MONO).tabular_nums().text_size(px(11.0)),
        }
    });
    let said = match &hint {
        Some(why) => format!("{words} · {why}"),
        None => words,
    };
    let face = view()
        .test_id(test_id)
        .role(AccessibilityRole::Button)
        .label(said)
        .accessibility_state(AccessibilityState {
            selected,
            disabled: !live,
            ..AccessibilityState::default()
        })
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(if form == Form::Terminal { 8.0 } else { 5.0 }))
        .when(live, |face| face.cursor_pointer())
        .on_press(move || {
            if let Some(action) = action.clone() {
                run(app, action);
            }
        })
        .child(label)
        .children(number);
    let face = match form {
        Form::Terminal => face
            .px(term::cells(1.0))
            .when(selected, |face| face.bg(color(Token::Sel))),
        Form::Desktop | Form::Phone => face
            .h(px(if form == Form::Phone { 34.0 } else { 26.0 }))
            .px(px(10.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(color(if selected { Token::Rule2 } else { Token::Rule }))
            .bg(color(if selected { Token::Surface } else { Token::Bg }))
            .when(live && !selected, |face| {
                face.hover(|s| s.bg(color(Token::Hover)))
            })
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(1.0))
            }),
    };
    match hint {
        Some(hint) => hint_on(app, face, hint).into_element(),
        None => face.into_element(),
    }
}

fn chip_row(app: AppState, chips: Vec<AnyElement>) -> AnyElement {
    let form = app.form;
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(match form {
            Form::Terminal => 8.0,
            Form::Phone => 6.0,
            Form::Desktop => 4.0,
        }))
        .children(chips)
        .into_element()
}

/// The comparisons the host offers, the shown one pressed; then, for each it cannot serve now,
/// its name and the host's reason.
fn comparisons(app: AppState, chips: &[ComparisonChip]) -> AnyElement {
    let form = app.form;
    let row = chip_row(
        app,
        chips
            .iter()
            .map(|one| {
                chip(
                    app,
                    &one.id,
                    one.label.clone(),
                    None,
                    one.selected,
                    one.unavailable
                        .as_ref()
                        .map(|why| format!("unavailable: {why}")),
                    one.action.clone(),
                )
            })
            .collect(),
    );
    let reasons: Vec<AnyElement> = chips
        .iter()
        .filter_map(|one| {
            let why = one.unavailable.as_ref()?;
            Some(
                dim(app, format!("{}: unavailable · {why}", one.label))
                    .number_of_lines(2u32)
                    .into_element(),
            )
        })
        .collect();
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(row)
        .children(reasons)
        .into_element()
}

/// `aece0a19 → 7c41d0e2 · /work/repo · 14 files · +120 −30`.
fn identity(app: AppState, line: &ComparisonLine) -> AnyElement {
    let form = app.form;
    let mut parts = Vec::with_capacity(3);
    if let Some(range) = &line.range {
        parts.push(
            mono(app, range.clone())
                .text_color(color(Token::Ink2))
                .into_element(),
        );
    }
    parts.push(
        mono(app, line.root.clone())
            .text_color(color(Token::Ink3))
            .number_of_lines(1u32)
            .text_overflow(EllipsizeMode::Middle)
            .min_w(px(0.0))
            .flex_shrink(1.0)
            .into_element(),
    );
    parts.push(
        dim(app, line.totals.clone())
            .number_of_lines(1u32)
            .into_element(),
    );
    view()
        .flex_row()
        .items_center()
        .min_w(px(0.0))
        .gap(px(if form == Form::Terminal { 16.0 } else { 12.0 }))
        .children(parts)
        .into_element()
}

/// The repositories the host can read here, the read one pressed.
fn repositories(app: AppState, chips: &[ScopeChip]) -> AnyElement {
    let mut row = vec![dim(app, "Repository").into_element()];
    row.extend(chips.iter().map(|one| {
        chip(
            app,
            &one.id,
            one.label.clone(),
            None,
            one.selected,
            None,
            Some(one.action.clone()),
        )
    }));
    chip_row(app, row)
}

fn author_chips(app: AppState, chips: &[AuthorChip]) -> AnyElement {
    chip_row(
        app,
        chips
            .iter()
            .map(|one| {
                chip(
                    app,
                    &one.id,
                    one.label.clone(),
                    Some(one.count),
                    one.selected,
                    Some(one.hint.clone()),
                    Some(one.action.clone()),
                )
            })
            .collect(),
    )
}

/// The path filter; F3 brings the keyboard to it outside the web.
fn filter(app: AppState, shown: Memo<Option<ChangesTab>>) -> AnyElement {
    let form = app.form;
    let handle = element_ref();
    app.focus_when_asked(FILTER_FIELD, handle);
    let entry = input()
        .element_ref(&handle)
        .test_id(FILTER_FIELD)
        .label("Filter changed paths")
        .placeholder("Filter paths")
        .value(move || {
            shown.with(|tab| {
                tab.as_ref()
                    .map(|tab| tab.filter.clone())
                    .unwrap_or_default()
            })
        })
        .on_change(move |typed: &str| {
            let session = shown.with_untracked(|tab| tab.as_ref().map(|tab| tab.session.clone()));
            if let Some(session) = session {
                run(
                    app,
                    Action::Changes(ChangesAction::Filter {
                        session,
                        text: typed.to_owned(),
                    }),
                );
            }
        })
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || {
            for_appearance(color(if form == Form::Terminal {
                Token::Ink3
            } else {
                Token::Ink4
            }))
        })
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT);
    match form {
        Form::Terminal => view()
            .flex_row()
            .items_center()
            .w(term::cells(28.0))
            .child(entry.flex_1().h(term::rows(1.0)))
            .into_element(),
        Form::Desktop | Form::Phone => {
            let (height, size) = if form == Form::Phone {
                (TARGET, 16.0)
            } else {
                (28.0, text_size(form))
            };
            view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .h(px(height))
                .px(px(8.0))
                .when(form == Form::Desktop, |field| field.w(px(240.0)))
                .when(form == Form::Phone, |field| field.flex_1().min_w(px(160.0)))
                .rounded(px(if form == Form::Phone { 10.0 } else { 6.0 }))
                .border_1()
                .border_color(color(Token::Rule2))
                .bg(color(Token::Surface))
                .focus_within(|s| s.border_color(color(Token::Focus)))
                .child(icon(Icon::Search, 14.0, color(Token::Ink3)))
                .child(
                    entry
                        .flex_1()
                        .h(px(height))
                        .font_family(FONT_TEXT)
                        .text_size(px(size)),
                )
                .into_element()
        }
    }
}
