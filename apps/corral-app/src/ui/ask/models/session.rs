//! A session's model pill and effort control in the composer bar, and the model or effort list
//! they open over the composer (`model::session::Picker`): its title, the catalog's refresh
//! state, `All | Scoped` where the host has scoped models, a filter that keeps the keyboard
//! (↑ ↓ move, Enter uses the highlighted row, Esc closes), then the rows. Each model row is its
//! name over its full `<provider>/<model>` id, the provider in stronger ink, so a vendor's
//! subscription and API providers read as the separate choices they are; its context window and
//! effort range at the trailing edge, `current`, `default` and `next` as words; a leading star,
//! filled for a favorite, marks or unmarks it (Alt+S on the highlighted row). While the host
//! cannot change the model (or its favorites) the rows (or stars) stay listed, muted, with its
//! reason above them. Desktop and phone: a panel in flow over the composer (the phone's rows full
//! touch targets); terminal: the same rows between rules. Test ids `session-picker`,
//! `field-picker`, `pick-<n>`, `pick-<n>-favorite`, `btn-model`, `btn-effort`.

use nsh_core::element::color as for_appearance;
use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::session::{
    Action, FleetAction, PickRow, Picker, PickerAction, PickerKind, ReadState, SessionComposer,
};
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::atoms::control_height;
use crate::ui::composer::copyable_input;
use crate::ui::composer::{PHONE_CHIP, chip_slop};
use crate::ui::kit::button::seg;
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::DISCLOSURE;

fn act(app: AppState, action: Action) {
    app.reduce(app.session, move |state, feed, _, _| {
        state.act(action, feed)
    });
}

/// Words at the bar's size: mono for an id.
fn small(form: Form, words: Text, mono: bool) -> Text {
    match form {
        Form::Terminal => words,
        Form::Phone => words
            .font_family(if mono { FONT_MONO } else { FONT_TEXT })
            .text_size(px(13.0)),
        Form::Desktop => words
            .font_family(if mono { FONT_MONO } else { FONT_TEXT })
            .text_size(px(if mono { 11.5 } else { 12.0 })),
    }
}

/// A pill's face: a terminal's band, the desktop's quiet outline, and on a phone a compact chip
/// whose touch target stays full height by its hit area.
fn face(form: Form, face: View) -> View {
    match form {
        Form::Terminal => face.px(term::cells(1.0)).bg(color(Token::Surface)),
        Form::Desktop | Form::Phone => {
            let (height, pad, radius) = if form == Form::Phone {
                (PHONE_CHIP, 10.0, 8.0)
            } else {
                (control_height(24.0), 8.0, 6.0)
            };
            face.h(px(height))
                .px(px(pad))
                .rounded(px(radius))
                .border_1()
                .border_color(color(Token::Rule2))
                .hover(|s| s.bg(color(Token::Hover)))
                .focus_visible(|s| {
                    s.outline(px(2.0))
                        .outline_color(color(Token::Focus))
                        .outline_offset(px(2.0))
                })
                .when(form == Form::Phone, |face| face.hit_slop(chip_slop()))
        }
    }
}

/// The model's words without its provider (`claude-sonnet-4-6`, not
/// `anthropic/claude-sonnet-4-6`), for the phone's chip; the full id stays in its label.
fn without_provider(words: &str) -> String {
    words
        .split(" → ")
        .map(|part| part.rsplit_once('/').map_or(part, |(_, model)| model))
        .collect::<Vec<_>>()
        .join(" → ")
}

