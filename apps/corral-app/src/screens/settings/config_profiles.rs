//! Settings › Configuration › Profiles: a searchable list of the draft's profiles beside the
//! one chosen, which shows its harness, model and effort, with its arguments folded under
//! Advanced. From 640 pt (84 Cells) of room the list stands beside the profile; narrower, and
//! on the phone, the list shows first and a profile opens in its place with a way back. New
//! and Duplicate ask for a name first, refusing any key the draft's `[profiles]` already holds
//! and warning, on the button too, when the name overrides one of corral's built-in profiles.
//! The workspace default is the draft's, else corral's reported one. Remove sits in the
//! profile's More menu and says what in the draft still names the profile before it goes,
//! leaving those keys for the owner to change.
//! Model and effort take any text; the values other profiles of the same harness set are
//! offered beside it, read from the draft as it is typed so the fields never remount. IDs:
//! `seg-config-profiles-<name>` for a row, `field-config-profiles-search`,
//! `btn-config-new-profiles`, `field-config-new-profiles` and `btn-config-add-profiles` for the
//! name, `btn-config-duplicate-profiles`, `btn-config-profile-more`, `btn-config-profile-remove`,
//! `btn-config-remove-profiles`, `btn-config-profile-ref-<n>`, `btn-config-profile-back`,
//! `btn-config-profile-advanced`, and `seg-config-<key>-<value>`, `btn-config-<key>-unset` for a
//! model's or effort's offers.

use nsh_base::{menu, menu_item};
use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use super::config_fields::{edit, entry, field, on_jump, row};
use super::parts::{button_row, cell, mono_cell, space};
use crate::model::Form;
use crate::model::settings::document::{self, Path};
use crate::model::settings::editor::{Edit, Part};
use crate::model::settings::form::{Entries, Field, Group, Live};
use crate::model::settings::profiles::{self, Check, Naming, Taken, key_of};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with, menu_trigger};
use crate::ui::kit::overlay::counted;
use crate::ui::kit::rows::{list_row, list_row_tall};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// Room from which the list stands beside the profile: points, and Cells in a terminal.
const BESIDE: f32 = 640.0;
const BESIDE_CELLS: f32 = 84.0;
/// The list's width beside the profile.
const LIST: f32 = 250.0;
const LIST_CELLS: f32 = 30.0;
/// The widest the profile's fields read at on the desktop.
const DETAIL: f32 = 600.0;
const MENU_MIN: f32 = 180.0;
const TARGET: f32 = 44.0;

/// What every part of the screen reads: the shape's profiles, the one chosen and its fields,
/// the draft's values, the names a new profile may land on, and the open name panel.
#[derive(Clone)]
struct Cx {
    names: Vec<String>,
    chosen: Option<String>,
    fields: Vec<Field>,
    live: Memo<Live>,
    taken: Memo<Taken>,
    naming: Signal<Option<Naming>>,
    typed: Signal<String>,
}

