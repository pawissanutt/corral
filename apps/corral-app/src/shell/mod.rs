//! The three shells, the only idiom-specific code, and what every shell mounts once over its
//! pages: the toast, the fresh confirmation, the refusal sheet and the Agents switcher. The connection banner is the kit's
//! (`kit::notice::connection`): a phone page sets it under its large title.

pub mod desktop;
pub mod lock;
mod pane;
pub mod phone;
pub mod terminal;
mod terminal_bar;
mod terminal_keys;

use std::cell::Cell;
use std::rc::Rc;

use nsh_base::button as press_target;
use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::act::NextButton;
use crate::model::agents::switcher::{
    AGENTS_CHORD, AGENTS_KEY, AGENTS_LABEL, Entry, NEW_ASSISTANT, NEW_ASSISTANT_NOTE, PINNED,
    RECENT_HEADING, SWITCHER_EMPTY, SWITCHER_SUB, SWITCHER_SUB_PANE,
};
use crate::model::commands::{
    CONFIRM_CANCEL, CONFIRM_CHECK, CONFIRM_EFFECT_HEADING, CONFIRM_SUB, REFUSED_SUB,
    act_confirm_label, act_confirm_word,
};
use crate::model::nav;
use crate::model::{Area, DataState, Destination, Form, Id, SettingsSection};
use crate::state::AppState;
use crate::ui::atoms::TOUCH_TARGET;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::overlay::{self, Confirm, Modal};
use crate::ui::kit::text::{dim, h3, label_value, mono, plain};
use crate::ui::kit::walk::walk;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// Whether the board shows as stale: corrald down or this device offline, where what shows is
/// the last known state (the mock's `.is-stale`). Disconnected shows no earlier state to fade.
pub fn stale(app: AppState) -> bool {
    app.data.get() != DataState::Disconnected && app.env().conn.refuses()
}

/// Tracked: the places the sidebar and the key line list, in order. An assistant-only
/// workspace has no board, so its board places are left out rather than shown unavailable.
pub fn places(app: AppState) -> Vec<Area> {
    app.feed.with(nav::places)
}

/// Tracked: Activity's pages in this workspace, which the sidebar lists under Activity while
/// one of them shows.
pub fn activity_pages(app: AppState) -> Vec<Destination> {
    app.feed.with(nav::activity_pages)
}

/// Tracked: the place whose row is current: the destination's, a page on top included (a
/// task's page is under Tasks, a session's under Sessions).
pub fn current_area(app: AppState) -> Area {
    app.destination().area()
}

/// A desktop window this narrow or narrower folds the sidebar to its rail of icons, whatever was
/// chosen while wide: the page's content needs about 760 pt beside the labels' 208, and a tablet
/// held either way lands at or under this width. The rail and its peek keep the sidebar there.
const DESKTOP_NARROW: f32 = 1024.0;

/// Whether the measured window or grid is too narrow for the sidebar's labels (tracked).
pub fn sidebar_narrow(app: AppState) -> bool {
    match app.form {
        Form::Desktop => window().width.0 <= DESKTOP_NARROW,
        Form::Terminal => crate::ui::term::sidebar_folds(app),
        Form::Phone => false,
    }
}

/// Whether the desktop layout gives the sidebar only its rail (tracked): always while narrow,
/// by choice while wide. Labels asked for while narrow open over the page instead.
pub fn sidebar_collapsed(app: AppState) -> bool {
    app.sidebar.get().collapsed(sidebar_narrow(app))
}

/// A Settings section's short name in the folded sidebar.
pub const fn section_short(section: SettingsSection) -> &'static str {
    match section {
        SettingsSection::Devices => "Dev",
        SettingsSection::Daemon => "Dmn",
        SettingsSection::Connection => "Conn",
        SettingsSection::Notifications => "Notif",
        SettingsSection::Config => "Conf",
        SettingsSection::General => "Gen",
    }
}

/// The toast over `app.toast`, leaving after 3.4 s or when the next one replaces it.
/// test_id "toast".
pub fn toast(app: AppState) -> AnyElement {
    overlay::toast(app, move || app.toast.get(), move || app.toast.set(None))
}