/// The model pill and the effort control of the session `id`'s composer: the model the host
/// reports (`→ next` until it takes a pending one), pressing it opens the model list; the
/// effort, pressing it cycles the levels, holding it (or the secondary press) opens their list.
/// Nothing for the effort when the model has no levels. While an agent's conversation is open
/// both are that agent's: the pill opens its model choices and the effort is words only.
pub(crate) fn session_pill(
    app: AppState,
    id: Id,
    composer: Memo<Option<SessionComposer>>,
) -> AnyElement {
    let form = app.form;
    let model = computed(move || composer.with(|c| c.as_ref().and_then(|c| c.model.clone())));
    let effort = computed(move || composer.with(|c| c.as_ref().and_then(|c| c.effort.clone())));
    let open = {
        let id = id.clone();
        computed(move || app.session.with(|state| state.ui(id.as_str()).picker.open))
    };
    let words = computed(move || {
        model.with(|model| match model {
            Some(model) => match &model.pending {
                Some(next) => format!("{} → {next}{DISCLOSURE}", model.label),
                None => format!("{}{DISCLOSURE}", model.label),
            },
            None => format!("model not reported{DISCLOSURE}"),
        })
    });
    let shown = computed(move || {
        let words = words.get();
        if form == Form::Phone {
            without_provider(&words)
        } else {
            words
        }
    });
    let muted =
        computed(move || model.with(|m| m.as_ref().is_none_or(|m| m.unavailable.is_some())));
    let toggle = {
        let id = id.clone();
        move || {
            // An opened agent's pill opens that agent's model choices, or nothing while it
            // cannot change.
            let agent = model.with_untracked(|m| {
                m.as_ref().and_then(|m| {
                    m.opens
                        .clone()
                        .map(|opens| (opens, m.unavailable.is_none()))
                })
            });
            if let Some((opens, changeable)) = agent {
                if changeable {
                    act(app, opens);
                }
                return;
            }
            let action = if open.get_untracked() == Some(PickerKind::Model) {
                PickerAction::Close(id.clone())
            } else {
                PickerAction::Open {
                    session: id.clone(),
                    kind: PickerKind::Model,
                }
            };
            act(app, Action::Picker(action));
        }
    };
    let pill = view()
        .test_id("btn-model")
        .role(AccessibilityRole::Button)
        .label(move || {
            let why = model.with(|m| m.as_ref().and_then(|m| m.unavailable.clone()));
            let mut label = format!("Model: {}", words.get().trim_end_matches(DISCLOSURE));
            if let Some(why) = why {
                label.push_str(&format!(", cannot change now: {why}"));
            }
            SharedString::from(label)
        })
        .accessibility_state(move || AccessibilityState {
            // An opened agent's pill is expanded while its own choices are open (pressing
            // would close them).
            expanded: Some(
                model.with(|m| match m.as_ref().and_then(|m| m.opens.as_ref()) {
                    Some(opens) => matches!(
                        opens,
                        Action::Fleet(FleetAction::Models { child: None, .. })
                    ),
                    None => open.get() == Some(PickerKind::Model),
                }),
            ),
            has_popup: true,
            ..AccessibilityState::default()
        })
        .flex_row()
        .items_center()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .cursor_pointer()
        .on_press(toggle)
        .child(
            small(
                form,
                text(move || shown.get())
                    .number_of_lines(1u32)
                    .truncate()
                    .text_color(move || {
                        if muted.get() {
                            color(Token::Ink3)
                        } else if form == Form::Terminal {
                            color(Token::Ink)
                        } else {
                            color(Token::Ink2)
                        }
                    }),
                true,
            )
            .min_w(px(0.0)),
        );
    let effort_control = dynamic(move || {
        let (level, fixed) = effort.with(|effort| {
            effort.as_ref().map(|effort| {
                let level = effort
                    .level
                    .clone()
                    .unwrap_or_else(|| "not reported".into());
                (level, effort.fixed)
            })
        })?;
        if fixed {
            return Some(untracked(|| effort_words(app, level)));
        }
        let id = id.clone();
        Some(untracked(|| effort_button(app, id, level)))
    });
    view()
        .flex_row()
        .items_center()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .gap(if form == Form::Terminal {
            term::cells(1.0)
        } else {
            px(6.0)
        })
        .child(face(form, pill))
        .child(effort_control)
        .into_element()
}

/// `effort high`: an opened agent's effort, as reported; nothing to press.
fn effort_words(app: AppState, level: String) -> AnyElement {
    let form = app.form;
    let words = view()
        .test_id("btn-effort")
        .label(format!("Effort {level}"))
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .child(small(
            form,
            text(if form == Form::Phone {
                level.clone()
            } else {
                format!("effort {level}")
            })
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3)),
            false,
        ));
    match form {
        Form::Terminal => words.px(term::cells(1.0)).into_element(),
        Form::Desktop | Form::Phone => words.into_element(),
    }
}