/// The Profiles part: the name panel when open, then the list and the chosen profile, beside
/// each other or one at a time as the room allows.
pub(super) fn profiles(
    app: AppState,
    listed: &Entries,
    group: Option<&Group>,
    live: Memo<Live>,
) -> AnyElement {
    let form = app.form;
    let cx = Cx {
        names: listed.names.clone(),
        chosen: listed.chosen.clone(),
        fields: group.map(|group| group.fields.clone()).unwrap_or_default(),
        live,
        taken: computed(move || {
            let draft = app.settings.with(|state| state.config.draft.clone());
            app.feed.with(|feed| Taken::read(&draft, feed))
        }),
        naming: signal(None),
        typed: signal(String::new()),
    };
    let room = signal(None::<f32>);
    let wide = computed(move || match (form, room.get()) {
        (Form::Phone, _) => false,
        (Form::Desktop, width) => width.is_none_or(|width| width >= BESIDE),
        (Form::Terminal, Some(width)) => width >= term::cells(BESIDE_CELLS).0,
        (Form::Terminal, None) => term::page_columns(app) >= BESIDE_CELLS,
    });
    // Back and Escape read whether the list stands beside the profile; written on a change only.
    effect(move || {
        let beside = wide.get();
        let held = app
            .settings
            .with_untracked(|state| state.config.profile_beside);
        untracked(move || {
            if held != beside {
                app.act(app.settings, move |state, _| {
                    state.config.profile_beside = beside;
                    Vec::new()
                });
            }
        });
    });
    let focused = computed(move || app.settings.with(|state| state.config.profile_focus));
    let layout = {
        let cx = cx.clone();
        dynamic(move || {
            let (wide, focused) = (wide.get(), focused.get());
            let cx = cx.clone();
            untracked(move || match (wide, cx.chosen.clone()) {
                (true, _) => split(app, &cx),
                (false, Some(name)) if focused => detail(app, &cx, name, false),
                _ => list(app, &cx),
            })
        })
        .into_element()
    };
    view()
        .min_w(px(0.0))
        .on_layout(move |frame: &Bounds<Points>| {
            let width = Some(frame.size.width.0);
            if room.get_untracked() != width {
                room.set(width);
            }
        })
        .child(cell(app, profiles::NOTE, Token::Ink3, false))
        .child(space(app, 10.0))
        .child(naming(app, &cx))
        .child(layout)
        .into_element()
}

/// The list beside the chosen profile, a rule between them.
fn split(app: AppState, cx: &Cx) -> AnyElement {
    let form = app.form;
    let side = match form {
        Form::Terminal => view().w(term::cells(LIST_CELLS)),
        _ => view().w(px(LIST)),
    }
    .flex_shrink_0()
    .min_w(px(0.0))
    .child(list(app, cx));
    let rule = match form {
        Form::Terminal => term::vrule(),
        _ => view()
            .w(px(1.0))
            .self_stretch()
            .flex_shrink_0()
            .bg(color(Token::Rule))
            .into_element(),
    };
    let shown = match cx.chosen.clone() {
        Some(name) => detail(app, cx, name, true),
        None => view().into_element(),
    };
    view()
        .flex_row()
        .items_start()
        .min_w(px(0.0))
        .gap(match form {
            Form::Terminal => term::cells(1.0),
            _ => px(18.0),
        })
        .child(side)
        .child(rule)
        .child(view().flex_1().min_w(px(0.0)).child(shown))
        .into_element()
}

/// New profile, the search, then a row per profile the search keeps.
fn list(app: AppState, cx: &Cx) -> AnyElement {
    let (naming, typed) = (cx.naming, cx.typed);
    let mut parts = vec![button_row(
        app,
        vec![button_with(
            app,
            profiles::NEW,
            ButtonKind::Ghost,
            ButtonOptions {
                icon: Some(Icon::Plus),
                ..ButtonOptions::default()
            },
            "btn-config-new-profiles",
            move || {
                typed.set(String::new());
                naming.set(Some(Naming::New));
            },
        )],
    )];
    parts.push(space(app, 8.0));
    if cx.names.is_empty() {
        parts.push(cell(app, profiles::EMPTY, Token::Ink3, false).into_element());
        return view().min_w(px(0.0)).children(parts).into_element();
    }
    parts.push(entry(
        app,
        "field-config-profiles-search".into(),
        profiles::SEARCH,
        move || {
            app.settings
                .with(|state| state.config.profile_query.clone())
        },
        move |text| {
            app.act(app.settings, move |state, _| {
                state.config.profile_query = text;
                Vec::new()
            });
        },
    ));
    parts.push(space(app, 6.0));
    let names = cx.names.clone();
    let live = cx.live;
    let kept = computed(move || {
        let query = app
            .settings
            .with(|state| state.config.profile_query.clone());
        let kept = live.with(|live| profiles::filtered(&names, &live.draft, &query));
        (kept, query)
    });
    let cx = cx.clone();
    parts.push(
        dynamic(move || {
            let (kept, query) = kept.get();
            let cx = cx.clone();
            untracked(move || {
                if kept.is_empty() {
                    return cell(app, profiles::no_match(&query), Token::Ink3, false)
                        .into_element();
                }
                view()
                    .min_w(px(0.0))
                    .role(AccessibilityRole::Group)
                    .label("Profiles")
                    .children(kept.into_iter().map(|name| {
                        let selected = cx.chosen.as_deref() == Some(name.as_str());
                        profile_row(app, &cx, name, selected)
                    }))
                    .into_element()
            })
        })
        .into_element(),
    );
    view().min_w(px(0.0)).children(parts).into_element()
}

