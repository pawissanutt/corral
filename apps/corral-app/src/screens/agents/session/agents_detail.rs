//! What the Agents tab shows about one agent and around it: the selected agent's detail (its
//! head and facts, brief and report clipped with "more", what it owns and waits for, what it does
//! now, its edits, the owner's steers, its question, verdict and findings, a pending model,
//! edits outside its files, its error, what retention dropped, then its controls); the controls
//! every agent and job view shares, where one the host cannot run now stays as its muted label
//! with the reason; the one confirmation a cancel or stop asks; an agent's model choices; and
//! the timeline. test ids `session-agent-detail`, `btn-child-<verb>`, `btn-job-<verb>`,
//! `fleet-confirm`, `btn-fleet-keep`, `btn-fleet-confirm`, `fleet-models`,
//! `seg-child-model-<n>`, `btn-models-close`, `btn-more-brief`, `btn-more-report`,
//! `btn-more-edits`, `session-timeline`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{
    AgentDetail, Clipped, ConfirmView, EditList, FleetButton, ModelPicker, Timeline,
};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::super::run;
use super::super::cards::frame;
use super::super::hue_words;
use super::{Tab, cell};

/// Words at the detail's size in `ink`, wrapping.
pub(super) fn words(form: Form, content: impl Into<SharedString>, ink: AdaptiveColor) -> Text {
    let words = text(content.into()).text_color(ink);
    match form {
        Form::Terminal => words,
        Form::Desktop => words
            .font_family(FONT_TEXT)
            .text_size(px(12.5))
            .line_height(px(18.0)),
        Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(14.0))
            .line_height(px(20.0)),
    }
}

/// A state's words in its hue: ink-2 for a neutral one.
fn toned(form: Form, content: impl Into<SharedString>, hue: Hue) -> Text {
    words(form, content, hue_words(form, hue))
}

/// The head of an agent or a job: its mark, its id in bold mono, its state word and its title.
pub(super) fn head(
    app: AppState,
    mark_of: (crate::model::board::Mark, Hue),
    id: &str,
    state: &str,
    title: &str,
) -> View {
    let form = app.form;
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(mark(app, mark_of.0, mark_of.1))
        .child(
            cell(form, id.to_owned(), Token::Ink, true)
                .flex_shrink_0()
                .font_weight(FontWeight::BOLD),
        )
        .child(
            text(state.to_owned())
                .number_of_lines(1u32)
                .flex_shrink_0()
                .text_color(hue_words(form, mark_of.1))
                .when(form != Form::Terminal, |t| {
                    t.font_family(FONT_TEXT).text_size(px(12.0))
                }),
        )
        .child(
            cell(form, title.to_owned(), Token::Ink, false)
                .flex_1()
                .font_weight(FontWeight::SEMIBOLD),
        )
}