/// `effort high ↻`: a press cycles, a long or secondary press opens the effort list.
fn effort_button(app: AppState, id: Id, level: String) -> AnyElement {
    let form = app.form;
    let cycle = {
        let id = id.clone();
        move || act(app, Action::Picker(PickerAction::CycleEffort(id.clone())))
    };
    let list = move || {
        act(
            app,
            Action::Picker(PickerAction::Open {
                session: id.clone(),
                kind: PickerKind::Effort,
            }),
        );
    };
    let list_too = list.clone();
    let face_view = view()
        .test_id("btn-effort")
        .role(AccessibilityRole::Button)
        .label(format!(
            "Effort {level}; press to use the next level, hold for the list"
        ))
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .cursor_pointer()
        .on_press(cycle)
        .on_long_press(move || list())
        .on_context_menu(move |_: &ContextMenuEvent| list_too())
        .child(small(
            form,
            text(if form == Form::Phone {
                level.clone()
            } else {
                format!("effort {level} ↻")
            })
            .number_of_lines(1u32)
            .text_color(color(if form == Form::Terminal {
                Token::Ink
            } else {
                Token::Ink2
            })),
            false,
        ));
    face(form, face_view).into_element()
}

/// The open picker over the composer; nothing while none is open. Mounted once per opening, so
/// the filter keeps the keyboard while the rows follow it.
pub(crate) fn session_picker(app: AppState, picker: Memo<Option<Picker>>) -> AnyElement {
    let opened = computed(move || picker.with(|p| p.as_ref().map(|p| (p.kind, p.session.clone()))));
    dynamic(move || {
        let (kind, session) = opened.get()?;
        Some(untracked(|| panel(app, picker, kind, session)))
    })
}

fn panel(app: AppState, picker: Memo<Option<Picker>>, kind: PickerKind, session: Id) -> AnyElement {
    let form = app.form;
    let field = element_ref();
    after_answer(move || field.focus());
    // In a side pane set aside the picker is hidden: its arrows go, and its filter gives the
    // keyboard back, until the pane does.
    {
        let session = session.clone();
        app.bind_while_shown(move || {
            for (key, delta) in [("up", -1_i8), ("down", 1)] {
                let session = session.clone();
                on_key_with(
                    key,
                    KeyOptions {
                        when: When::Always,
                        ..KeyOptions::default()
                    },
                    move |_| {
                        act(
                            app,
                            Action::Picker(PickerAction::Step {
                                session: session.clone(),
                                delta,
                            }),
                        );
                    },
                );
            }
        });
    }
    effect(move || {
        if !app.binds_keys() {
            field.blur();
        }
    });
    let filter = {
        let session = session.clone();
        let chosen = session.clone();
        let displayed =
            move || picker.with(|p| p.as_ref().map(|p| p.filter.clone()).unwrap_or_default());
        copyable_input(app, Some(field), displayed)
            .test_id("field-picker")
            .label(match kind {
                PickerKind::Model => "Filter models",
                PickerKind::Effort => "Filter effort levels",
            })
            .placeholder(match kind {
                PickerKind::Model => "Type to filter models…",
                PickerKind::Effort => "Type to filter levels…",
            })
            .on_change(move |text: &str| {
                act(
                    app,
                    Action::Picker(PickerAction::Filter {
                        session: session.clone(),
                        text: text.to_owned(),
                    }),
                );
            })
            .on_submit(move |_: &str| {
                if app.field_acts() {
                    act(app, Action::Picker(PickerAction::Choose(chosen.clone())));
                }
            })
            .flex_1()
            .min_w(px(0.0))
            .text_color(color(Token::Ink))
            .placeholder_color(move || for_appearance(color(Token::Ink3)))
            .caret_color(move || for_appearance(color(Token::Ink)))
            .bg(AdaptiveColor::TRANSPARENT)
            .when(form == Form::Terminal, |f| f.h(term::rows(1.0)))
            .when(form != Form::Terminal, |f| {
                f.h(px(if form == Form::Phone { 44.0 } else { 28.0 }))
                    .font_family(FONT_TEXT)
                    .text_size(px(if form == Form::Phone { 16.0 } else { 13.0 }))
            })
    };
    let head = head(app, picker);
    let rows = dynamic(move || {
        let shown = picker.get()?;
        Some(untracked(|| body(app, &shown)))
    });
    let shell = view()
        .test_id("session-picker")
        .role(AccessibilityRole::Dialog)
        .label(move || {
            SharedString::from(
                picker.with(|p| p.as_ref().map(|p| p.title.clone()).unwrap_or_default()),
            )
        });
    match form {
        Form::Terminal => shell
            .flex_none()
            .child(term::hrule())
            .child(view().px(term::cells(1.0)).child(head))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .px(term::cells(1.0))
                    .child(term::quiet("› "))
                    .child(filter),
            )
            .child(rows)
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .flex_none()
            .px(px(14.0))
            .pt(px(6.0))
            .child(
                shell
                    .p(px(6.0))
                    .gap(px(4.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(color(Token::Rule2))
                    .bg(color(Token::Surface))
                    .child(head)
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .px(px(8.0))
                            .rounded(px(7.0))
                            .bg(color(Token::BgSunk))
                            .border_1()
                            .border_color(color(Token::Rule))
                            .child(filter),
                    )
                    .child(rows),
            )
            .into_element(),
    }
}