/// A profile's row: its name and whether the workspace defaults to it, by the draft or else
/// corral, its model in full, its harness and effort; one line in a terminal.
fn profile_row(app: AppState, cx: &Cx, name: String, selected: bool) -> AnyElement {
    let (form, live, taken) = (app.form, cx.live, cx.taken);
    let read = name.clone();
    let summary = computed(move || {
        let default = taken.with(|taken| taken.default.clone());
        live.with(|live| profiles::summary(&live.draft, &read, default.as_deref()))
    });
    let shown = name.clone();
    let content = dynamic(move || {
        let summary = summary.get();
        let name = shown.clone();
        untracked(move || {
            let default = summary.default.then(|| {
                cell(app, profiles::DEFAULT_MARK, Token::Ink3, false)
                    .flex_shrink_0()
                    .into_element()
            });
            // A terminal's row is one line: the name, then the model cut to the room left.
            let line = form == Form::Terminal;
            let model = mono_cell(app, summary.model_words(), Token::Ink2).when(line, |model| {
                model.number_of_lines(1u32).truncate().flex_1()
            });
            view()
                .flex_1()
                .min_w(px(0.0))
                .when(line, |row| row.flex_row().gap(term::cells(1.0)))
                .when(!line, |row| row.gap(px(2.0)))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .flex_shrink_0()
                        .child(cell(app, name, Token::Ink, true))
                        .children(default),
                )
                .child(model)
                .when(!line, |row| {
                    row.child(cell(app, summary.runs_words(), Token::Ink3, false))
                })
                .into_element()
        })
    })
    .into_element();
    let label = name.clone();
    let id = format!("seg-config-profiles-{name}");
    let press = move || {
        let name = name.clone();
        app.act(app.settings, move |state, _| {
            state.choose_entry(Part::Profiles, name)
        });
    };
    if form == Form::Terminal {
        list_row(app, label, id, move || selected, press, vec![content])
    } else {
        list_row_tall(app, label, id, move || selected, press, vec![content])
    }
}

/// The name a new or duplicated profile takes: the field, why the name cannot be used or the
/// built-in profile it overrides, and the button that makes it, which says so. Enter makes it
/// too.
fn naming(app: AppState, cx: &Cx) -> AnyElement {
    let (naming, typed, taken) = (cx.naming, cx.typed, cx.taken);
    dynamic(move || {
        let making = naming.get()?;
        Some(untracked(move || {
            naming_panel(app, making, taken, naming, typed)
        }))
    })
    .into_element()
}