/// The one local question a destructive or forced operation asks: its title, the CLI line it
/// runs with every argument, its lasting effect and, when forced, what it skips; then Cancel
/// and the confirming button (`app.confirm_no()`, `app.confirm_yes()`). Nothing has been
/// sent while it shows. The phone shows it as a sheet; a terminal asks for the operation's
/// word to be typed. Escape and the scrim cancel. test_id "confirm" ("confirm-cancel",
/// "confirm-confirm", "confirm-word").
pub fn confirmation(app: AppState) -> AnyElement {
    overlay::modal(
        app,
        move || app.confirm.with(Option::is_some),
        move || app.confirm_no(),
        move || confirm_modal(app),
    )
}

/// What the sheet says in the labelled preview: the operation runs only in the sample.
const PREVIEW_SUB: &str = "Preview simulation · runs on this device only; nothing is sent.";

fn confirm_modal(app: AppState) -> Modal {
    let simulated = untracked(|| app.data.get()) == DataState::Fixture;
    let (title, heading, rows, sends) = app
        .confirm
        .with_untracked(|asked| {
            asked.as_ref().map(|asked| {
                (
                    asked.title().to_owned(),
                    asked.heading(),
                    asked.rows(),
                    asked.sends(),
                )
            })
        })
        .unwrap_or((String::new(), CONFIRM_EFFECT_HEADING, Vec::new(), true));
    let body = view()
        .child(h3(app, heading, None))
        .child(command_rows(app, rows))
        .into_element();
    let sub = match (sends, simulated) {
        (false, _) => LEAVE_SUB,
        (true, true) => PREVIEW_SUB,
        (true, false) => CONFIRM_SUB,
    };
    Modal {
        test_id: String::from("confirm"),
        sub: Some(sub.to_owned()),
        body,
        foot: None,
        done: None,
        confirm: Some(Confirm {
            label: act_confirm_label(&title, app.form),
            cancel: CONFIRM_CANCEL.to_owned(),
            word: act_confirm_word(&title),
            check: (sends && app.form == Form::Phone).then(|| CONFIRM_CHECK.to_owned()),
            on_confirm: Rc::new(move || app.confirm_yes()),
        }),
        title,
    }
}

/// What the sheet says when leaving with unsaved edits: nothing leaves this device.
const LEAVE_SUB: &str = "Asked once, on this device. Nothing is sent to corrald.";

/// The operation's rows as a label-value list, bounded so a long note scrolls inside the
/// sheet instead of pushing its buttons away.
fn command_rows(app: AppState, rows: Vec<(String, String)>) -> AnyElement {
    let list = view().gap(px(5.0)).children(
        rows.into_iter()
            .map(|(label, value)| label_value(app, label, mono(app, value).into_element())),
    );
    match app.form {
        Form::Terminal => list.into_element(),
        Form::Desktop | Form::Phone => scroll()
            .test_id("confirm-command")
            .max_h(px(200.0))
            .child(list)
            .into_element(),
    }
}

/// The engine's refusal of an operation: why, then one button per next step in the engine's
/// order, the first one filled. A forced step reads "… anyway" and says what it skips under
/// it, so pressing it is its one confirmation. Each button shows the CLI line it runs. Escape,
/// the scrim and Close dismiss it; nothing runs. test_id "refused" ("refused-done",
/// "btn-refusal-<n>").
pub fn refusal(app: AppState) -> AnyElement {
    overlay::modal(
        app,
        move || app.refusal.with(Option::is_some),
        move || app.close_refusal(),
        move || refusal_modal(app),
    )
}

fn refusal_modal(app: AppState) -> Modal {
    let (title, reason, buttons) = app
        .refusal
        .with_untracked(|sheet| {
            sheet.as_ref().map(|sheet| {
                (
                    format!("Refused · {}", sheet.title),
                    sheet.refusal.reason.clone(),
                    sheet.buttons(),
                )
            })
        })
        .unwrap_or_default();
    let gap = if app.form == Form::Terminal {
        0.0
    } else {
        12.0
    };
    let none = buttons.is_empty();
    let steps: Vec<AnyElement> = buttons
        .into_iter()
        .enumerate()
        .map(|(index, step)| next_step(app, index, step))
        .collect();
    let body = view()
        .gap(px(gap))
        .child(plain(app, reason))
        .when(none, |body| {
            body.child(
                plain(app, "The engine named no step past this.").text_color(color(Token::Ink3)),
            )
        })
        .children(steps)
        .into_element();
    Modal {
        test_id: String::from("refused"),
        title,
        sub: Some(REFUSED_SUB.to_owned()),
        body,
        foot: None,
        done: Some("Close".to_owned()),
        confirm: None,
    }
}