/// The title with the filter's count, the scope switch, the catalog's refresh state and Close.
fn head(app: AppState, picker: Memo<Option<Picker>>) -> AnyElement {
    let form = app.form;
    let title = computed(move || {
        picker.with(|p| {
            p.as_ref().map_or_else(String::new, |p| match &p.count {
                Some(count) => format!("{} · {count}", p.title),
                None => p.title.clone(),
            })
        })
    });
    let catalog = computed(move || picker.with(|p| p.as_ref().and_then(|p| p.catalog.clone())));
    let scoped = computed(move || picker.with(|p| p.as_ref().and_then(|p| p.scope.clone())));
    let close = computed(move || picker.with(|p| p.as_ref().map(|p| p.close.clone())));
    let words = text(move || title.get())
        .number_of_lines(1u32)
        .truncate()
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    let words = if form == Form::Terminal {
        words
    } else {
        words.font_family(FONT_TEXT).text_size(px(13.0))
    };
    let scope = dynamic(move || {
        let switch = scoped.get()?;
        Some(untracked(|| {
            let (all, only) = (switch.all.clone(), switch.only.clone());
            seg(
                app,
                &[(false, "All"), (true, "Scoped")],
                move || scoped.with(|s| s.as_ref().is_some_and(|s| s.scoped)),
                move |pick: bool| act(app, if pick { only.clone() } else { all.clone() }),
                "seg-models",
            )
        }))
    });
    let refresh = dynamic(move || {
        let (hue, words) = catalog.get()?;
        Some(untracked(|| {
            small(
                form,
                text(words)
                    .number_of_lines(1u32)
                    .truncate()
                    .text_color(if hue == Hue::Neutral {
                        color(Token::Ink3)
                    } else {
                        hue_ink(hue)
                    }),
                false,
            )
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .into_element()
        }))
    });
    let close_button = view()
        .test_id("btn-picker-close")
        .role(AccessibilityRole::Button)
        .label("Close")
        .cursor_pointer()
        .on_press(move || {
            if let Some(action) = close.get_untracked() {
                act(app, action);
            }
        })
        .child(small(
            form,
            text(if form == Form::Terminal { "esc" } else { "Esc" }).text_color(color(Token::Ink3)),
            false,
        ))
        .when(form == Form::Phone, |v| {
            v.min_h(px(44.0)).px(px(10.0)).items_center()
        });
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(if form == Form::Terminal {
            term::cells(2.0)
        } else {
            px(10.0)
        })
        .when(form != Form::Terminal, |v| v.px(px(4.0)).pt(px(2.0)))
        .child(words.flex_shrink(1.0).min_w(px(0.0)))
        .child(scope)
        .child(view().flex_1().min_w(px(0.0)))
        .child(refresh)
        .child(close_button)
        .into_element()
}