fn naming_panel(
    app: AppState,
    making: Naming,
    taken: Memo<Taken>,
    naming: Signal<Option<Naming>>,
    typed: Signal<String>,
) -> AnyElement {
    let form = app.form;
    let create = {
        let making = making.clone();
        move || {
            let name = typed.get_untracked().trim().to_owned();
            if !taken.with_untracked(|taken| taken.check(&name).ready()) {
                return;
            }
            naming.set(None);
            typed.set(String::new());
            let change = match &making {
                Naming::New => Edit::AddEntry(Part::Profiles, name),
                Naming::Duplicate(from) => Edit::DuplicateProfile {
                    from: from.clone(),
                    name,
                },
            };
            app.act(app.settings, move |state, _| {
                // A search the new name does not match would hide what was just made.
                state.config.profile_query.clear();
                state.edit_config(change)
            });
        }
    };
    let check = computed(move || typed.with(|typed| taken.with(|taken| taken.check(typed))));
    let confirm = {
        let (create, making) = (create.clone(), making.clone());
        dynamic(move || {
            let check = check.get();
            let (create, action) = (create.clone(), making.action(&check));
            untracked(move || {
                button_with(
                    app,
                    action,
                    ButtonKind::Primary,
                    ButtonOptions {
                        disabled: !check.ready(),
                        ..ButtonOptions::default()
                    },
                    "btn-config-add-profiles",
                    create,
                )
            })
        })
        .into_element()
    };
    let cancel = button(
        app,
        profiles::CANCEL,
        ButtonKind::Quiet,
        "btn-config-name-cancel",
        move || {
            naming.set(None);
            typed.set(String::new());
        },
    );
    let warning = dynamic(move || {
        let (words, token) = match check.get() {
            Check::Refused(words) => (words, Token::Fail),
            Check::Overrides(words) => (words, Token::Held),
            Check::Empty | Check::Free => return None,
        };
        Some(untracked(move || {
            cell(app, words, token, false).into_element()
        }))
    })
    .into_element();
    let panel = view()
        .test_id("config-profile-name")
        .role(AccessibilityRole::Group)
        .label(making.title())
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .child(cell(app, making.title(), Token::Ink, true))
        .child(name_field(app, typed, create))
        .child(warning)
        .child(button_row(app, vec![confirm, cancel]));
    view()
        .min_w(px(0.0))
        .child(boxed(app, panel))
        .child(space(app, 12.0))
        .into_element()
}

/// The name's field, taking the keyboard focus as the panel opens; Enter submits.
fn name_field(app: AppState, typed: Signal<String>, submit: impl Fn() + 'static) -> AnyElement {
    let form = app.form;
    let handle = element_ref();
    after_answer(move || handle.focus());
    let field = input()
        .element_ref(&handle)
        .test_id("field-config-new-profiles")
        .label(profiles::NAME_LABEL)
        .placeholder(profiles::NAME_LABEL)
        .value(move || typed.get())
        .on_change(move |text: &str| typed.set(text.to_owned()))
        .on_submit(move |_: &str| submit())
        .auto_correct(false)
        .auto_capitalize(AutoCapitalize::None)
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT)
        .border_1()
        .border_color(color(Token::Rule2))
        .focus(|s| s.border_color(color(Token::Focus)));
    match form {
        Form::Terminal => field.h(term::rows(1.0)).px(term::cells(1.0)).into_element(),
        Form::Desktop => field
            .font_family(FONT_TEXT)
            .text_size(px(13.0))
            .h(px(30.0))
            .px(px(9.0))
            .rounded(px(6.0))
            .into_element(),
        Form::Phone => field
            .font_family(FONT_TEXT)
            .text_size(px(16.0))
            .min_h(px(TARGET))
            .px(px(12.0))
            .rounded(px(10.0))
            .into_element(),
    }
}