/// One next step: its button, the CLI line it runs and, when forced, what it skips.
fn next_step(app: AppState, index: usize, step: NextButton) -> AnyElement {
    let skips = step
        .skips
        .map(|skips| plain(app, format!("Skips {skips}.")).text_color(color(Token::Ink2)));
    view()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(button_with(
            app,
            step.label,
            if step.primary {
                ButtonKind::Primary
            } else {
                ButtonKind::Ghost
            },
            ButtonOptions {
                wide: app.form == Form::Phone,
                danger: step.skips.is_some(),
                ..ButtonOptions::default()
            },
            format!("btn-refusal-{index}"),
            move || app.run_next(index),
        ))
        .children(skips)
        .child(mono(app, step.command).text_color(color(Token::Ink3)))
        .into_element()
}

/// What shows or hides the desktop side pane where a page's head or the top bar shows it:
/// "Side pane" with its chord, outlined while the pane shows. None off the desktop.
pub fn pane_trigger(app: AppState, test_id: &'static str) -> Option<AnyElement> {
    use crate::model::dock;
    dock::supported(app.form).then(|| {
        let open = computed(move || app.pane_open());
        dynamic(move || {
            let kind = if open.get() {
                ButtonKind::Ghost
            } else {
                ButtonKind::Quiet
            };
            untracked(|| {
                button_with(
                    app,
                    dock::PANE,
                    kind,
                    ButtonOptions {
                        icon: Some(Icon::Split),
                        key_hint: Some(dock::KEY),
                        ..ButtonOptions::default()
                    },
                    test_id,
                    move || app.toggle_pane(),
                )
            })
        })
        .into_element()
    })
}

/// What opens the Agents switcher where a page's head shows it: "Agents" with its chord (none
/// on the phone, which has no keyboard to press it on). The sidebars draw their own row in the
/// shell's colours.
pub fn agents_trigger(app: AppState, test_id: &'static str) -> AnyElement {
    button_with(
        app,
        AGENTS_LABEL,
        ButtonKind::Quiet,
        ButtonOptions {
            icon: Some(Icon::Agents),
            key_hint: Some(if app.form == Form::Terminal {
                AGENTS_CHORD
            } else {
                AGENTS_KEY
            }),
            ..ButtonOptions::default()
        },
        test_id,
        move || app.open_agents(),
    )
}

/// The Agents switcher over every page (`model::agents::switcher`): the conversations pinned on
/// this device, then the recent ones, then New assistant. The conversation last opened here
/// takes the keyboard focus as it opens, so Enter opens it again; the arrows walk the rows, P
/// pins or unpins the focused one; Escape, the scrim and the chord again close it. A dialog on
/// desktop, a sheet on the phone, the framed region over the page in a terminal. test_id
/// "agents" (rows `switch-<id>`, their pins `switch-pin-<id>`, `switch-new`).
pub fn switcher(app: AppState) -> AnyElement {
    overlay::modal(
        app,
        move || app.switcher.get(),
        move || app.close_agents(),
        move || switcher_modal(app),
    )
}

fn switcher_modal(app: AppState) -> Modal {
    let shown = untracked(|| app.agents_switcher());
    let mut walked = Vec::new();
    let mut ids: Vec<Id> = Vec::new();
    let mut rows: Vec<AnyElement> = Vec::new();
    let mut first = None;
    for (heading, entries) in [(PINNED, &shown.pinned), (RECENT_HEADING, &shown.recent)] {
        if entries.is_empty() {
            continue;
        }
        rows.push(
            dim(app, heading)
                .font_weight(FontWeight::BOLD)
                .into_element(),
        );
        for entry in entries {
            let handle = element_ref();
            if shown.selected.as_ref() == Some(&entry.id) {
                first = Some(handle);
            }
            walked.push(handle);
            ids.push(entry.id.clone());
            rows.push(agent_row(app, entry.clone(), handle));
        }
    }
    if ids.is_empty() {
        rows.push(dim(app, SWITCHER_EMPTY).into_element());
    }
    let new = element_ref();
    rows.push(switch_target(
        app,
        format!("+ {NEW_ASSISTANT}"),
        NEW_ASSISTANT_NOTE.to_owned(),
        false,
        new,
        "switch-new".to_owned(),
        move || app.new_assistant(),
    ));
    walked.push(new);
    let focused = Rc::new(Cell::new(None::<usize>));
    for (index, handle) in walked.iter().enumerate() {
        let focused = focused.clone();
        handle.on_focus_in(move || focused.set(Some(index)));
    }
    let first = first.unwrap_or(new);
    after_answer(move || first.focus());
    walk(walked);
    let options = KeyOptions {
        when: When::NotEditing,
        ..KeyOptions::default()
    };
    let beside = crate::model::dock::supported(app.form);
    if beside {
        let (focused, ids) = (focused.clone(), ids.clone());
        on_key_with("s", options, move |_| {
            if let Some(id) = focused.get().and_then(|at| ids.get(at)) {
                app.open_agent_beside(id.clone());
            }
        });
    }
    on_key_with("p", options, move |_| {
        if let Some(id) = focused.get().and_then(|at| ids.get(at)) {
            app.toggle_agent_pin(id);
        }
    });
    Modal {
        test_id: "agents".into(),
        title: AGENTS_LABEL.into(),
        sub: Some(
            if beside {
                SWITCHER_SUB_PANE
            } else {
                SWITCHER_SUB
            }
            .into(),
        ),
        body: view()
            .gap(px(if app.form == Form::Terminal { 0.0 } else { 2.0 }))
            .role(AccessibilityRole::List)
            .children(rows)
            .into_element(),
        foot: None,
        done: None,
        confirm: None,
    }
}