/// The notes over the rows (why the host cannot change it now, where the read stands, why the
/// list is empty), the rows, then the highlighted row's full name.
fn body(app: AppState, picker: &Picker) -> AnyElement {
    let form = app.form;
    let line = |words: String, hue: Hue| {
        let words = small(
            form,
            text(words).text_color(if hue == Hue::Neutral {
                color(Token::Ink3)
            } else {
                hue_ink(hue)
            }),
            false,
        );
        if form == Form::Terminal {
            view().px(term::cells(1.0)).child(words).into_element()
        } else {
            view().px(px(8.0)).py(px(2.0)).child(words).into_element()
        }
    };
    let mut notes = Vec::new();
    if let Some(why) = &picker.unavailable {
        notes.push(line(
            format!("The model cannot change now: {why}"),
            Hue::Held,
        ));
    }
    if let Some(why) = &picker.favorites {
        notes.push(line(
            format!("Favorites cannot change now: {why}"),
            Hue::Neutral,
        ));
    }
    match &picker.read {
        ReadState::Reading | ReadState::ReadingKept if picker.kind == PickerKind::Model => {
            notes.push(line("Reading the model catalog…".into(), Hue::Neutral));
        }
        ReadState::Failed(why) => {
            notes.push(line(
                format!("Could not read the model catalog: {why}"),
                Hue::Fail,
            ));
        }
        _ => {}
    }
    if let Some(note) = &picker.note {
        notes.push(line(note.clone(), Hue::Neutral));
    }
    let rows: Vec<AnyElement> = picker
        .rows
        .iter()
        .enumerate()
        .map(|(at, row)| {
            option(
                app,
                row,
                picker.at == Some(at),
                picker.unavailable.is_some(),
            )
        })
        .collect();
    let detail = picker
        .detail
        .clone()
        .map(|detail| line(detail, Hue::Neutral));
    let list = view()
        .role(AccessibilityRole::Listbox)
        .label(picker.title.clone())
        .children(rows);
    let list = if form == Form::Terminal {
        scroll().max_h(term::rows(10.0)).child(list).into_element()
    } else {
        scroll()
            .max_h(px(if form == Form::Phone { 320.0 } else { 280.0 }))
            .child(list.gap(px(1.0)))
            .into_element()
    };
    view()
        .children(notes)
        .child(list)
        .children(detail)
        .into_element()
}