/// The chosen profile: the way back when it stands alone, its name with Duplicate and More,
/// the refusal's reasons about it, the Remove warning while open, then its fields.
fn detail(app: AppState, cx: &Cx, name: String, wide: bool) -> AnyElement {
    let form = app.form;
    let removing = signal(false);
    let mut parts = Vec::new();
    if !wide {
        parts.push(button_row(
            app,
            vec![button(
                app,
                format!("‹ {}", profiles::BACK),
                ButtonKind::Quiet,
                "btn-config-profile-back",
                move || {
                    app.act(app.settings, |state, _| {
                        state.config.profile_focus = false;
                        Vec::new()
                    });
                },
            )],
        ));
        parts.push(space(app, 6.0));
    }
    parts.push(head(app, cx, &name, removing));
    parts.push(entry_reasons(app, &name));
    let gone = name.clone();
    let warning = dynamic(move || {
        let name = removing.get().then(|| gone.clone())?;
        Some(untracked(move || removal(app, name, removing)))
    });
    parts.push(warning.into_element());
    parts.push(space(app, 10.0));
    // The shape's fields in its order: harness (with its offers), model, effort, arguments.
    let fields: Vec<AnyElement> = cx
        .fields
        .iter()
        .map(|shape| match key_of(shape) {
            Some("model") => chosen_field(app, cx, shape, &name, "model"),
            Some("effort") => chosen_field(app, cx, shape, &name, "effort"),
            Some("args") => advanced(app, shape.clone(), cx.live),
            _ => field(app, shape, cx.live),
        })
        .collect();
    let gap = match form {
        Form::Terminal => term::rows(1.0),
        _ => px(14.0),
    };
    let column = view().min_w(px(0.0)).gap(gap);
    parts.push(column.children(fields).into_element());
    view()
        .test_id("config-profile")
        .min_w(px(0.0))
        .when(form == Form::Desktop, |detail| detail.max_w(px(DETAIL)))
        .children(parts)
        .into_element()
}

/// The profile's name, then Duplicate and its More menu.
fn head(app: AppState, cx: &Cx, name: &str, removing: Signal<bool>) -> AnyElement {
    let form = app.form;
    let (naming, typed, taken) = (cx.naming, cx.typed, cx.taken);
    let copied = name.to_owned();
    let title = cell(app, name.to_owned(), Token::Ink, true)
        .role(AccessibilityRole::Header)
        .when(form != Form::Terminal, |title| title.text_size(px(15.0)));
    let duplicate = button(
        app,
        profiles::DUPLICATE,
        ButtonKind::Ghost,
        "btn-config-duplicate-profiles",
        move || {
            let making = Naming::Duplicate(copied.clone());
            typed.set(taken.with_untracked(|taken| making.initial(taken)));
            naming.set(Some(making));
        },
    );
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .min_w(px(0.0))
        .child(view().flex_1().min_w(px(0.0)).child(title))
        .child(button_row(app, vec![duplicate, more(app, name, removing)]))
        .into_element()
}

/// The profile's quiet More menu: Remove, which asks first.
fn more(app: AppState, name: &str, removing: Signal<bool>) -> AnyElement {
    let form = app.form;
    let phone = form == Form::Phone;
    let words = text(profiles::REMOVE_MENU)
        .number_of_lines(1u32)
        .text_color(color(Token::Fail))
        .when(form != Form::Terminal, |words| {
            words
                .font_family(FONT_TEXT)
                .text_size(px(if phone { 16.0 } else { 12.5 }))
        });
    let content = match form {
        Form::Terminal => view().h(term::rows(1.0)).px(term::cells(1.0)),
        _ => view()
            .min_h(px(if phone { TARGET + 4.0 } else { 28.0 }))
            .px(px(if phone { 18.0 } else { 12.0 })),
    }
    .flex_row()
    .items_center()
    .child(words);
    menu()
        .popup_style(move |s| {
            let s = s
                .bg(color(Token::Surface))
                .border_1()
                .border_color(color(Token::Rule2));
            match form {
                Form::Desktop => s
                    .min_w(px(MENU_MIN))
                    .py(px(4.0))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .shadow_color(color(Token::Shell)),
                Form::Phone => s.rounded_t(px(14.0)).pt(px(8.0)).pb(px(24.0)),
                Form::Terminal => s.min_w(term::cells(20.0)),
            }
        })
        .backdrop_style(|s| s.bg(color(Token::Shell).opacity(0.5)))
        .on_open_change(counted(app))
        .trigger(menu_trigger(
            app,
            profiles::MORE,
            format!("More for {name}"),
            "btn-config-profile-more",
        ))
        .child(
            menu_item()
                .accessibility_label(profiles::REMOVE_MENU)
                .on_select(move || removing.set(true))
                .cursor_pointer()
                .hover(|s| s.bg(color(Token::Hover)))
                .focus(|s| s.bg(color(Token::Hover)))
                .child(content.test_id("btn-config-profile-remove")),
        )
        .into_element()
}