/// The controls of an agent or a job: the first one the host can run now is the primary, a
/// destructive one is drawn as such, one the host cannot run now is muted and does nothing, its
/// reason in a line under the row.
pub(super) fn controls(app: AppState, buttons: &[FleetButton]) -> AnyElement {
    let form = app.form;
    let first = buttons
        .iter()
        .position(|button| button.unavailable.is_none() && !button.danger);
    let row = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 8.0 }))
        .children(buttons.iter().enumerate().map(|(n, control)| {
            let action = control.action.clone();
            let inert = control.unavailable.is_some();
            button_with(
                app,
                control.label.clone(),
                if Some(n) == first {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Ghost
                },
                ButtonOptions {
                    disabled: inert,
                    danger: control.danger,
                    key_hint: control.key,
                    ..ButtonOptions::default()
                },
                control.id.clone(),
                move || {
                    if !inert {
                        run(app, action.clone());
                    }
                },
            )
        }));
    let reasons = buttons.iter().filter_map(|control| {
        control.unavailable.as_ref().map(|why| {
            words(
                form,
                format!("{} unavailable: {why}", control.label),
                color(Token::Ink3),
            )
            .into_element()
        })
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(row)
        .children(reasons.collect::<Vec<_>>())
        .into_element()
}

/// A long text on the raised bubble (plain in a terminal), clipped until "more" opens it, with
/// what the host itself left out.
fn clipped(app: AppState, heading: &'static str, clipped: &Clipped) -> AnyElement {
    let form = app.form;
    let body = words(form, clipped.text.clone(), color(Token::Ink));
    let body = match form {
        Form::Terminal => body.into_element(),
        _ => view()
            .px(px(12.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .bg(color(Token::Surface2))
            .child(body)
            .into_element(),
    };
    let more = clipped.more.clone().map(|(label, id, action)| {
        button(app, label, ButtonKind::Quiet, id, move || {
            run(app, action.clone())
        })
    });
    let note = clipped
        .note
        .map(|note| words(form, note, color(Token::Ink3)).into_element());
    view()
        .gap(px(4.0))
        .child(h3(app, heading, None))
        .child(body)
        .children(note)
        .children(more)
        .into_element()
}

fn edits(app: AppState, list: &EditList) -> AnyElement {
    let form = app.form;
    let lines = list.lines.iter().map(|edit| {
        let ink = if edit.outside {
            hue_ink(Hue::Held)
        } else {
            color(Token::Ink2)
        };
        let mut said = edit.path.clone();
        if edit.outside {
            said.push_str("  ! outside its files");
        }
        view()
            .flex_row()
            .gap(px(12.0))
            .child(
                text(said)
                    .number_of_lines(1u32)
                    .truncate()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_color(ink)
                    .when(form != Form::Terminal, |t| {
                        t.font_family(FONT_MONO).text_size(px(12.0))
                    }),
            )
            .children(
                edit.at
                    .clone()
                    .map(|at| cell(form, at, Token::Ink3, true).flex_shrink_0()),
            )
            .into_element()
    });
    let more = list.more.clone().map(|(label, action)| {
        button(app, label, ButtonKind::Quiet, "btn-more-edits", move || {
            run(app, action.clone())
        })
    });
    view()
        .gap(px(2.0))
        .child(h3(app, list.heading.clone(), None))
        .children(lines.collect::<Vec<_>>())
        .children(more)
        .into_element()
}

/// `lead · time · tail`; only `lead` without a tail.
pub(super) fn timed(lead: &str, time: String, tail: &str) -> String {
    if tail.is_empty() {
        return lead.to_owned();
    }
    [lead, time.as_str(), tail].join(" · ")
}

/// The selected agent's detail, built again only when what its controls and sections are made
/// of changes; its facts line (running time, cost) and what it does now follow in place.
pub(super) fn detail(app: AppState, tab: Tab) -> AnyElement {
    let frame = tab.part(|t| {
        t.detail.clone().map(|detail| AgentDetail {
            facts: Default::default(),
            activity: None,
            ..detail
        })
    });
    dynamic(move || {
        frame
            .get()
            .flatten()
            .map(|detail| untracked(|| detail_frame(app, tab, &detail)))
    })
    .into_element()
}

fn detail_frame(app: AppState, tab: Tab, detail: &AgentDetail) -> AnyElement {
    let form = app.form;
    let gap = if form == Form::Terminal { 0.0 } else { 6.0 };
    let live = tab.part(|t| {
        t.detail
            .as_ref()
            .map(|detail| (detail.facts.clone(), detail.activity.clone()))
    });
    let child = detail.child.clone();
    let facts_line = cell(
        form,
        move || {
            let time = tab.time(&child);
            live.with(|live| {
                live.as_ref()
                    .and_then(|live| live.as_ref())
                    .map(|((lead, tail), _)| timed(lead, time, tail))
                    .unwrap_or_default()
            })
        },
        Token::Ink3,
        true,
    );
    let doing = computed(move || {
        live.with(|live| {
            live.as_ref()
                .and_then(|live| live.as_ref())
                .is_some_and(|(_, activity)| activity.is_some())
        })
    });
    let activity = dynamic(move || {
        doing.get().then(|| {
            untracked(|| {
                text(move || {
                    live.with(|live| {
                        live.as_ref()
                            .and_then(|live| live.as_ref())
                            .and_then(|(_, activity)| activity.clone())
                            .map(|activity| format!("Now: {activity}"))
                            .unwrap_or_default()
                    })
                })
                .text_color(hue_words(form, Hue::Worker))
                .when(form != Form::Terminal, |t| {
                    t.font_family(FONT_TEXT)
                        .text_size(px(if form == Form::Phone { 14.0 } else { 12.5 }))
                        .line_height(px(if form == Form::Phone { 20.0 } else { 18.0 }))
                })
                .into_element()
            })
        })
    });
    let mut facts: Vec<AnyElement> = Vec::new();
    if let Some(owns) = &detail.owns {
        facts.push(words(form, owns.clone(), color(Token::Ink2)).into_element());
    }
    facts.push(activity);
    if let Some((said, bring)) = &detail.asks {
        let line = toned(form, said.clone(), Hue::You);
        facts.push(match bring.clone() {
            Some(action) => view()
                .test_id("btn-child-question")
                .role(AccessibilityRole::Button)
                .label(format!("{said}; press to answer"))
                .cursor_pointer()
                .on_press(move || run(app, action.clone()))
                .child(line.font_weight(FontWeight::SEMIBOLD))
                .into_element(),
            None => line.into_element(),
        });
    }
    if let Some((hue, said)) = &detail.verdict {
        facts.push(toned(form, said.clone(), *hue).into_element());
    }
    facts.extend(detail.findings.iter().map(|finding| {
        view()
            .pl(if form == Form::Terminal {
                term::cells(2.0)
            } else {
                px(14.0)
            })
            .child(words(form, finding.clone(), color(Token::Ink2)))
            .into_element()
    }));
    if let Some(model) = &detail.pending_model {
        facts.push(
            words(
                form,
                format!("Switches to {model} at its next turn"),
                color(Token::Ink2),
            )
            .into_element(),
        );
    }
    if let Some(outside) = &detail.outside {
        facts.push(toned(form, format!("! {outside}"), Hue::Held).into_element());
    }
    if let Some(error) = &detail.error {
        facts.push(toned(form, format!("✗ {error}"), Hue::Fail).into_element());
    }
    if let Some(dropped) = &detail.dropped {
        facts.push(toned(form, dropped.clone(), Hue::Held).into_element());
    }
    let steers = (!detail.steers.is_empty()).then(|| {
        view()
            .gap(px(2.0))
            .child(h3(app, "Steered", None))
            .children(
                detail
                    .steers
                    .iter()
                    .map(|steer| words(form, steer.clone(), color(Token::Ink2)).into_element())
                    .collect::<Vec<_>>(),
            )
            .into_element()
    });
    let read = detail.read.clone().map(|(hue, said)| {
        let retry = detail.retry.clone().map(|action| {
            button(
                app,
                "Retry",
                ButtonKind::Quiet,
                "btn-retry-child",
                move || run(app, action.clone()),
            )
        });
        view()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .child(toned(form, said, hue).flex_1().min_w(px(0.0)))
            .children(retry)
            .into_element()
    });
    view()
        .test_id("session-agent-detail")
        .gap(px(gap))
        .child(head(
            app,
            (detail.mark, detail.hue),
            &detail.child,
            detail.state,
            &detail.title,
        ))
        .child(facts_line)
        .children(facts)
        .child(controls(app, &detail.buttons))
        .children(read)
        .children(
            detail
                .brief
                .as_ref()
                .map(|brief| clipped(app, "Brief", brief)),
        )
        .children(steers)
        .children(detail.edits.as_ref().map(|list| edits(app, list)))
        .children(
            detail
                .report
                .as_ref()
                .map(|report| clipped(app, "Report", report)),
        )
        .into_element()
}

/// The confirmation of a cancel or stop: what it does, the safe choice holding the keys first.
fn confirm(app: AppState, ask: &ConfirmView) -> AnyElement {
    let form = app.form;
    let (keep, keep_action) = ask.no.clone();
    let (yes, yes_action) = ask.yes.clone();
    let safe = element_ref();
    after_answer(move || safe.focus());
    let buttons = view()
        .flex_row()
        .flex_wrap()
        .gap(px(if form == Form::Terminal { 16.0 } else { 8.0 }))
        .child(button_with(
            app,
            keep,
            ButtonKind::Ghost,
            ButtonOptions {
                focus: Some(safe),
                key_hint: Some("esc"),
                ..ButtonOptions::default()
            },
            "btn-fleet-keep",
            move || run(app, keep_action.clone()),
        ))
        .child(button_with(
            app,
            yes,
            ButtonKind::Ghost,
            ButtonOptions {
                danger: true,
                ..ButtonOptions::default()
            },
            "btn-fleet-confirm",
            move || run(app, yes_action.clone()),
        ));
    let title = ask.title.clone();
    frame(
        app,
        "fleet-confirm",
        move || title.clone(),
        None,
        vec![
            words(form, ask.detail.clone(), color(Token::Ink2)).into_element(),
            buttons.into_element(),
        ],
        true,
    )
}

/// An agent's model choices: what it runs on now, then each model, the current one marked, an
/// unapproved one muted with why.
fn models(app: AppState, picker: &ModelPicker) -> AnyElement {
    let form = app.form;
    let close = picker.close.clone();
    let close = button(
        app,
        "Close",
        ButtonKind::Quiet,
        "btn-models-close",
        move || run(app, close.clone()),
    );
    let mut rows: Vec<AnyElement> =
        vec![words(form, picker.current.clone(), color(Token::Ink2)).into_element()];
    if let Some((hue, said)) = &picker.note {
        rows.push(toned(form, said.clone(), *hue).into_element());
    }
    rows.extend(picker.choices.iter().map(|choice| {
        let action = choice.action.clone();
        let inert = choice.unavailable.is_some();
        let lead = if choice.current { "● " } else { "○ " };
        let label = cell(
            form,
            format!("{lead}{}", choice.label),
            if inert { Token::Ink3 } else { Token::Ink },
            true,
        )
        .font_weight(if choice.current {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        });
        let mut said: Vec<String> = choice.detail.iter().cloned().collect();
        if let Some(why) = &choice.unavailable {
            said.push(format!("unavailable: {why}"));
        }
        let about = (!said.is_empty())
            .then(|| words(form, said.join(" · "), color(Token::Ink3)).into_element());
        let body = view().gap(px(2.0)).child(label).children(about);
        let line = view()
            .test_id(&choice.id)
            .role(AccessibilityRole::Button)
            .label(match &choice.unavailable {
                Some(why) => format!("{}, unavailable: {why}", choice.label),
                None => choice.label.clone(),
            })
            .accessibility_state(AccessibilityState {
                selected: choice.current,
                disabled: inert,
                ..AccessibilityState::default()
            })
            .when(!inert, |line| {
                line.cursor_pointer()
                    .on_press(move || run(app, action.clone()))
            })
            .child(body);
        match form {
            Form::Terminal => line.into_element(),
            _ => line
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .when(!inert, |line| line.hover(|s| s.bg(color(Token::Hover))))
                .into_element(),
        }
    }));
    let title = picker.title.clone();
    view()
        .test_id("fleet-models")
        .child(frame(
            app,
            "fleet-models-card",
            move || title.clone(),
            Some(close),
            rows,
            true,
        ))
        .into_element()
}

/// What the tab asks the owner above what it shows: a confirmation, else the model choices;
/// built again only when what it asks changes.
pub(super) fn asking(app: AppState, tab: Tab) -> AnyElement {
    let asked = tab.part(|t| (t.confirm.clone(), t.picker.clone()));
    dynamic(move || {
        let (confirming, picking) = asked.get()?;
        untracked(|| {
            confirming
                .as_ref()
                .map(|ask| confirm(app, ask))
                .or_else(|| picking.as_ref().map(|picker| models(app, picker)))
        })
    })
    .into_element()
}

/// When each agent ran: one bar per agent on the shared axis, its end glyph after it. The rows
/// are built again only when an agent starts, ends or changes state; the bars and the axis
/// follow the clock in place.
pub(super) fn timeline(app: AppState, tab: Tab) -> AnyElement {
    let shape = tab.part(|t| t.timeline.clone());
    dynamic(move || {
        let timeline = shape.get().flatten()?;
        Some(untracked(|| timeline_rows(app, tab, &timeline)))
    })
    .into_element()
}

fn timeline_rows(app: AppState, tab: Tab, timeline: &Timeline) -> AnyElement {
    let form = app.form;
    let bars = timeline.rows.iter().map(|row| {
        let action = row.action.clone();
        let bar = {
            let child = row.child.clone();
            computed(move || {
                tab.full
                    .with(|t| t.as_ref().and_then(|t| t.clock.bars.get(&child).copied()))
                    .unwrap_or((0.0, 0.0))
            })
        };
        let label = cell(form, row.label.clone(), Token::Ink, true)
            .flex_shrink_0()
            .w(if form == Form::Terminal {
                term::cells(11.0)
            } else {
                px(80.0)
            });
        let track = view()
            .flex_1()
            .min_w(px(0.0))
            .h(if form == Form::Terminal {
                term::rows(1.0)
            } else {
                px(8.0)
            })
            .relative()
            .child(
                view()
                    .absolute()
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .left(move || Length::from(relative(bar.get().0.clamp(0.0, 1.0))))
                    .w(move || {
                        let (start, end) = bar.get();
                        Length::from(relative((end - start).clamp(0.0, 1.0).max(0.01)))
                    })
                    .rounded(px(if form == Form::Terminal { 0.0 } else { 3.0 }))
                    .bg(hue_ink(row.hue)),
            );
        view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .label(format!("{} on the timeline", row.label))
            .on_press(move || run(app, action.clone()))
            .child(label)
            .child(track)
            .children(
                row.glyph
                    .map(|glyph| text(glyph).text_color(hue_ink(row.hue))),
            )
            .into_element()
    });
    let axis = cell(
        form,
        move || {
            tab.full
                .with(|t| t.as_ref().and_then(|t| t.clock.axis_end.clone()))
                .map(|end| format!("0s – {end}"))
                .unwrap_or_default()
        },
        Token::Ink3,
        true,
    );
    view()
        .test_id("session-timeline")
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(h3(app, timeline.heading.clone(), None))
        .children(bars.collect::<Vec<_>>())
        .child(view().flex_row().justify_end().child(axis))
        .into_element()
}