/// One row: the check on the current one; its name over its id with the provider in stronger
/// ink (a model) or its description (an effort); its facts and marks at the trailing edge.
fn option(app: AppState, row: &PickRow, highlighted: bool, muted: bool) -> AnyElement {
    let form = app.form;
    let check = if row.current {
        mark(app, Mark::Done, Hue::Ok)
    } else {
        view()
            .w(if form == Form::Terminal {
                term::cells(1.0)
            } else {
                px(12.0)
            })
            .into_element()
    };
    let ink = |strong: Token| color(if muted { Token::Ink3 } else { strong });
    let name = small(
        form,
        text(row.label.clone())
            .number_of_lines(1u32)
            .truncate()
            .font_weight(FontWeight::BOLD)
            .text_color(ink(Token::Ink)),
        false,
    );
    let id = row.provider.as_ref().map(|provider| {
        let rest = row
            .model
            .strip_prefix(provider.as_str())
            .unwrap_or(row.model.as_str())
            .to_owned();
        view()
            .flex_row()
            .min_w(px(0.0))
            .child(
                small(
                    form,
                    text(provider.clone()).text_color(ink(Token::Ink2)),
                    true,
                )
                .flex_shrink_0(),
            )
            .child(
                small(
                    form,
                    text(rest)
                        .number_of_lines(1u32)
                        .truncate()
                        .text_color(color(Token::Ink3)),
                    true,
                )
                .min_w(px(0.0)),
            )
    });
    let mut facts = row.meta.clone();
    for word in &row.marks {
        if facts.is_empty() {
            facts.push_str(word);
        } else {
            facts.push_str(&format!(" · {word}"));
        }
    }
    let facts = small(
        form,
        text(facts)
            .number_of_lines(1u32)
            .truncate()
            .text_color(color(Token::Ink3)),
        false,
    )
    .flex_shrink(1.0)
    .min_w(px(0.0))
    .text_right();
    let action = row.action.clone();
    let enabled = action.is_some();
    let mut label = format!("{} · {}", row.label, row.model);
    if !row.meta.is_empty() {
        label.push_str(&format!(" · {}", row.meta));
    }
    let item = view()
        .test_id(&row.id)
        .role(AccessibilityRole::Option)
        .label(label)
        .accessibility_state(AccessibilityState {
            selected: row.current,
            disabled: !enabled,
            ..AccessibilityState::default()
        })
        .flex_row()
        .items_center()
        .flex_1()
        .min_w(px(0.0))
        .when(enabled, |v| {
            v.cursor_pointer().on_press(move || {
                if let Some(action) = &action {
                    act(app, action.clone());
                }
            })
        });
    // The star is the row's sibling, so pressing it never also chooses the model.
    let star = row.favorite.map(|favorite| star(app, row, favorite));
    let line = view().flex_row().items_center();
    if form == Form::Terminal {
        let item = item
            .gap(term::cells(1.0))
            .child(check)
            .child(name.flex_shrink_0());
        let item = match id {
            Some(id) => item.child(id.flex_1()),
            None => item.child(view().flex_1()),
        }
        .child(facts);
        return line
            .gap(term::cells(1.0))
            .px(term::cells(1.0))
            .when(highlighted, |v| v.bg(color(Token::Surface2)))
            .children(star)
            .child(item)
            .into_element();
    }
    let phone = form == Form::Phone;
    let item = item
        .gap(px(8.0))
        .py(px(if phone { 8.0 } else { 6.0 }))
        .when(phone, |v| v.min_h(px(44.0)))
        .child(view().w(px(16.0)).items_center().child(check))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(1.0))
                .child(name)
                .children(id),
        )
        .child(facts.max_w(px(if phone { 140.0 } else { 220.0 })));
    line.gap(px(8.0))
        .px(px(8.0))
        .rounded(px(7.0))
        .when(highlighted, |v| v.bg(color(Token::Sel)))
        .hover(|s| s.bg(color(Token::Hover)))
        .children(star)
        .child(item)
        .into_element()
}

/// A model's star: filled for a favorite of the workspace, hollow otherwise; pressing it marks
/// or unmarks the model and keeps the list open. Muted, and pressing it does nothing, while the
/// host cannot change favorites (the reason stands over the rows).
fn star(app: AppState, row: &PickRow, favorite: bool) -> AnyElement {
    let form = app.form;
    let action = row.star.clone();
    let enabled = action.is_some();
    let ink = match (enabled, favorite) {
        (true, true) => hue_ink(Hue::Held),
        (true, false) => color(Token::Ink2),
        (false, _) => color(Token::Ink3),
    };
    let label = match (favorite, enabled) {
        (true, true) => format!("Remove {} from favorites", row.label),
        (false, true) => format!("Add {} to favorites", row.label),
        (true, false) => format!("{} is a favorite; favorites cannot change now", row.label),
        (false, false) => format!(
            "{} is not a favorite; favorites cannot change now",
            row.label
        ),
    };
    view()
        .test_id(&format!("{}-favorite", row.id))
        .role(AccessibilityRole::Button)
        .label(label)
        .accessibility_state(AccessibilityState {
            selected: favorite,
            disabled: !enabled,
            ..AccessibilityState::default()
        })
        .flex_none()
        .items_center()
        .justify_center()
        .when(form == Form::Phone, |v| v.min_w(px(36.0)).min_h(px(44.0)))
        .when(enabled, |v| {
            v.cursor_pointer().on_press(move || {
                if let Some(action) = &action {
                    act(app, action.clone());
                }
            })
        })
        .child(small(
            form,
            text(if favorite { "★" } else { "☆" }).text_color(ink),
            false,
        ))
        .into_element()
}