/// One conversation: what opens it (Ask's chat in bold), then its pin.
fn agent_row(app: AppState, entry: Entry, handle: ElementRef) -> AnyElement {
    let id = entry.id.clone();
    let open = switch_target(
        app,
        entry.title,
        entry.note,
        entry.ask,
        handle,
        format!("switch-{id}"),
        move || app.switch_agent(id.clone()),
    );
    view()
        .flex_row()
        .items_center()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(view().flex_1().min_w(px(0.0)).child(open))
        .children(crate::model::dock::supported(app.form).then(|| {
            let id = entry.id.clone();
            button_with(
                app,
                crate::model::dock::OPEN_BESIDE,
                ButtonKind::Quiet,
                ButtonOptions {
                    icon: Some(Icon::Split),
                    icon_only: true,
                    ..ButtonOptions::default()
                },
                format!("switch-side-{id}"),
                move || app.open_agent_beside(id.clone()),
            )
        }))
        .child(pin_toggle(app, entry.id))
        .into_element()
}

/// Pin or Unpin, following the saved pins while the switcher shows; the rows keep the order
/// they opened in, so a pin moves its conversation up the next time.
fn pin_toggle(app: AppState, id: Id) -> AnyElement {
    let pinned = {
        let id = id.clone();
        computed(move || app.agent_pinned(id.as_str()))
    };
    dynamic(move || {
        let pinned = pinned.get();
        let id = id.clone();
        untracked(move || {
            let test_id = format!("switch-pin-{id}");
            button_with(
                app,
                if pinned { "Unpin" } else { "Pin" },
                if pinned {
                    ButtonKind::Ghost
                } else {
                    ButtonKind::Quiet
                },
                ButtonOptions {
                    icon: Some(Icon::Pin),
                    ..ButtonOptions::default()
                },
                test_id,
                move || app.toggle_agent_pin(&id),
            )
        })
    })
}

/// A row of the switcher the walk reaches: its title, and its note under it (beside it in a
/// terminal).
fn switch_target(
    app: AppState,
    title: String,
    note: String,
    bold: bool,
    handle: ElementRef,
    test_id: String,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let spoken = format!("{title}, {note}");
    let words = text(title)
        .number_of_lines(1u32)
        .truncate()
        .when(bold, |words| words.font_weight(FontWeight::BOLD))
        .text_color(color(Token::Ink));
    let note = dim(app, note).number_of_lines(1u32).truncate();
    let row = press_target()
        .on_press(move |_: &()| on_press())
        .element_ref(&handle)
        .accessibility_label(spoken)
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(|s| s.bg(color(Token::Hover)));
    let styled = match app.form {
        Form::Terminal => row
            .flex_row()
            .items_center()
            .gap(term::cells(2.0))
            .h(term::rows(1.0))
            .px(term::cells(1.0))
            .child(words.flex_shrink_0())
            .child(note.flex_1().min_w(px(0.0))),
        Form::Desktop => row
            .gap(px(1.0))
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .child(words.font_family(FONT_TEXT).text_size(px(13.0)))
            .child(note),
        Form::Phone => row
            .justify_center()
            .gap(px(2.0))
            .min_h(px(TOUCH_TARGET + 4.0))
            .py(px(8.0))
            .child(words.font_family(FONT_TEXT).text_size(px(16.0)))
            .child(note),
    };
    view().test_id(&test_id).child(styled).into_element()
}