/// The refusal's reasons about the profile itself.
fn entry_reasons(app: AppState, name: &str) -> AnyElement {
    let anchor = Path::of(&["profiles", name]);
    let reasons = computed(move || {
        app.settings.with(|state| {
            state
                .config
                .reasons_at(&anchor)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
    });
    dynamic(move || {
        let reasons = reasons.get();
        untracked(move || {
            view()
                .min_w(px(0.0))
                .children(
                    reasons
                        .into_iter()
                        .map(|reason| cell(app, reason, Token::Fail, false).into_element()),
                )
                .into_element()
        })
    })
    .into_element()
}

/// Remove's warning: what removing changes and every key of the draft that still names the
/// profile, each a button to it; then Remove (Remove anyway while any remain) and Cancel, the
/// keyboard on Cancel. The keys are left for the owner: the app never picks another profile.
fn removal(app: AppState, name: String, removing: Signal<bool>) -> AnyElement {
    let form = app.form;
    let read = name.clone();
    let references = computed(move || {
        app.settings.with(|state| {
            document::parse(&state.config.draft)
                .map(|doc| profiles::references(&doc, &read))
                .unwrap_or_default()
        })
    });
    let body = dynamic(move || {
        let references = references.get();
        let name = name.clone();
        untracked(move || {
            let mut lines = vec![
                cell(
                    app,
                    profiles::removal(&name, &references),
                    Token::Ink,
                    false,
                )
                .into_element(),
            ];
            for (at, reference) in references.iter().enumerate() {
                let path = reference.path.clone();
                lines.push(
                    view()
                        .flex_row()
                        .child(button(
                            app,
                            reference.place.clone(),
                            ButtonKind::Quiet,
                            format!("btn-config-profile-ref-{at}"),
                            move || {
                                let path = path.clone();
                                app.act(app.settings, move |state, _| state.show_config(&path));
                            },
                        ))
                        .into_element(),
                );
            }
            let safe = element_ref();
            after_answer(move || safe.focus());
            let gone = name.clone();
            let remove = button_with(
                app,
                if references.is_empty() {
                    profiles::REMOVE
                } else {
                    profiles::REMOVE_ANYWAY
                },
                ButtonKind::Ghost,
                ButtonOptions {
                    danger: true,
                    ..ButtonOptions::default()
                },
                "btn-config-remove-profiles",
                move || {
                    edit(app, Edit::RemoveEntry(Part::Profiles, gone.clone()));
                },
            );
            let cancel = button_with(
                app,
                profiles::CANCEL,
                ButtonKind::Quiet,
                ButtonOptions {
                    focus: Some(safe),
                    ..ButtonOptions::default()
                },
                "btn-config-remove-cancel",
                move || removing.set(false),
            );
            lines.push(button_row(app, vec![remove, cancel]));
            view()
                .min_w(px(0.0))
                .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                .children(lines)
                .into_element()
        })
    })
    .into_element();
    let panel = view()
        .test_id("config-profile-remove")
        .role(AccessibilityRole::Group)
        .label(profiles::REMOVE)
        .child(body);
    view()
        .min_w(px(0.0))
        .child(space(app, 8.0))
        .child(boxed(app, panel))
        .into_element()
}

/// A panel set off from the form: outlined on a screen; a terminal's blank rows set it off.
fn boxed(app: AppState, panel: View) -> AnyElement {
    let panel = panel.min_w(px(0.0));
    match app.form {
        Form::Terminal => panel,
        _ => panel
            .p(px(12.0))
            .border_1()
            .border_color(color(Token::Rule2))
            .rounded(px(8.0)),
    }
    .into_element()
}

/// A model or effort: the shape's free-text field with its note, what it inherits unset and the
/// refusal's reasons; under it the values other profiles of this harness set, one press each,
/// and Use harness default, which unsets it. The offers ignore this profile's own value, so
/// picking or typing one never redraws them and the keyboard stays where it was.
fn chosen_field(
    app: AppState,
    cx: &Cx,
    shape: &Field,
    name: &str,
    key: &'static str,
) -> AnyElement {
    let live = cx.live;
    let shown = Field {
        note: Some(profiles::field_note(key)),
        ..shape.clone()
    };
    let (names, profile, path) = (cx.names.clone(), name.to_owned(), shape.path.clone());
    let offered =
        computed(move || live.with(|live| profiles::offers(&live.draft, &names, &profile, key)));
    let offers = dynamic(move || {
        let values = offered.get();
        let path = path.clone();
        untracked(move || offer_row(app, &path, key, values))
    })
    .into_element();
    view()
        .min_w(px(0.0))
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(field(app, &shown, live))
        .child(row(app, "", offers))
        .into_element()
}

/// The offered values under their caption, then Use harness default.
fn offer_row(app: AppState, path: &Path, key: &'static str, values: Vec<String>) -> AnyElement {
    let form = app.form;
    let mut parts: Vec<AnyElement> = Vec::new();
    if !values.is_empty() {
        parts.push(cell(app, profiles::offers_caption(key), Token::Ink3, false).into_element());
    }
    for value in values {
        let picked = path.clone();
        let id = format!("seg-config-{path}-{value}");
        parts.push(button(
            app,
            value.clone(),
            ButtonKind::Ghost,
            id,
            move || edit(app, Edit::Text(picked.clone(), value.clone())),
        ));
    }
    let unset = path.clone();
    parts.push(button(
        app,
        profiles::HARNESS_DEFAULT,
        ButtonKind::Quiet,
        format!("btn-config-{path}-unset"),
        move || edit(app, Edit::Text(unset.clone(), String::new())),
    ));
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .min_w(px(0.0))
        .gap(px(match form {
            Form::Terminal => 8.0,
            _ => 6.0,
        }))
        .role(AccessibilityRole::Group)
        .label(profiles::offers_caption(key))
        .children(parts)
        .into_element()
}

/// The arguments, folded under Advanced: open while a refusal names them or the last edit added
/// one of their lines, and whenever a refusal comes to name them.
fn advanced(app: AppState, args: Field, live: Memo<Live>) -> AnyElement {
    let form = app.form;
    let path = args.path.clone();
    let named = path.clone();
    let refused = computed(move || {
        app.settings
            .with(|state| state.config.reasons_at(&named).next().is_some())
    });
    let started = app.settings.with_untracked(|state| {
        let added = state.config.added.as_ref();
        added.is_some_and(|added| added.0.starts_with(&path.0))
    });
    let open = signal(started || refused.get_untracked());
    effect(move || {
        if refused.get() {
            open.set(true);
        }
    });
    let jump_path = path.clone();
    let handle = element_ref();
    on_jump(
        app,
        move |path| *path == jump_path,
        move || open.set(true),
        handle,
    );
    let count = computed(move || live.with(|live| live.draft.lines(&path).len()));
    let toggle = dynamic(move || {
        let (shown, count) = (open.get(), count.get());
        untracked(move || {
            button_with(
                app,
                profiles::advanced(shown, count),
                ButtonKind::Quiet,
                ButtonOptions {
                    focus: Some(handle),
                    ..ButtonOptions::default()
                },
                "btn-config-profile-advanced",
                move || {
                    open.update(|open| *open = !*open);
                    after_answer(move || handle.focus());
                },
            )
        })
    })
    .into_element();
    let fields =
        dynamic(move || open.get().then(|| untracked(|| field(app, &args, live)))).into_element();
    view()
        .min_w(px(0.0))
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .child(view().flex_row().child(toggle))
        .child(fields)
        .into_element()
}
